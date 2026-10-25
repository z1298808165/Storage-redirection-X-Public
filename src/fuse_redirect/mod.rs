mod attrs;
mod backend_overlay;
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
    request_policy_fingerprint, scoped_fuse_mount_roots_for_request,
};
pub use scoped_mount::{
    ScopedMountAttempt, ScopedMountReport, conclude_scoped_mount, log_scoped_mount_roots,
};

use crate::platform::{fs, paths};
use attrs::{file_attr_from_metadata, synthetic_dir_attr};
use fuser::{
    Errno, FileAttr, FileType, FopenFlags, Generation, INodeNo, ReplyEmpty, ReplyEntry, Request,
};
use inode::{remove_inode_path, remove_unreferenced_inode};
use metadata::{cstring_path, errno_from_code, errno_from_io, fix_path_metadata, last_errno};
use perf::FusePerfStats;
use policy::{
    ANDROID_APP_UID_START, BackendPath, OperationKind, PolicyRegistry, RedirectPolicy,
    RequesterIdentity,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use once_cell::sync::Lazy;

/// attr 有效期：与原生 MP FuseDaemon 对齐（10s）。writeback 回写模式下内核在
/// 本地维护 inode 的 i_size，应用轮询 stat 命中未过期的内核缓存时拿到的是
/// 真实尺寸；TTL 过短（旧值 250ms）会让每次轮询穿透到后端 lstat，读到回刷
/// 滞后的旧尺寸——QQ 相机录制即因此读到 size=0/截断值而判「录制异常」。
const ATTR_TTL: Duration = Duration::from_secs(10);
/// entry 有效期：与原生一致取 0（不缓存查找结果，负查询亦不缓存）。
const ENTRY_TTL: Duration = Duration::ZERO;
const ROOT_INO: u64 = 1;
const MAX_READ_SIZE: usize = 256 * 1024;

/// 系统 uid 请求者身份缓存 TTL：pid 的 /proc 身份极少变化（进程退出即失效，
/// 重用窗口内 cmdline+uid 双校验兜底），5 秒足够吸收 write 等高频路径的重复解析。
const SYSTEM_REQUESTER_CACHE_TTL_MS: i64 = 5_000;
/// 缓存容量上限：系统 uid 进程数量级远小于该值，超限整表清空（简单防膨胀）。
const SYSTEM_REQUESTER_CACHE_CAP: usize = 256;
/// 系统 uid 请求者身份缓存条目：(解析时刻单调毫秒，身份；None 表示 pid 当时
/// 不可归因——缓存失败结果避免 miss 风暴)。
type SystemRequesterCacheEntry = (i64, Option<RequesterIdentity>);
static SYSTEM_REQUESTER_CACHE: Lazy<Mutex<HashMap<i32, SystemRequesterCacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const DIR_CANDIDATE_CACHE_TTL: Duration = Duration::from_millis(250);
static INODE_POLICY_FALLBACKS: AtomicU64 = AtomicU64::new(0);
/// 会话级 ENOENT 诊断限流：前 4 条带策略指纹落日志，用于定位「未登记 uid / 旧映射残留」。
static OPEN_ENOENT_DIAG_COUNT: AtomicU64 = AtomicU64::new(0);
/// create 入口的 ENOENT 诊断限流（与 open 分开计数）。
static CREATE_ENOENT_DIAG_COUNT: AtomicU64 = AtomicU64::new(0);
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
    /// 会话是否协商了 FUSE_WRITEBACK_CACHE：写进内核页缓存、延迟回刷后端。
    /// 决定文件 open/create 应答是否必须携带 FOPEN_KEEP_CACHE——缺失时内核在
    /// 每次 open 都失效该 inode 页缓存，第二个 fd 读正在写的文件会经 GETATTR
    /// 取到回刷滞后的后端 size（截断视图），读方（如录制 I 帧提取）据此判坏。
    writeback_enabled: AtomicBool,
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

/// MediaProvider 应用 uid：宿主 daemon 以 root 运行，直接解析 packages.list
/// （系统维护的权威映射；不同设备/用户 profile 的分配值不构成契约），
/// 解析失败回退 A16/A17 观测值。
fn media_provider_uid() -> u32 {
    static MEDIA_PROVIDER_UID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *MEDIA_PROVIDER_UID.get_or_init(|| {
        let parsed = std::fs::read_to_string("/data/system/packages.list")
            .ok()
            .and_then(|content| {
                content.lines().find_map(|line| {
                    let mut fields = line.split_whitespace();
                    if fields.next() == Some("com.android.providers.media.module") {
                        fields.next().and_then(|value| value.parse::<i32>().ok())
                    } else {
                        None
                    }
                })
            })
            .filter(|uid| *uid >= 10000);
        parsed.map(|uid| uid as u32).unwrap_or(10217)
    })
}

/// 宿主共享会话的「系统读取者」白名单：MediaProvider 应用 uid 与 media/media_rw
/// 系统身份。这些调用方按路径访问应用重定向树是系统扫描/元数据协商的正常路径，
/// 不属于 scoped storage 越权；其余应用 uid 一律维持 deny_all。
fn is_system_reader_uid(uid: u32) -> bool {
    uid == media_provider_uid() || uid == 1013 || uid == 1023
}

impl FuseRedirectFs {
    /// 系统读取者的路径解析（miss 重试语义）：先用请求者自身策略解析——MP 在宿主
    /// 共享会话有已登记策略（公共视图），命中即维持原语义；解析失败且请求者为
    /// 白名单 uid（MP/媒体身份）时，改用目标应用的属主策略重试——文件在被重定向
    /// 的后端落点而公共视图不存在，正是本轮 QQ 相机 fd 协商 ENOENT 的路径侧同源
    /// 场景。仅 Read 类，仅 `Android/data/<pkg>/` 前缀且该应用已登记；写路径与
    /// 其它应用 uid 一律维持原解析结果（宿主拒绝未登记调用方写入的沙盒设计）。
    fn backend_for_relative_with_system_reader(
        &self,
        uid: u32,
        policy: &Arc<RedirectPolicy>,
        rel: &str,
    ) -> Option<(Arc<RedirectPolicy>, BackendPath)> {
        if let Some(backend) = policy.backend_for_relative(rel, OperationKind::Read) {
            return Some((Arc::clone(policy), backend));
        }
        let owner_policy = self.system_reader_policy_on_miss(uid, policy, rel)?;
        let backend = owner_policy.backend_for_relative(rel, OperationKind::Read)?;
        Some((owner_policy, backend))
    }

    /// 请求者策略解析 miss 时的系统读取者重试：命中则返回属主策略，调用方以它
    /// 重新执行原解析并沿同一视角完成 attr 塑形与应答；未命中返回 None 维持原结果。
    fn system_reader_policy_on_miss(
        &self,
        uid: u32,
        _policy: &Arc<RedirectPolicy>,
        rel: &str,
    ) -> Option<Arc<RedirectPolicy>> {
        if !is_system_reader_uid(uid) {
            return None;
        }
        let package_name = rel
            .strip_prefix("Android/data/")
            .and_then(|rest| rest.split('/').next())
            .filter(|name| !name.is_empty())?;
        let owner_policy = self.policy.find_owner_policy_by_package(package_name)?;
        owner_policy.backend_for_relative(rel, OperationKind::Read)?;
        static SYSTEM_READER_COUNT: AtomicU64 = AtomicU64::new(0);
        let seen = SYSTEM_READER_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if seen == 1 || seen.is_multiple_of(256) {
            log::debug!(
                "fuse system-reader resolve uid={} pkg={} rel={} count={}",
                uid,
                package_name,
                rel,
                seen
            );
        }
        Some(owner_policy)
    }
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
            writeback_enabled: AtomicBool::new(false),
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

    /// 文件 open/create 应答的 FOPEN 标志：writeback 回写模式下必须携带
    /// FOPEN_KEEP_CACHE。缺失时内核每次 open 都失效该 inode 的页缓存，第二个
    /// fd 读「正在写的文件」会经 GETATTR 取到回刷滞后的后端 size（截断视图），
    /// 例如录制中 I 帧提取管线打开 growing mp4 直接判坏。与 passthrough open
    /// 应答（恒带 KEEP_CACHE）保持一致。
    pub(super) fn file_open_flags(&self) -> FopenFlags {
        if self.writeback_enabled.load(Ordering::Relaxed) {
            FopenFlags::FOPEN_KEEP_CACHE
        } else {
            FopenFlags::empty()
        }
    }

    fn policy_for_request(&self, req: &Request) -> Arc<RedirectPolicy> {
        self.policy.for_request(req.uid(), req.pid())
    }

    /// 系统 uid 请求者的监视归因（仅监视开启且走系统回退策略时解析）。
    ///
    /// daemon 以 root 运行，直接读 `/proc/<pid>` 反查发起者（MP 域的 SELinux 限制
    /// 不存在），身份可信度由 status 的 Uid 行裁决；pid 是发起任务的强信号，短
    /// TTL 缓存吸收 write 等高频路径的重复解析。非系统 uid 或未走系统回退策略
    /// （应用 uid 命中自身策略）时返回 None，记录维持会话默认形态。
    fn requester_attribution(
        &self,
        req: &Request,
        policy: &RedirectPolicy,
    ) -> Option<RequesterIdentity> {
        if !policy.is_file_monitor_enabled || !policy.is_system_fallback {
            return None;
        }
        let uid = req.uid() as i32;
        if uid <= 0 || uid >= ANDROID_APP_UID_START {
            return None;
        }
        let pid = req.pid() as i32;
        if pid <= 0 {
            return None;
        }
        let now_ms = crate::platform::paths::monotonic_ms();
        if let Ok(cache) = SYSTEM_REQUESTER_CACHE.lock()
            && let Some((at_ms, identity)) = cache.get(&pid)
            && now_ms.saturating_sub(*at_ms) <= SYSTEM_REQUESTER_CACHE_TTL_MS
        {
            return match identity {
                Some(cached) if cached.uid == uid => Some(cached.clone()),
                _ => None,
            };
        }
        let identity = crate::platform::proc_identity::read_proc_identity(pid)
            .filter(|(proc_uid, _)| *proc_uid == uid)
            .map(|(proc_uid, package)| RequesterIdentity {
                package,
                uid: proc_uid,
            });
        if let Ok(mut cache) = SYSTEM_REQUESTER_CACHE.lock() {
            if cache.len() >= SYSTEM_REQUESTER_CACHE_CAP {
                cache.clear();
            }
            cache.insert(pid, (now_ms, identity.clone()));
        }
        identity
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

    /// fstat 已打开 fd（GETATTR 带 fh）的属主策略回退：fd 可经 binder 跨进程复制
    /// （如 QQ 录制结束后把视频 fd 交给 MediaProvider 做原始格式协商，MP 进程对
    /// 该 fd fstat 时 FUSE GETATTR 以 MP 的 uid 到达 daemon）。fd 的文件身份在
    /// open 时刻已固定，不能按请求者 uid 重新路由 rel 路径——非属主解析必然
    /// ENOENT。属主与请求者一致或属主策略已注销时返回 None，沿用调用方策略。
    fn inode_owner_policy_for_fd_stat(
        &self,
        ino: INodeNo,
        requester_uid: u32,
    ) -> Option<Arc<RedirectPolicy>> {
        let owner_uid = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            state.inode_policy_uids.get(&ino.0).copied()?
        };
        let owner_policy = self.policy.for_uid_exact(owner_uid)?;
        if owner_policy.uid as u32 == requester_uid {
            return None;
        }
        log::debug!(
            "fuse getattr fd-owner fallback ino={:x} requester_uid={} owner_uid={}",
            ino.0,
            requester_uid,
            owner_uid
        );
        Some(owner_policy)
    }

    fn remember_policy_for_inode_locked(
        state: &mut FuseState,
        ino: INodeNo,
        rel: &str,
        policy: &RedirectPolicy,
    ) {
        // 跨进程 fstat 回退以「应用属主」为唯一可信登记：系统身份（MP/media/root）
        // 的解析晚到、失败（attr ENOENT 时 remember 已先行执行）或视图不同，都不得
        // 覆盖已登记的应用属主；应用属主之间先到者保留。
        let uid = policy.uid as u32;
        let is_app_uid = uid >= 10000;
        if let Some(&existing) = state.inode_policy_uids.get(&ino.0)
            && (!is_app_uid || (existing >= 10000 && existing != uid))
        {
            return;
        }
        if let Some(&existing) = state.path_policy_uids.get(rel)
            && (!is_app_uid || (existing >= 10000 && existing != uid))
        {
            return;
        }
        state.path_policy_uids.insert(rel.to_string(), uid);
        state.inode_policy_uids.insert(ino.0, uid);
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

    /// xattr 系 op 的公共后端路由：错误统一走应答错误观测，成功后以
    /// backend 的 NUL 结尾路径执行动作。
    fn with_backend_for_xattr(
        &self,
        policy: &RedirectPolicy,
        ino: INodeNo,
        op: &'static str,
        action: impl FnOnce(&std::ffi::CString) -> Result<callbacks::XattrOutcome, Errno>,
    ) -> Result<callbacks::XattrOutcome, Errno> {
        let backend = self.backend_for_ino(policy, ino).inspect_err(|&errno| {
            let rel = self.path_for_ino(ino);
            self.log_reply_error(op, policy.uid as u32, rel.as_deref(), errno);
        })?;
        let c_path = metadata::cstring_path(&backend.path)?;
        action(&c_path)
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
        let mut backend = self.backend_for_relative(&policy, &rel, OperationKind::Write)?;
        // HyperOS 下载 bind 在任务入队瞬间创建，快照 TTL 可能刚好错过；新建入口
        // 强制重读 init mountinfo，避免首次 CREATE 与 bind 创建竞态时落到被遮蔽的
        // 真实目录（真机 2026-10-19：商店增量下载校验 error=3 的直接成因）。
        if let Some(resolved) = backend_overlay::overlay_backend_path_fresh(&backend.path) {
            backend.path = resolved;
        }
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

    fn reply_entry_for_rel(
        &self,
        uid: u32,
        policy: &Arc<RedirectPolicy>,
        rel: &str,
        reply: ReplyEntry,
    ) {
        // lookup 是系统读取者路径反解的主入口：MP 对重定向树的路径访问先经此建立 dentry。
        let Some((policy, backend)) =
            self.backend_for_relative_with_system_reader(uid, policy, rel)
        else {
            // 双重 miss（请求者视图 + 系统读取者重试）：ENOENT 归因由
            // log_reply_error 承担，log_new_entry_errno 的 create 诊断不适用。
            self.log_reply_error("lookup", uid, Some(rel), Errno::ENOENT);
            reply.error(Errno::ENOENT);
            return;
        };
        let policy = &policy;
        let ino = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, rel);
            Self::remember_policy_for_inode_locked(&mut state, ino, rel, policy);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.visible_attr_for_backend(policy, ino, &backend) {
            Ok(attr) => reply.entry(&ENTRY_TTL, &attr, Generation(0)),
            Err(errno) => {
                self.log_reply_error("lookup", policy.uid as u32, Some(rel), errno);
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
        let parent_path = Path::new(parent.as_ref());
        let parent_overlaid = backend_overlay::overlay_backend_path(parent_path).is_some();
        let existed = fs::is_directory(&parent);
        if existed || fs::create_directory(&parent, owner_uid) {
            // 覆盖挂载目标（如 HyperOS apk-tmp 下载目录）由系统按下载方身份自管
            // 属主与权限，既有层级不得改写；仅本会话新建的层级按调用方身份修复。
            if !existed || !parent_overlaid {
                fix_path_metadata(
                    parent_path,
                    policy.uid,
                    MAPPED_DIR_MODE,
                    backend.is_shared_public_backend,
                    true,
                );
            }
            Ok(())
        } else {
            Err(Errno::EIO)
        }
    }

    fn invalidate_dir_candidate_cache(&self) {
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        state.clear_dir_candidate_cache();
    }

    /// open 失败回包留痕：EIO 这类「不该出现」的 errno 是宿主数据面故障的唯一直接
    /// 痕迹（实测反馈设备 QQ 打开 chatpic 缩略图收到 EIO，且无其它任何日志）。
    /// ENOENT 是常规探测、EACCES/EROFS 已有监视台账单独记录，均不重复留痕。
    /// W 级日志不受 verbose 门禁，宿主进程必然落入 running.log。
    /// create/mkdir 等新建入口的异常 errno 诊断：写路径 O_CREAT 走 create 回调而非 open，
    /// 不补这个点则「未登记 uid / 旧映射残留」的 ENOENT 与旧只读策略的 EROFS 全部无痕。
    /// xattr 系 op 的策略解析：与 getattr 一致的属主回退——跨进程 fd 的
    /// getxattr/setxattr 同样不能按请求者 uid 重路由 rel 路径。
    fn policy_for_fd_stat_or_request(&self, req: &Request, ino: INodeNo) -> Arc<RedirectPolicy> {
        let policy = self.policy_for_read_request(req, Some(ino), None);
        if let Some(owner_policy) = self.inode_owner_policy_for_fd_stat(ino, req.uid()) {
            return owner_policy;
        }
        policy
    }

    /// 统一应答错误观测。ENOENT/EACCES 是热路径正常语义，仅 debug 日志开启时
    /// 记录——此前 QQ 相机失败链中 MP 侧 getattr ENOENT 因宿主应答完全无观测
    /// 成为盲区，这次补上。其余错误罕见且排查价值高，warn 级采样（前 16 次全
    /// 记，之后每 256 次记一次）兼顾证据留存与模块日志 20s 轮转的限流约束。
    fn log_reply_error(&self, op: &'static str, uid: u32, rel: Option<&str>, errno: Errno) {
        let errno_value = i32::from(errno);
        if matches!(errno_value, libc::ENOENT | libc::EACCES) {
            if crate::logging::is_debug_logging_enabled() {
                log::debug!("fuse {op} error uid={uid} rel={rel:?} errno={errno_value}");
            }
            return;
        }
        static UNEXPECTED_COUNT: AtomicU64 = AtomicU64::new(0);
        let seen = UNEXPECTED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if seen > 16 && !seen.is_multiple_of(256) {
            return;
        }
        log::warn!("fuse {op} error uid={uid} rel={rel:?} errno={errno_value} seen={seen}");
    }

    fn log_new_entry_errno(policy: &RedirectPolicy, name: &OsStr, errno: Errno) {
        let errno_value = i32::from(errno);
        if !matches!(errno_value, libc::ENOENT | libc::EACCES | libc::EROFS) {
            return;
        }
        if errno_value == libc::EROFS {
            log::warn!(
                "fuse create read-only deny pkg={} name={:?} errno={} policy_fp={:016x} monitor_only={}",
                policy.package_name,
                name,
                errno_value,
                policy.fingerprint,
                policy.is_monitor_only
            );
            return;
        }
        let seen = CREATE_ENOENT_DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
        if seen >= 4 {
            return;
        }
        log::warn!(
            "fuse create enoent diag pkg={} name={:?} errno={} policy_fp={:016x} deny_all={} monitor_only={} seen={}",
            policy.package_name,
            name,
            errno_value,
            policy.fingerprint,
            policy.is_deny_all(),
            policy.is_monitor_only,
            seen + 1
        );
    }

    fn log_unexpected_open_errno(policy: &RedirectPolicy, rel: &str, errno: Errno) {
        let errno_value = i32::from(errno);
        if errno_value == libc::EROFS {
            // 只读拒绝在正常场景只来自 read_only_paths；若出现的路径不在新配置的只读表里，
            // 这个指纹就是「宿主仍持旧策略」的直接证据（全量场景跑的确定性 EROFS 排查锚点）。
            log::warn!(
                "fuse open read-only deny pkg={} rel={} errno={} policy_fp={:016x} monitor_only={}",
                policy.package_name,
                rel,
                errno_value,
                policy.fingerprint,
                policy.is_monitor_only
            );
            return;
        }
        if matches!(errno_value, libc::ENOENT | libc::EACCES) {
            // ENOENT 是 deny_all/旧映射残留的主要表象，量可能很大：会话级限 4 条，
            // 带指纹即可判定「未登记 uid」与「旧映射残留」。
            let seen = OPEN_ENOENT_DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
            if seen >= 4 {
                return;
            }
            log::warn!(
                "fuse open enoent diag pkg={} rel={} errno={} policy_fp={:016x} deny_all={} monitor_only={} seen={}",
                policy.package_name,
                rel,
                errno_value,
                policy.fingerprint,
                policy.is_deny_all(),
                policy.is_monitor_only,
                seen + 1
            );
            return;
        }
        log::warn!(
            "fuse open unexpected errno pkg={} rel={} errno={}",
            policy.package_name,
            rel,
            errno_value
        );
    }

    fn open_backend_file(path: &Path, flags: i32, mode: u32) -> Result<File, Errno> {
        // FUSE writeback 回写模式下的部分页写，内核要先读原页（read-modify-write），
        // 该读仍由本 daemon 应答。backend 若照搬应用的 O_WRONLY 打开，pread 会返回
        // EBADF，内核 write_begin 失败后在应用侧呈现为诡异的 pwrite 短写（夸克下载
        // 引擎的 TNet 日志与下载目标都是 O_WRONLY，短写让其进入重建文件死循环）。
        // daemon 以自身权限打开 backend，写意图一律升级为 O_RDWR；O_PATH 打开保持
        // 原样（其 fd 不可读写是内核语义，升级反而破坏路径探测）。
        let mut backend_flags = flags | libc::O_CLOEXEC;
        if backend_flags & libc::O_PATH == 0 && backend_flags & libc::O_ACCMODE != libc::O_RDONLY {
            backend_flags = (backend_flags & !libc::O_ACCMODE) | libc::O_RDWR;
        }
        let c_path = cstring_path(path)?;
        // SAFETY: c_path 是合法的 NUL 结尾字符串，backend_flags/mode 为整数参数，
        // open 只在失败时返回 -1，成功返回的 fd 由 File 接管所有权。
        let fd = unsafe { libc::open(c_path.as_ptr(), backend_flags, mode) };
        if fd < 0 {
            Err(errno_from_code(last_errno()))
        } else {
            // SAFETY: fd 刚由 open 成功返回且未被其它对象接管，转移给 File 独占管理。
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    fn remove_child(
        &self,
        policy: &RedirectPolicy,
        parent: INodeNo,
        name: &OsStr,
        is_dir: bool,
        caller: Option<&RequesterIdentity>,
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
                policy.emit_monitor_mutation(
                    if is_dir { "rmdir" } else { "unlink" },
                    &backend,
                    None,
                    caller,
                );
                reply.ok();
            }
            Err(error) => reply.error(errno_from_io(error)),
        }
    }
}
