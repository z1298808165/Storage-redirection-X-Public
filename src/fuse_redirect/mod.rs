mod attrs;
mod callbacks;
pub(crate) mod config;
mod helpers;
mod inode;
mod metadata;
mod perf;
mod policy;
mod rules;
pub(crate) mod scoped_mount;
pub(super) use rules::normalize_rule_list;

pub(crate) use policy::SharedPolicyTable;

// 公开这些配置类型供 daemon/测试流复用；部分构建目标只使用其中的函数。
pub use config::{
    FuseRedirectConfig, MountRequestFields, fuse_config_from_request, mount_blocking_with_ready,
    scoped_fuse_mount_roots_for_request,
};
pub use scoped_mount::{
    ScopedMountAttempt, ScopedMountReport, conclude_scoped_mount, log_scoped_mount_roots,
};

use crate::platform::{fs, paths};
use attrs::{file_attr_from_metadata, synthetic_dir_attr};
use fuser::{Errno, FileAttr, FileType, Generation, INodeNo, ReplyEmpty, ReplyEntry, Request};
use inode::{remove_inode_path, remove_unreferenced_inode};
use metadata::{cstring_path, errno_from_code, errno_from_io, fix_path_metadata, last_errno};
use perf::FusePerfStats;
use policy::{BackendPath, OperationKind, PolicyRegistry, RedirectPolicy};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

const TTL: Duration = Duration::from_millis(250);
const ROOT_INO: u64 = 1;
const MAX_READ_SIZE: usize = 256 * 1024;
const DIR_CANDIDATE_CACHE_TTL: Duration = Duration::from_millis(250);
static INODE_POLICY_FALLBACKS: AtomicU64 = AtomicU64::new(0);
const INITIAL_DIR_CANDIDATE_CACHE_ENTRIES: usize = 64;
const MEDIA_RW_UID: u32 = 1023;
pub(super) const MEDIA_RW_GID: u32 = 1023;
pub(super) const MAPPED_DIR_MODE: libc::mode_t = 0o2773;
const SHARED_PUBLIC_DIR_MODE: u32 = 0o2770;
/// 一级压缩（去重、剔除被父根覆盖的子路径）后的软目标根数。
///
/// 优先保持"只在通配规则命中的最小具体父目录上挂 FUSE"的设计前提，能不收敛就不收敛。
pub(super) const TARGET_SCOPED_FUSE_ROOTS: usize = 4;
/// 多 scoped 会话模式允许保留的根数上限。
///
/// 这个上限不是规则能力或内核约束，而是资源预算：每个根对应一个 `srx_fuse` 子进程和
/// 一次会话，父进程的挂载等待预算也按根数线性增长。顶层根超过该预算时改用单个共享
/// 存储根 FUSE 会话，仍由原始规则逐路径匹配，不再因为根数退回精度较弱的 namespace。
pub(super) const MAX_SCOPED_FUSE_ROOTS: usize = 16;
const INITIAL_DIR_CANDIDATE_CACHE_BYTES: usize = 256 * 1024;

thread_local! {
    // FUSE 读回调通常在固定工作线程上重复执行，复用 256 KiB 内的缓冲区减少分配。
    static FUSE_READ_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

struct FuseRedirectFs {
    /// 策略注册表；每个回调按调用方 uid 取一次，未命中时回退到会话默认。
    policy: PolicyRegistry,
    /// FUSE 请求由多个内核线程并发派发，读多写少：
    /// 使用读写锁让 read/readdir/fsync 等只读路径可以并行取句柄，避免互斥锁把并发读串行化。
    state: RwLock<FuseState>,
    perf: FusePerfStats,
    passthrough_enabled: AtomicBool,
}

struct FuseState {
    next_ino: u64,
    next_fh: u64,
    inodes: HashMap<String, u64>,
    paths_by_inode: HashMap<u64, String>,
    /// 最近一次按应用策略解析每个 FUSE 路径，用于 root 子进程跨 namespace 接续访问。
    path_policy_uids: HashMap<String, u32>,
    inode_policy_uids: HashMap<u64, u32>,
    inode_path_versions: HashMap<u64, u64>,
    lookup_counts: HashMap<u64, u64>,
    dir_entry_refs: HashMap<u64, u64>,
    files: HashMap<u64, OpenFile>,
    dirs: HashMap<u64, Arc<[DirEntry]>>,
    dir_candidate_cache: HashMap<String, CachedDirCandidates>,
    dir_candidate_cache_bytes: usize,
    dir_candidate_cache_byte_budget: usize,
    dir_candidate_cache_capacity: usize,
    dir_candidate_cache_max_capacity: usize,
}

impl FuseState {
    fn next_handle(&mut self) -> u64 {
        let fh = self.next_fh;
        self.next_fh = self.next_fh.saturating_add(1).max(1);
        fh
    }

    fn clear_dir_candidate_cache(&mut self) {
        self.dir_candidate_cache.clear();
        self.dir_candidate_cache_bytes = 0;
    }
}

fn dir_candidate_cache_capacity_limits() -> (usize, usize, usize) {
    let total_kib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                (fields.next() == Some("MemTotal:")).then(|| fields.next()?.parse::<u64>().ok())?
            })
        })
        .unwrap_or(4 * 1024 * 1024);
    let (max_capacity, byte_budget) = if total_kib < 4 * 1024 * 1024 {
        (64, 256 * 1024)
    } else if total_kib < 8 * 1024 * 1024 {
        (128, 512 * 1024)
    } else {
        (256, 1024 * 1024)
    };
    (
        INITIAL_DIR_CANDIDATE_CACHE_ENTRIES.min(max_capacity),
        max_capacity,
        byte_budget,
    )
}

struct OpenFile {
    // quality-allow(lint-suppression): rel字段保留供调试和诊断输出使用，当前未被读取但不应删除。
    #[allow(dead_code)]
    rel: String,
    file: Option<Arc<File>>,
    // backing 注册必须覆盖整个打开句柄生命周期；提前注销会让内核 passthrough 返回 EIO。
    _backing: Option<Arc<fuser::BackingId>>,
    is_read_only: bool,
}

#[derive(Clone)]
struct DirEntry {
    ino: INodeNo,
    kind: FileType,
    name: String,
    rel: String,
}

#[derive(Clone)]
pub(super) struct DirEntryCandidate {
    rel: String,
    kind: FileType,
    name: String,
}

#[derive(Clone)]
pub(super) struct DirectorySourceSignature {
    path: PathBuf,
    modified: Option<SystemTime>,
    is_dir: Option<bool>,
}

impl DirectorySourceSignature {
    fn capture(path: &Path) -> Self {
        let metadata = std::fs::metadata(path).ok();
        Self {
            path: path.to_path_buf(),
            modified: metadata.as_ref().and_then(|value| value.modified().ok()),
            is_dir: metadata.as_ref().map(std::fs::Metadata::is_dir),
        }
    }

    fn state(&self) -> DirectorySourceState {
        let Some(metadata) = std::fs::metadata(&self.path).ok() else {
            return if self.modified.is_none() && self.is_dir.is_none() {
                DirectorySourceState::Current
            } else {
                DirectorySourceState::Missing
            };
        };
        let modified = metadata.modified().ok();
        let is_dir = Some(metadata.is_dir());
        if self.modified == modified && self.is_dir == is_dir {
            DirectorySourceState::Current
        } else {
            DirectorySourceState::Changed
        }
    }
}

enum DirectorySourceState {
    Current,
    Changed,
    Missing,
}

struct CachedDirCandidates {
    created_at: Instant,
    estimated_bytes: usize,
    candidates: Vec<DirEntryCandidate>,
    sources: Vec<DirectorySourceSignature>,
}

pub(super) fn estimate_cached_dir_candidates_bytes(
    rel: &str,
    candidates: &[DirEntryCandidate],
    sources: &[DirectorySourceSignature],
) -> usize {
    let candidate_bytes = candidates.iter().fold(0usize, |total, candidate| {
        total
            .saturating_add(candidate.rel.len())
            .saturating_add(candidate.name.len())
            .saturating_add(std::mem::size_of::<DirEntryCandidate>())
    });
    let source_bytes = sources.iter().fold(0usize, |total, source| {
        total
            .saturating_add(source.path.to_string_lossy().len())
            .saturating_add(std::mem::size_of::<DirectorySourceSignature>())
    });
    rel.len()
        .saturating_add(candidate_bytes)
        .saturating_add(source_bytes)
        .saturating_add(INITIAL_DIR_CANDIDATE_CACHE_BYTES / 64)
}

/// 新建条目（create/mknod/mkdir）共用的前置解析结果。
struct NewEntryRoute {
    policy: Arc<RedirectPolicy>,
    rel: String,
    backend: BackendPath,
}

impl FuseRedirectFs {
    /// 取出按 uid 策略表句柄。
    ///
    /// 宿主共享会话在 `spawn_mount2` 交出 `FuseRedirectFs` 之后，仍需要按 uid 登记应用策略，
    /// 因此必须在交出所有权之前把句柄取出来，交给控制通道。
    pub(crate) fn policy_table(&self) -> SharedPolicyTable {
        self.policy.shared_table()
    }

    fn new(config: FuseRedirectConfig) -> Option<Self> {
        let package_name = config.package_name.clone();
        // 宿主共享会话服务的是完整存储视图：未登记 uid 必须一律拒绝，否则它会回退到直通策略，
        // 直接读写真实存储——沙盒失效，且越权产生的落点无法靠事后清理恢复。
        let deny_unregistered = config.is_passthrough_host;
        let policy = PolicyRegistry::new(RedirectPolicy::new(config)?, deny_unregistered);
        let (dir_cache_capacity, dir_cache_max_capacity, dir_cache_byte_budget) =
            dir_candidate_cache_capacity_limits();
        let mut inodes = HashMap::new();
        let mut paths_by_inode = HashMap::new();
        inodes.insert(String::new(), ROOT_INO);
        paths_by_inode.insert(ROOT_INO, String::new());

        let perf = FusePerfStats::new(package_name);
        perf.record_dir_cache_capacity(dir_cache_capacity, dir_cache_max_capacity);
        perf.record_dir_cache_budget(0, dir_cache_byte_budget);
        perf.log_dir_cache_config();

        Some(Self {
            policy,
            perf,
            passthrough_enabled: AtomicBool::new(false),
            state: RwLock::new(FuseState {
                next_ino: ROOT_INO + 1,
                next_fh: 1,
                inodes,
                paths_by_inode,
                path_policy_uids: HashMap::new(),
                inode_policy_uids: HashMap::new(),
                inode_path_versions: HashMap::from([(ROOT_INO, 0)]),
                lookup_counts: HashMap::new(),
                dir_entry_refs: HashMap::new(),
                files: HashMap::new(),
                dirs: HashMap::new(),
                dir_candidate_cache: HashMap::new(),
                dir_candidate_cache_bytes: 0,
                dir_candidate_cache_byte_budget: dir_cache_byte_budget,
                dir_candidate_cache_capacity: dir_cache_capacity,
                dir_candidate_cache_max_capacity: dir_cache_max_capacity,
            }),
        })
    }

    fn policy_for_request(&self, req: &Request) -> Arc<RedirectPolicy> {
        self.policy.for_request(req.uid(), req.pid())
    }

    fn policy_for_read_request(
        &self,
        req: &Request,
        ino: Option<INodeNo>,
        rel: Option<&str>,
    ) -> Arc<RedirectPolicy> {
        let policy = self.policy_for_request(req);
        if req.uid() != 0 || !policy.is_deny_all() {
            return policy;
        }
        let cached_uid = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            ino.and_then(|value| state.inode_policy_uids.get(&value.0).copied())
                .or_else(|| rel.and_then(|value| state.path_policy_uids.get(value).copied()))
        };
        if let Some(cached_uid) = cached_uid
            && let Some(cached_policy) = self.policy.for_uid_exact(cached_uid)
        {
            let count = INODE_POLICY_FALLBACKS.fetch_add(1, Ordering::Relaxed) + 1;
            if count == 1 || count.is_multiple_of(256) {
                log::info!(
                    "fuse policy inode fallback uid=0 pid={} ino={} rel={} app_uid={} pkg={} count={}",
                    req.pid(),
                    ino.map(|value| value.0).unwrap_or(0),
                    rel.unwrap_or(""),
                    cached_uid,
                    cached_policy.package_name,
                    count
                );
            }
            return cached_policy;
        }
        policy
    }

    fn remember_policy_for_inode_locked(
        state: &mut FuseState,
        ino: INodeNo,
        rel: &str,
        policy: &RedirectPolicy,
    ) {
        state
            .path_policy_uids
            .insert(rel.to_string(), policy.uid as u32);
        state.inode_policy_uids.insert(ino.0, policy.uid as u32);
    }

    fn ino_for_path_locked(state: &mut FuseState, rel: &str) -> INodeNo {
        if let Some(ino) = state.inodes.get(rel).copied() {
            return INodeNo(ino);
        }
        let ino = state.next_ino;
        state.next_ino = state.next_ino.saturating_add(1).max(ROOT_INO + 1);
        state.inodes.insert(rel.to_string(), ino);
        state.paths_by_inode.insert(ino, rel.to_string());
        state.inode_path_versions.insert(ino, 0);
        INodeNo(ino)
    }

    fn add_lookup_locked(state: &mut FuseState, ino: INodeNo) {
        if ino.0 != ROOT_INO {
            let count = state.lookup_counts.entry(ino.0).or_default();
            *count = count.saturating_add(1);
        }
    }

    fn remove_lookup_locked(state: &mut FuseState, ino: INodeNo, count: u64) {
        if let Some(current) = state.lookup_counts.get_mut(&ino.0) {
            *current = current.saturating_sub(count);
            if *current == 0 {
                state.lookup_counts.remove(&ino.0);
            }
        }
        remove_unreferenced_inode(state, ino.0);
    }

    fn path_for_ino(&self, ino: INodeNo) -> Option<String> {
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        state.paths_by_inode.get(&ino.0).cloned()
    }

    fn backend_for_ino(&self, policy: &RedirectPolicy, ino: INodeNo) -> Result<BackendPath, Errno> {
        let rel = self.path_for_ino(ino).ok_or(Errno::ENOENT)?;
        policy
            .backend_for_relative(&rel, OperationKind::Read)
            .ok_or(Errno::ENOENT)
    }

    fn backend_for_relative(
        &self,
        policy: &RedirectPolicy,
        rel: &str,
        operation: OperationKind,
    ) -> Result<BackendPath, Errno> {
        policy
            .backend_for_relative(rel, operation)
            .ok_or(Errno::ENOENT)
    }

    fn child_rel(parent_rel: &str, name: &OsStr) -> Result<String, Errno> {
        let name_bytes = name.as_bytes();
        if name_bytes.is_empty()
            || name_bytes.contains(&0)
            || name_bytes == b"."
            || name_bytes == b".."
            || name_bytes.contains(&b'/')
        {
            return Err(Errno::EINVAL);
        }
        let name_text = String::from_utf8_lossy(name_bytes).to_string();
        if parent_rel.is_empty() {
            Ok(name_text)
        } else {
            Ok(paths::join(parent_rel, &name_text))
        }
    }

    /// 新建条目的公共前置解析：父 inode → 子相对路径 → 写后端 → 只读拒绝 →
    /// 确保父目录。任一步失败返回 `Errno`，由调用方以各自的 Reply 类型应答；
    /// 只读拒绝在返回前按操作名发出监视事件，三个回调的告警语义与抽取前一致。
    fn route_new_entry(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        operation_name: &'static str,
    ) -> Result<NewEntryRoute, Errno> {
        let Some(parent_rel) = self.path_for_ino(parent) else {
            return Err(Errno::ENOENT);
        };
        let rel = Self::child_rel(&parent_rel, name)?;
        let policy = self.policy_for_request(req);
        let backend = self.backend_for_relative(&policy, &rel, OperationKind::Write)?;
        if backend.is_read_only {
            policy.emit_monitor_read_only_deny(operation_name, &backend);
            return Err(Errno::EROFS);
        }
        self.ensure_parent_for_backend(&policy, &backend)?;
        Ok(NewEntryRoute {
            policy,
            rel,
            backend,
        })
    }

    fn attr_for_backend(
        &self,
        policy: &RedirectPolicy,
        ino: INodeNo,
        backend: &BackendPath,
    ) -> Result<FileAttr, Errno> {
        let metadata = std::fs::symlink_metadata(&backend.path).map_err(errno_from_io)?;
        let mut attr = file_attr_from_metadata(ino, metadata);
        if backend.is_shared_public_backend {
            attr.uid = policy.uid as u32;
            attr.gid = MEDIA_RW_GID;
            if attr.kind == FileType::Directory {
                attr.perm = SHARED_PUBLIC_DIR_MODE as u16;
            }
        }
        Ok(attr)
    }

    fn visible_attr_for_backend(
        &self,
        policy: &RedirectPolicy,
        ino: INodeNo,
        backend: &BackendPath,
    ) -> Result<FileAttr, Errno> {
        match self.attr_for_backend(policy, ino, backend) {
            Ok(attr) => Ok(attr),
            Err(errno) if errno.code() == libc::ENOENT && policy.is_virtual_dir(&backend.rel) => {
                Ok(synthetic_dir_attr(ino, policy.uid as u32, MEDIA_RW_GID))
            }
            Err(errno) => Err(errno),
        }
    }

    fn reply_entry_for_rel(&self, policy: &RedirectPolicy, rel: &str, reply: ReplyEntry) {
        let Some(backend) = policy.backend_for_relative(rel, OperationKind::Read) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let ino = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, rel);
            Self::remember_policy_for_inode_locked(&mut state, ino, rel, policy);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.visible_attr_for_backend(policy, ino, &backend) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(errno) => {
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                Self::remove_lookup_locked(&mut state, ino, 1);
                reply.error(errno);
            }
        }
    }

    fn ensure_parent_for_backend(
        &self,
        policy: &RedirectPolicy,
        backend: &BackendPath,
    ) -> Result<(), Errno> {
        let Some(parent) = backend.path.parent() else {
            return Ok(());
        };
        let parent = parent.to_string_lossy();
        if parent.is_empty() {
            return Ok(());
        }
        let owner_uid = if !crate::metadata_repair::enabled() {
            -1
        } else if backend.is_shared_public_backend {
            MEDIA_RW_UID as i32
        } else {
            policy.uid
        };
        if fs::is_directory(&parent) || fs::create_directory(&parent, owner_uid) {
            fix_path_metadata(
                Path::new(parent.as_ref()),
                policy.uid,
                MAPPED_DIR_MODE,
                backend.is_shared_public_backend,
                true,
            );
            Ok(())
        } else {
            Err(Errno::EIO)
        }
    }

    fn invalidate_dir_candidate_cache(&self) {
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        state.clear_dir_candidate_cache();
    }

    fn open_backend_file(path: &Path, flags: i32, mode: u32) -> Result<File, Errno> {
        let c_path = cstring_path(path)?;
        let fd = unsafe { libc::open(c_path.as_ptr(), flags | libc::O_CLOEXEC, mode) };
        if fd < 0 {
            Err(errno_from_code(last_errno()))
        } else {
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    fn remove_child(
        &self,
        policy: &RedirectPolicy,
        parent: INodeNo,
        name: &OsStr,
        is_dir: bool,
        reply: ReplyEmpty,
    ) {
        let Some(parent_rel) = self.path_for_ino(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let rel = match Self::child_rel(&parent_rel, name) {
            Ok(rel) => rel,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let Some(backend) = policy.backend_for_relative(&rel, OperationKind::Read) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if backend.is_read_only {
            policy.emit_monitor_read_only_deny(if is_dir { "rmdir" } else { "unlink" }, &backend);
            reply.error(Errno::EROFS);
            return;
        }
        let result = if is_dir {
            std::fs::remove_dir(&backend.path)
        } else {
            std::fs::remove_file(&backend.path)
        };
        match result {
            Ok(()) => {
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                remove_inode_path(&mut state, &rel);
                state.clear_dir_candidate_cache();
                reply.ok();
            }
            Err(error) => reply.error(errno_from_io(error)),
        }
    }
}
