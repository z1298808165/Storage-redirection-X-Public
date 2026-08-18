mod config;
mod inode;
mod metadata;
mod perf;
mod policy;

pub use config::{
    FuseRedirectConfig, MountRequestFields, fuse_config_from_request, mount_blocking_with_ready,
    scoped_fuse_mount_roots_for_request,
};

use crate::platform::{fs, paths};
use fuser::{
    AccessFlags, CopyFileRangeFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, InitFlags, KernelConfig, LockOwner, OpenAccMode, OpenFlags, RenameFlags,
    ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyDirectoryPlus, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow, WriteFlags,
};
use inode::{
    add_dir_entry_refs, remap_inode_path, remove_dir_entry_refs, remove_inode_path,
    remove_unreferenced_inode,
};
use metadata::{
    adjust_metadata_mode, chmod_path, chown_path, cstring_path, errno_from_code, errno_from_io,
    fix_existing_path_metadata, fix_path_metadata, last_errno, rename_noreplace, truncate_path,
    utimens_path,
};
use perf::{DirectoryCacheMissReason, FusePerfStats};
use policy::{BackendPath, OperationKind, RedirectPolicy};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_millis(250);
const ROOT_INO: u64 = 1;
const MAX_READ_SIZE: usize = 256 * 1024;
const DIR_CANDIDATE_CACHE_TTL: Duration = Duration::from_millis(250);
const MAX_DIR_CANDIDATE_CACHE_ENTRIES: usize = 64;
const MEDIA_RW_UID: u32 = 1023;
pub(super) const MEDIA_RW_GID: u32 = 1023;
pub(super) const MAPPED_DIR_MODE: libc::mode_t = 0o2773;
const SHARED_PUBLIC_DIR_MODE: u32 = 0o2770;
pub(super) const MAX_SCOPED_FUSE_ROOTS: usize = 4;

thread_local! {
    // FUSE 读回调通常在固定工作线程上重复执行，复用 256 KiB 内的缓冲区减少分配。
    static FUSE_READ_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

struct FuseRedirectFs {
    policy: RedirectPolicy,
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
    inode_path_versions: HashMap<u64, u64>,
    lookup_counts: HashMap<u64, u64>,
    dir_entry_refs: HashMap<u64, u64>,
    files: HashMap<u64, OpenFile>,
    dirs: HashMap<u64, Arc<[DirEntry]>>,
    dir_candidate_cache: HashMap<String, CachedDirCandidates>,
}

impl FuseState {
    fn next_handle(&mut self) -> u64 {
        let fh = self.next_fh;
        self.next_fh = self.next_fh.saturating_add(1).max(1);
        fh
    }
}

struct OpenFile {
    // quality-allow(lint-suppression): rel字段保留供调试和诊断输出使用，当前未被读取但不应删除。
    #[allow(dead_code)]
    rel: String,
    file: Option<Arc<File>>,
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
struct DirEntryCandidate {
    rel: String,
    kind: FileType,
    name: String,
}

#[derive(Clone)]
struct DirectorySourceSignature {
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
    candidates: Vec<DirEntryCandidate>,
    sources: Vec<DirectorySourceSignature>,
}

impl FuseRedirectFs {
    fn new(config: FuseRedirectConfig) -> Option<Self> {
        let package_name = config.package_name.clone();
        let policy = RedirectPolicy::new(config)?;
        let mut inodes = HashMap::new();
        let mut paths_by_inode = HashMap::new();
        inodes.insert(String::new(), ROOT_INO);
        paths_by_inode.insert(ROOT_INO, String::new());

        Some(Self {
            policy,
            perf: FusePerfStats::new(package_name),
            passthrough_enabled: AtomicBool::new(false),
            state: RwLock::new(FuseState {
                next_ino: ROOT_INO + 1,
                next_fh: 1,
                inodes,
                paths_by_inode,
                inode_path_versions: HashMap::from([(ROOT_INO, 0)]),
                lookup_counts: HashMap::new(),
                dir_entry_refs: HashMap::new(),
                files: HashMap::new(),
                dirs: HashMap::new(),
                dir_candidate_cache: HashMap::new(),
            }),
        })
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

    fn backend_for_ino(&self, ino: INodeNo) -> Result<BackendPath, Errno> {
        let rel = self.path_for_ino(ino).ok_or(Errno::ENOENT)?;
        self.policy
            .backend_for_relative(&rel, OperationKind::Read)
            .ok_or(Errno::ENOENT)
    }

    fn backend_for_relative(
        &self,
        rel: &str,
        operation: OperationKind,
    ) -> Result<BackendPath, Errno> {
        self.policy
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

    fn attr_for_backend(&self, ino: INodeNo, backend: &BackendPath) -> Result<FileAttr, Errno> {
        let metadata = std::fs::symlink_metadata(&backend.path).map_err(errno_from_io)?;
        let mut attr = file_attr_from_metadata(ino, metadata);
        if backend.is_shared_public_backend {
            attr.uid = self.policy.uid as u32;
            attr.gid = MEDIA_RW_GID;
            if attr.kind == FileType::Directory {
                attr.perm = SHARED_PUBLIC_DIR_MODE as u16;
            }
        }
        Ok(attr)
    }

    fn visible_attr_for_backend(
        &self,
        ino: INodeNo,
        backend: &BackendPath,
    ) -> Result<FileAttr, Errno> {
        match self.attr_for_backend(ino, backend) {
            Ok(attr) => Ok(attr),
            Err(errno)
                if errno.code() == libc::ENOENT && self.policy.is_virtual_dir(&backend.rel) =>
            {
                Ok(synthetic_dir_attr(
                    ino,
                    self.policy.uid as u32,
                    MEDIA_RW_GID,
                ))
            }
            Err(errno) => Err(errno),
        }
    }

    fn reply_entry_for_rel(&self, rel: &str, reply: ReplyEntry) {
        let Some(backend) = self.policy.backend_for_relative(rel, OperationKind::Read) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let ino = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, rel);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.visible_attr_for_backend(ino, &backend) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(errno) => {
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                Self::remove_lookup_locked(&mut state, ino, 1);
                reply.error(errno);
            }
        }
    }

    fn ensure_parent_for_backend(&self, backend: &BackendPath) -> Result<(), Errno> {
        let Some(parent) = backend.path.parent() else {
            return Ok(());
        };
        let parent = parent.to_string_lossy();
        if parent.is_empty() {
            return Ok(());
        }
        let owner_uid = if backend.is_shared_public_backend {
            MEDIA_RW_UID as i32
        } else {
            self.policy.uid
        };
        if fs::is_directory(&parent) || fs::create_directory(&parent, owner_uid) {
            fix_path_metadata(
                Path::new(parent.as_ref()),
                self.policy.uid,
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
        state.dir_candidate_cache.clear();
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

    fn remove_child(&self, parent: INodeNo, name: &OsStr, is_dir: bool, reply: ReplyEmpty) {
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
        let Some(backend) = self.policy.backend_for_relative(&rel, OperationKind::Read) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if backend.is_read_only {
            self.policy
                .emit_monitor_read_only_deny(if is_dir { "rmdir" } else { "unlink" }, &backend);
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
                state.dir_candidate_cache.clear();
                reply.ok();
            }
            Err(error) => reply.error(errno_from_io(error)),
        }
    }
}

impl Filesystem for FuseRedirectFs {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        let passthrough_supported = config.capabilities().contains(InitFlags::FUSE_PASSTHROUGH);
        let passthrough_enabled = config.add_capabilities(InitFlags::FUSE_PASSTHROUGH).is_ok();
        let stack_depth_enabled = config.set_max_stack_depth(2).is_ok();
        let max_background_enabled = config.set_max_background(32).is_ok();
        let congestion_enabled = config.set_congestion_threshold(24).is_ok();
        let max_write_enabled = config.set_max_write(1024 * 1024).is_ok();
        self.passthrough_enabled
            .store(passthrough_enabled, Ordering::Relaxed);
        log::info!(
            "fuse init pkg={} kernel_abi={} passthrough_supported={} passthrough_enabled={} stack_depth={} max_background={} congestion={} max_write={}",
            self.policy.package_name,
            config.kernel_abi(),
            passthrough_supported,
            passthrough_enabled,
            stack_depth_enabled,
            max_background_enabled,
            congestion_enabled,
            max_write_enabled
        );
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let _perf = self.perf.observe(&self.perf.lookup_calls);
        let Some(parent_rel) = self.path_for_ino(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match Self::child_rel(&parent_rel, name) {
            Ok(rel) => self.reply_entry_for_rel(&rel, reply),
            Err(errno) => reply.error(errno),
        }
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        if ino.0 == ROOT_INO {
            return;
        }
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        Self::remove_lookup_locked(&mut state, ino, nlookup);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let _perf = self.perf.observe(&self.perf.metadata_calls);
        match self
            .backend_for_ino(ino)
            .and_then(|backend| self.visible_attr_for_backend(ino, &backend))
        {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(errno) => reply.error(errno),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
        let _perf = self.perf.observe(&self.perf.metadata_calls);
        match self.backend_for_ino(ino).and_then(|backend| {
            std::fs::read_link(&backend.path)
                .map(|path| path.as_os_str().as_bytes().to_vec())
                .map_err(errno_from_io)
        }) {
            Ok(bytes) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn opendir(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let _perf = self.perf.observe(&self.perf.open_calls);
        let track_dir_perf = crate::logging::is_debug_logging_enabled();
        let first_lock_started = track_dir_perf.then(std::time::Instant::now);
        let (mut rel, mut path_version) = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(rel) = state.paths_by_inode.get(&ino.0).cloned() else {
                reply.error(Errno::ENOENT);
                return;
            };
            let version = state.inode_path_versions.get(&ino.0).copied().unwrap_or(0);
            (rel, version)
        };
        let mut lock_wait_ns = elapsed_ns(first_lock_started);
        let mut scan_ns = 0u64;
        let mut retries = 0u64;
        let (fh, entry_count) = loop {
            let backend = match self.policy.backend_for_relative(&rel, OperationKind::Read) {
                Some(backend) => backend,
                None => {
                    reply.error(Errno::ENOENT);
                    return;
                }
            };
            if !backend.path.is_dir() && !self.policy.is_virtual_dir(&rel) {
                reply.error(Errno::ENOTDIR);
                return;
            }
            let cache_lock_started = track_dir_perf.then(Instant::now);
            let cached = {
                let state = self.state.read().unwrap_or_else(|err| err.into_inner());
                state.dir_candidate_cache.get(&rel).map(|cached| {
                    (
                        cached.created_at,
                        cached.candidates.clone(),
                        cached.sources.clone(),
                    )
                })
            };
            lock_wait_ns = lock_wait_ns.saturating_add(elapsed_ns(cache_lock_started));
            let (cached_candidates, cache_miss_reason) = match cached {
                None => (None, DirectoryCacheMissReason::NotCached),
                Some((created_at, _candidates, _sources))
                    if created_at.elapsed() > DIR_CANDIDATE_CACHE_TTL =>
                {
                    (None, DirectoryCacheMissReason::TtlExpired)
                }
                Some((_, candidates, sources)) => {
                    let reason = sources.iter().find_map(|source| match source.state() {
                        DirectorySourceState::Current => None,
                        DirectorySourceState::Changed => {
                            Some(DirectoryCacheMissReason::SourceChanged)
                        }
                        DirectorySourceState::Missing => {
                            Some(DirectoryCacheMissReason::SourceMissing)
                        }
                    });
                    match reason {
                        Some(reason) => (None, reason),
                        None => (Some(candidates), DirectoryCacheMissReason::NotCached),
                    }
                }
            };
            let (candidates, sources, from_cache) = if let Some(candidates) = cached_candidates {
                self.perf.record_dir_cache_hit();
                (candidates, Vec::new(), true)
            } else {
                self.perf.record_dir_cache_miss(cache_miss_reason);
                let scan_started = track_dir_perf.then(Instant::now);
                let (candidates, sources) = collect_dir_entry_candidates(&self.policy, &rel);
                scan_ns = scan_ns.saturating_add(elapsed_ns(scan_started));
                (candidates, sources, false)
            };
            let lock_started = track_dir_perf.then(std::time::Instant::now);
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            lock_wait_ns = lock_wait_ns.saturating_add(elapsed_ns(lock_started));
            let Some(current_rel) = state.paths_by_inode.get(&ino.0) else {
                reply.error(Errno::ENOENT);
                return;
            };
            let current_version = state.inode_path_versions.get(&ino.0).copied().unwrap_or(0);
            if current_rel != &rel || current_version != path_version {
                rel = current_rel.clone();
                path_version = current_version;
                retries = retries.saturating_add(1);
                continue;
            }
            if !from_cache {
                if state.dir_candidate_cache.len() >= MAX_DIR_CANDIDATE_CACHE_ENTRIES {
                    self.perf.record_dir_cache_eviction();
                    state.dir_candidate_cache.clear();
                }
                state.dir_candidate_cache.insert(
                    rel.clone(),
                    CachedDirCandidates {
                        created_at: Instant::now(),
                        candidates: candidates.clone(),
                        sources,
                    },
                );
            }
            let fh = state.next_handle();
            let entry_count = candidates.len().saturating_add(2);
            let entries: Arc<[DirEntry]> =
                materialize_dir_entries(&mut state, ino, &rel, candidates).into();
            add_dir_entry_refs(&mut state, &entries);
            state.dirs.insert(fh, entries);
            break (fh, entry_count);
        };
        self.perf
            .record_dir_scan(scan_ns, lock_wait_ns, retries, entry_count);
        reply.opened(FileHandle(fh), FopenFlags::FOPEN_CACHE_DIR);
    }

    fn readdir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let _perf = self.perf.observe(&self.perf.read_calls);
        let handle = fh.into();
        let entries = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            state.dirs.get(&handle).cloned()
        };
        let Some(entries) = entries else {
            reply.error(Errno::EBADF);
            return;
        };
        for (index, entry) in entries.iter().enumerate().skip(offset as usize) {
            if reply.add(entry.ino, (index + 1) as u64, entry.kind, &entry.name) {
                break;
            }
        }
        reply.ok();
    }

    fn readdirplus(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectoryPlus,
    ) {
        let _perf = self.perf.observe(&self.perf.read_calls);
        let handle = fh.into();
        let entries = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            if !state.paths_by_inode.contains_key(&ino.0) {
                reply.error(Errno::ENOENT);
                return;
            }
            let Some(entries) = state.dirs.get(&handle).cloned() else {
                reply.error(Errno::EBADF);
                return;
            };
            entries
        };
        for (index, entry) in entries.iter().enumerate().skip(offset as usize) {
            let Some(backend) = self
                .policy
                .backend_for_relative(&entry.rel, OperationKind::Read)
            else {
                continue;
            };
            let Ok(attr) = self.visible_attr_for_backend(entry.ino, &backend) else {
                continue;
            };
            if reply.add(
                entry.ino,
                (index + 1) as u64,
                &entry.name,
                &TTL,
                &attr,
                Generation(0),
            ) {
                break;
            }
        }
        reply.ok();
    }

    fn releasedir(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        reply: ReplyEmpty,
    ) {
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        if let Some(entries) = state.dirs.remove(&fh.into()) {
            remove_dir_entry_refs(&mut state, &entries);
        }
        reply.ok();
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let _perf = self.perf.observe(&self.perf.open_calls);
        let backend = match self.backend_for_ino(ino) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only && open_flags_write(flags.0) {
            self.policy
                .emit_monitor_read_only_deny(fuse_open_operation_name(flags.0), &backend);
            reply.error(Errno::EROFS);
            return;
        }
        let mut open_flags = flags.0 | libc::O_CLOEXEC;
        open_flags &= !libc::O_CREAT;
        let file = match Self::open_backend_file(&backend.path, open_flags, 0) {
            Ok(file) => file,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let fh = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let fh = state.next_handle();
            state.files.insert(
                fh,
                OpenFile {
                    rel: backend.rel.clone(),
                    file: file.try_clone().ok().map(Arc::new),
                    is_read_only: backend.is_read_only,
                },
            );
            fh
        };
        if self.passthrough_enabled.load(Ordering::Relaxed) {
            match reply.open_backing(&file) {
                Ok(backing) => {
                    reply.opened_passthrough(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE, &backing)
                }
                Err(_) => reply.opened(FileHandle(fh), FopenFlags::empty()),
            }
        } else {
            reply.opened(FileHandle(fh), FopenFlags::empty());
        }
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyData,
    ) {
        let _perf = self.perf.observe(&self.perf.read_calls);
        let file = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(open_file) = state.files.get(&fh.into()) else {
                reply.error(Errno::EBADF);
                return;
            };
            let Some(file) = open_file.file.clone() else {
                reply.error(Errno::ENOSYS);
                return;
            };
            file
        };
        let requested = (size as usize).min(MAX_READ_SIZE);
        FUSE_READ_BUFFER.with(|buffer| {
            let mut buffer = buffer.borrow_mut();
            let reused = buffer.capacity() >= requested;
            buffer.resize(requested, 0);
            match file.read_at(&mut buffer, offset) {
                Ok(n) => {
                    self.perf.record_read_buffer(n, reused);
                    reply.data(&buffer[..n]);
                }
                Err(error) => reply.error(errno_from_io(error)),
            }
        });
    }

    fn write(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        reply: ReplyWrite,
    ) {
        let _perf = self.perf.observe(&self.perf.write_calls);
        let file = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(open_file) = state.files.get(&fh.into()) else {
                reply.error(Errno::EBADF);
                return;
            };
            if open_file.is_read_only {
                let rel = open_file.rel.clone();
                drop(state);
                if let Some(backend) = self.policy.backend_for_relative(&rel, OperationKind::Write)
                {
                    self.policy.emit_monitor_read_only_deny("write", &backend);
                }
                reply.error(Errno::EROFS);
                return;
            }
            let Some(file) = open_file.file.clone() else {
                drop(state);
                match self.backend_for_ino(ino) {
                    Ok(backend) if backend.is_read_only => {
                        self.policy.emit_monitor_read_only_deny("write", &backend);
                        reply.error(Errno::EROFS);
                    }
                    _ => reply.error(Errno::ENOSYS),
                }
                return;
            };
            file
        };
        match file.write_at(data, offset) {
            Ok(n) => reply.written(n as u32),
            Err(error) => reply.error(errno_from_io(error)),
        }
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<fuser::LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        state.files.remove(&fh.into());
        reply.ok();
    }

    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        if state.files.contains_key(&fh.into()) {
            reply.ok();
        } else {
            reply.error(Errno::EBADF);
        }
    }

    fn fsync(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        datasync: bool,
        reply: ReplyEmpty,
    ) {
        let file = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(open_file) = state.files.get(&fh.into()) else {
                reply.error(Errno::EBADF);
                return;
            };
            let Some(file) = open_file.file.clone() else {
                reply.error(Errno::ENOSYS);
                return;
            };
            file
        };
        let result = if datasync {
            file.sync_data()
        } else {
            file.sync_all()
        };
        match result {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(errno_from_io(error)),
        }
    }

    fn copy_file_range(
        &self,
        _req: &Request,
        _ino_in: INodeNo,
        fh_in: FileHandle,
        offset_in: u64,
        _ino_out: INodeNo,
        fh_out: FileHandle,
        offset_out: u64,
        len: u64,
        flags: CopyFileRangeFlags,
        reply: ReplyWrite,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        if !flags.is_empty() {
            reply.error(Errno::EINVAL);
            return;
        }
        let Some(mut input_offset) = libc::off_t::try_from(offset_in).ok() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let Some(mut output_offset) = libc::off_t::try_from(offset_out).ok() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let (input, output, output_read_only, output_rel) = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(input) = state.files.get(&fh_in.into()) else {
                reply.error(Errno::EBADF);
                return;
            };
            let Some(input_file) = input.file.clone() else {
                reply.error(Errno::ENOSYS);
                return;
            };
            let Some(output) = state.files.get(&fh_out.into()) else {
                reply.error(Errno::EBADF);
                return;
            };
            let Some(output_file) = output.file.clone() else {
                reply.error(Errno::ENOSYS);
                return;
            };
            (
                input_file,
                output_file,
                output.is_read_only,
                output.rel.clone(),
            )
        };
        if output_read_only {
            if let Some(backend) = self
                .policy
                .backend_for_relative(&output_rel, OperationKind::Write)
            {
                self.policy
                    .emit_monitor_read_only_deny("copy_file_range", &backend);
            }
            reply.error(Errno::EROFS);
            return;
        }
        let copy_len = len.min(usize::MAX as u64) as usize;
        // SAFETY: 两个文件描述符均来自状态表中的活动 Arc<File>，offset 指针指向本地可写值，长度和 flags 已完成边界校验。
        let copied = unsafe {
            libc::syscall(
                libc::SYS_copy_file_range,
                input.as_raw_fd(),
                &mut input_offset as *mut libc::off_t,
                output.as_raw_fd(),
                &mut output_offset as *mut libc::off_t,
                copy_len,
                0u32,
            )
        };
        if copied >= 0 {
            reply.written(copied as u32);
        } else {
            reply.error(errno_from_code(last_errno()));
        }
    }

    fn create(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
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
        let backend = match self.backend_for_relative(&rel, OperationKind::Write) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only {
            self.policy.emit_monitor_read_only_deny("create", &backend);
            reply.error(Errno::EROFS);
            return;
        }
        if let Err(errno) = self.ensure_parent_for_backend(&backend) {
            reply.error(errno);
            return;
        }
        let create_mode = mode & !umask;
        let file = match Self::open_backend_file(
            &backend.path,
            flags | libc::O_CREAT | libc::O_CLOEXEC,
            create_mode,
        ) {
            Ok(file) => file,
            Err(errno) => {
                log::warn!(
                    "fuse create backend open failed rel={} backend={} errno={:?}",
                    rel,
                    backend.path.display(),
                    errno
                );
                reply.error(errno);
                return;
            }
        };
        fix_path_metadata(
            &backend.path,
            self.policy.uid,
            create_mode,
            backend.is_shared_public_backend,
            false,
        );
        self.invalidate_dir_candidate_cache();
        let ino = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, &rel);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        let attr = match self.attr_for_backend(ino, &backend) {
            Ok(attr) => attr,
            Err(errno) => {
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                Self::remove_lookup_locked(&mut state, ino, 1);
                reply.error(errno);
                return;
            }
        };
        let fh = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let fh = state.next_handle();
            state.files.insert(
                fh,
                OpenFile {
                    rel,
                    file: file.try_clone().ok().map(Arc::new),
                    is_read_only: false,
                },
            );
            state.dir_candidate_cache.clear();
            fh
        };
        self.policy.emit_monitor_create(&backend);
        if self.passthrough_enabled.load(Ordering::Relaxed) {
            match reply.open_backing(&file) {
                Ok(backing) => reply.created_passthrough(
                    &TTL,
                    &attr,
                    Generation(0),
                    FileHandle(fh),
                    FopenFlags::empty(),
                    &backing,
                ),
                Err(_) => reply.created(
                    &TTL,
                    &attr,
                    Generation(0),
                    FileHandle(fh),
                    FopenFlags::empty(),
                ),
            }
        } else {
            reply.created(
                &TTL,
                &attr,
                Generation(0),
                FileHandle(fh),
                FopenFlags::empty(),
            );
        }
    }

    fn mknod(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let file_type = mode & libc::S_IFMT;
        if file_type != 0 && file_type != libc::S_IFREG {
            reply.error(Errno::EPERM);
            return;
        }
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
        let backend = match self.backend_for_relative(&rel, OperationKind::Write) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only {
            self.policy
                .emit_monitor_read_only_deny(stringify!(mknod), &backend);
            reply.error(Errno::EROFS);
            return;
        }
        if let Err(errno) = self.ensure_parent_for_backend(&backend) {
            reply.error(errno);
            return;
        }
        let create_mode = mode & !libc::S_IFMT & !umask;
        let file = match Self::open_backend_file(
            &backend.path,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
            create_mode,
        ) {
            Ok(file) => file,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        drop(file);
        fix_path_metadata(
            &backend.path,
            self.policy.uid,
            create_mode,
            backend.is_shared_public_backend,
            false,
        );
        self.invalidate_dir_candidate_cache();
        let ino = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, &rel);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.attr_for_backend(ino, &backend) {
            Ok(attr) => {
                self.policy.emit_monitor_create(&backend);
                reply.entry(&TTL, &attr, Generation(0));
            }
            Err(errno) => {
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                Self::remove_lookup_locked(&mut state, ino, 1);
                reply.error(errno);
            }
        }
    }

    fn mkdir(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
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
        let backend = match self.backend_for_relative(&rel, OperationKind::Write) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only {
            self.policy.emit_monitor_read_only_deny("mkdir", &backend);
            reply.error(Errno::EROFS);
            return;
        }
        if let Err(errno) = self.ensure_parent_for_backend(&backend) {
            reply.error(errno);
            return;
        }
        let mode = mode & !umask;
        match std::fs::create_dir(&backend.path) {
            Ok(()) => fix_path_metadata(
                &backend.path,
                self.policy.uid,
                mode,
                backend.is_shared_public_backend,
                true,
            ),
            Err(error) => {
                reply.error(errno_from_io(error));
                return;
            }
        }
        self.invalidate_dir_candidate_cache();
        self.policy.emit_monitor_create(&backend);
        self.reply_entry_for_rel(&rel, reply);
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        self.remove_child(parent, name, false, reply);
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        self.remove_child(parent, name, true, reply);
    }

    fn rename(
        &self,
        _req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let rename_flags = flags.bits();
        let rename_noreplace_flag = libc::RENAME_NOREPLACE as u32;
        if rename_flags & !rename_noreplace_flag != 0 {
            reply.error(Errno::EINVAL);
            return;
        }
        let Some(parent_rel) = self.path_for_ino(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let Some(new_parent_rel) = self.path_for_ino(newparent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let old_rel = match Self::child_rel(&parent_rel, name) {
            Ok(rel) => rel,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let new_rel = match Self::child_rel(&new_parent_rel, newname) {
            Ok(rel) => rel,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let old_backend = match self.backend_for_relative(&old_rel, OperationKind::Write) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let new_backend = match self.backend_for_relative(&new_rel, OperationKind::Write) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if old_backend.is_read_only || new_backend.is_read_only {
            let record_backend = if new_backend.is_read_only {
                &new_backend
            } else {
                &old_backend
            };
            self.policy.emit_monitor_read_only_deny_with_from(
                "rename",
                record_backend,
                Some(&old_backend),
                libc::EROFS,
            );
            reply.error(Errno::EROFS);
            return;
        }
        if let Err(errno) = self.ensure_parent_for_backend(&new_backend) {
            reply.error(errno);
            return;
        }
        let result = if rename_flags & rename_noreplace_flag != 0 {
            rename_noreplace(&old_backend.path, &new_backend.path)
        } else {
            std::fs::rename(&old_backend.path, &new_backend.path).map_err(errno_from_io)
        };
        match result {
            Ok(()) => {
                fix_existing_path_metadata(
                    &new_backend.path,
                    self.policy.uid,
                    new_backend.is_shared_public_backend,
                );
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                remap_inode_path(&mut state, &old_rel, &new_rel);
                state.dir_candidate_cache.clear();
                reply.ok();
            }
            Err(errno) => reply.error(errno),
        }
    }

    fn setattr(
        &self,
        _req: &Request,
        ino: INodeNo,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
        size: Option<u64>,
        atime: Option<TimeOrNow>,
        mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let backend = match self.backend_for_ino(ino) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only
            && (mode.is_some()
                || uid.is_some()
                || gid.is_some()
                || size.is_some()
                || atime.is_some()
                || mtime.is_some())
        {
            self.policy.emit_monitor_read_only_deny(
                fuse_setattr_operation_name(
                    mode.is_some(),
                    uid.is_some(),
                    gid.is_some(),
                    size.is_some(),
                    atime.is_some(),
                    mtime.is_some(),
                ),
                &backend,
            );
            reply.error(Errno::EROFS);
            return;
        }

        if let Some(mode) = mode {
            let mode = adjust_metadata_mode(
                mode,
                backend.is_shared_public_backend,
                backend.path.is_dir(),
            );
            if let Err(errno) = chmod_path(&backend.path, mode) {
                reply.error(errno);
                return;
            }
        }
        if uid.is_some() || gid.is_some() {
            let uid = if backend.is_shared_public_backend {
                MEDIA_RW_UID
            } else {
                uid.unwrap_or(u32::MAX)
            };
            let gid = if backend.is_shared_public_backend {
                MEDIA_RW_GID
            } else {
                gid.unwrap_or(u32::MAX)
            };
            if let Err(errno) = chown_path(&backend.path, uid, gid) {
                reply.error(errno);
                return;
            }
        }
        if let Some(size) = size
            && let Err(errno) = truncate_path(&backend.path, size)
        {
            reply.error(errno);
            return;
        }
        if (atime.is_some() || mtime.is_some())
            && let Err(errno) = utimens_path(&backend.path, atime, mtime)
        {
            reply.error(errno);
            return;
        }

        match self.attr_for_backend(ino, &backend) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(errno) => reply.error(errno),
        }
    }

    fn access(&self, _req: &Request, ino: INodeNo, mask: AccessFlags, reply: ReplyEmpty) {
        let backend = match self.backend_for_ino(ino) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only && mask.contains(AccessFlags::W_OK) {
            self.policy.emit_monitor_read_only_deny_with_errno(
                "access:write",
                &backend,
                libc::EACCES,
            );
            reply.error(Errno::EACCES);
            return;
        }
        let c_path = match cstring_path(&backend.path) {
            Ok(path) => path,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let ret = unsafe { libc::access(c_path.as_ptr(), mask.bits()) };
        if ret == 0 {
            reply.ok();
        } else {
            reply.error(errno_from_code(last_errno()));
        }
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let path = self.policy.real_root.as_path();
        let c_path = match cstring_path(path) {
            Ok(path) => path,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        let ret = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if ret != 0 {
            reply.error(errno_from_code(last_errno()));
            return;
        }
        let stat = unsafe { stat.assume_init() };
        reply.statfs(
            stat.f_blocks,
            stat.f_bfree,
            stat.f_bavail,
            stat.f_files,
            stat.f_ffree,
            stat.f_bsize as u32,
            255,
            stat.f_frsize as u32,
        );
    }

    fn fsyncdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            if !state.dirs.contains_key(&fh.into()) {
                reply.error(Errno::EBADF);
                return;
            }
        }
        let backend = match self.backend_for_ino(ino) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        match File::open(&backend.path).and_then(|file| file.sync_all()) {
            Ok(()) => reply.ok(),
            Err(error) => reply.error(errno_from_io(error)),
        }
    }
}

fn collect_dir_entry_candidates(
    policy: &policy::RedirectPolicy,
    rel: &str,
) -> (Vec<DirEntryCandidate>, Vec<DirectorySourceSignature>) {
    use crate::platform::paths;
    let mut entries = Vec::new();
    let mut seen = HashMap::<String, usize>::new();
    let mut sources = Vec::new();
    append_backend_dir_entries(
        policy,
        rel,
        &policy.redirect_backend_for_rel(rel),
        &mut entries,
        &mut seen,
        &mut sources,
    );
    append_backend_dir_entries(
        policy,
        rel,
        &policy.real_backend_for_rel(rel),
        &mut entries,
        &mut seen,
        &mut sources,
    );
    for mapping in &policy.path_mappings {
        if paths::matches(
            &mapping.request_path,
            &policy.storage_path_for_rel(rel),
            true,
        ) && let Some(target_rel) =
            paths::relative_child_path(&mapping.final_path, &policy.storage_root)
        {
            append_backend_dir_entries(
                policy,
                rel,
                &policy.real_backend_for_storage_rel(target_rel),
                &mut entries,
                &mut seen,
                &mut sources,
            );
        }
    }
    append_rule_prefix_entries(policy, rel, &mut entries, &mut seen);
    (entries, sources)
}

fn append_directory_source(path: &Path, sources: &mut Vec<DirectorySourceSignature>) {
    if sources.iter().any(|source| source.path == path) {
        return;
    }
    sources.push(DirectorySourceSignature::capture(path));
}

fn append_backend_dir_entries(
    policy: &policy::RedirectPolicy,
    parent_rel: &str,
    backend: &Path,
    entries: &mut Vec<DirEntryCandidate>,
    seen: &mut HashMap<String, usize>,
    sources: &mut Vec<DirectorySourceSignature>,
) {
    use crate::platform::paths;
    append_directory_source(backend, sources);
    let Ok(read_dir) = std::fs::read_dir(backend) else {
        return;
    };
    for item in read_dir.flatten() {
        let name = item.file_name().to_string_lossy().to_string();
        if name.is_empty() {
            continue;
        }
        let child_rel = if parent_rel.is_empty() {
            name.clone()
        } else {
            paths::join(parent_rel, &name)
        };
        let Some(child_backend) = policy.backend_for_relative(&child_rel, OperationKind::Read)
        else {
            continue;
        };
        // 该子项按策略解析出的后端必须就是当前枚举源这一侧，否则跳过，避免真实侧与
        // 重定向侧互相收录对方的条目。真实后端根按私有子树分流后，同一父目录下的子项
        // 可能落在两个真实根之一，故真实侧改用按 rel 判定而非单一路径相等比较。
        // 重定向根虽然位于 private_real_root 之下，但枚举项形如
        // <redirect_root>/<rel>，与真实侧的 <真实根>/<rel> 只有在 rel 自我嵌套时才
        // 可能相等，因此这条放宽不会让重定向侧条目被误判为真实侧后端。
        let is_expected_backend = paths_eq(&item.path(), &child_backend.path)
            || policy.is_real_backend_path_for_storage_rel(
                &policy.full_storage_rel(&child_rel),
                &item.path(),
            );
        if !is_expected_backend && !policy.is_virtual_dir(&child_rel) {
            continue;
        }
        let kind = item
            .file_type()
            .map(file_type_from_std)
            .unwrap_or(FileType::RegularFile);
        insert_dir_entry_candidate(entries, seen, child_rel, name, kind);
    }
}

fn append_rule_prefix_entries(
    policy: &policy::RedirectPolicy,
    parent_rel: &str,
    entries: &mut Vec<DirEntryCandidate>,
    seen: &mut HashMap<String, usize>,
) {
    use crate::platform::paths;
    for prefix in policy.rule_prefixes.iter() {
        let Some(child_name) = policy::visible_prefix_child(parent_rel, prefix) else {
            continue;
        };
        let child_rel = if parent_rel.is_empty() {
            child_name.clone()
        } else {
            paths::join(parent_rel, &child_name)
        };
        insert_dir_entry_candidate(entries, seen, child_rel, child_name, FileType::Directory);
    }
}

fn insert_dir_entry_candidate(
    entries: &mut Vec<DirEntryCandidate>,
    seen: &mut HashMap<String, usize>,
    child_rel: String,
    name: String,
    kind: FileType,
) {
    let key = name.to_ascii_lowercase();
    if let Some(index) = seen.get(&key).copied() {
        if entries[index].kind != FileType::Directory && kind == FileType::Directory {
            entries[index].kind = kind;
        }
        return;
    }
    let index = entries.len();
    entries.push(DirEntryCandidate {
        rel: child_rel,
        kind,
        name,
    });
    seen.insert(key, index);
}

fn materialize_dir_entries(
    state: &mut FuseState,
    ino: INodeNo,
    rel: &str,
    candidates: Vec<DirEntryCandidate>,
) -> Vec<DirEntry> {
    let parent_rel = paths::parent(rel);
    let parent_ino = if rel.is_empty() {
        ROOT_INO
    } else {
        // quality-allow(chinese-language): ROOT_INO 是 FUSE 根 inode 的固定技术标识。
        *state.inodes.get(&parent_rel).unwrap_or(&ROOT_INO)
    };
    let mut entries = Vec::with_capacity(candidates.len().saturating_add(2));
    entries.push(DirEntry {
        ino,
        kind: FileType::Directory,
        name: ".".to_string(),
        rel: rel.to_string(),
    });
    entries.push(DirEntry {
        ino: INodeNo(parent_ino),
        kind: FileType::Directory,
        name: "..".to_string(),
        rel: parent_rel.clone(),
    });
    entries.extend(candidates.into_iter().map(|candidate| {
        let ino = FuseRedirectFs::ino_for_path_locked(state, &candidate.rel);
        DirEntry {
            ino,
            kind: candidate.kind,
            name: candidate.name,
            rel: candidate.rel,
        }
    }));
    entries
}

fn file_type_from_std(file_type: std::fs::FileType) -> FileType {
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else {
        FileType::RegularFile
    }
}

fn file_type_from_mode(mode: u32) -> FileType {
    match mode & libc::S_IFMT {
        libc::S_IFDIR => FileType::Directory,
        libc::S_IFLNK => FileType::Symlink,
        libc::S_IFBLK => FileType::BlockDevice,
        libc::S_IFCHR => FileType::CharDevice,
        libc::S_IFIFO => FileType::NamedPipe,
        libc::S_IFSOCK => FileType::Socket,
        _ => FileType::RegularFile,
    }
}

fn file_attr_from_metadata(ino: INodeNo, metadata: std::fs::Metadata) -> FileAttr {
    use std::os::unix::fs::MetadataExt as _;
    FileAttr {
        ino,
        size: metadata.size(),
        blocks: metadata.blocks(),
        atime: unix_time(metadata.atime(), metadata.atime_nsec()),
        mtime: unix_time(metadata.mtime(), metadata.mtime_nsec()),
        ctime: unix_time(metadata.ctime(), metadata.ctime_nsec()),
        crtime: UNIX_EPOCH,
        kind: file_type_from_mode(metadata.mode()),
        perm: (metadata.mode() & 0o7777) as u16,
        nlink: metadata.nlink() as u32,
        uid: metadata.uid(),
        gid: metadata.gid(),
        rdev: metadata.rdev() as u32,
        flags: 0,
        blksize: metadata.blksize() as u32,
    }
}

fn synthetic_dir_attr(ino: INodeNo, uid: u32, gid: u32) -> FileAttr {
    let now = SystemTime::now();
    FileAttr {
        ino,
        size: 0,
        blocks: 0,
        atime: now,
        mtime: now,
        ctime: now,
        crtime: UNIX_EPOCH,
        kind: FileType::Directory,
        perm: 0o2773,
        nlink: 2,
        uid,
        gid,
        rdev: 0,
        flags: 0,
        blksize: 4096,
    }
}

fn unix_time(sec: i64, nsec: i64) -> SystemTime {
    if sec < 0 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::new(sec as u64, nsec.max(0) as u32)
}

pub(super) fn normalize_rule_list(paths_in: Vec<String>, user_id: i32) -> Vec<String> {
    use crate::platform::paths;
    let mut out = Vec::with_capacity(paths_in.len());
    let storage_root = paths::storage_user_root_for_user(user_id);
    for path in paths_in {
        let path = path.trim_start();
        let (excluded, body) = if let Some(stripped) = path.strip_prefix('!') {
            (true, stripped.trim_start())
        } else {
            (false, path)
        };
        let mut resolved = paths::resolve_user_path(&paths::normalize(body), user_id);
        if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
            continue;
        }
        if !paths::is_absolute(&resolved) {
            resolved = paths::normalize(&paths::join(&storage_root, &resolved));
        }
        if paths::is_child(&resolved, &storage_root) {
            if excluded {
                out.push(format!("!{resolved}"));
            } else {
                out.push(resolved);
            }
        }
    }
    paths::sort_dedup_paths_case_insensitive(&mut out);
    out
}

fn open_flags_write(flags: i32) -> bool {
    let accmode = OpenFlags(flags).acc_mode();
    accmode == OpenAccMode::O_WRONLY || accmode == OpenAccMode::O_RDWR || flags & libc::O_TRUNC != 0
}

fn fuse_open_operation_name(flags: i32) -> &'static str {
    if open_flags_write(flags) {
        "open:write"
    } else {
        "open:read"
    }
}

fn fuse_setattr_operation_name(
    has_mode: bool,
    has_uid: bool,
    has_gid: bool,
    has_size: bool,
    has_atime: bool,
    has_mtime: bool,
) -> &'static str {
    if has_size {
        "truncate"
    } else if has_mode {
        "chmod"
    } else if has_uid || has_gid {
        "chown"
    } else if has_atime || has_mtime {
        "utimens"
    } else {
        "setattr"
    }
}

fn elapsed_ns(started: Option<std::time::Instant>) -> u64 {
    started
        .map(|value| value.elapsed().as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn paths_eq(left: &Path, right: &Path) -> bool {
    left == right
}
