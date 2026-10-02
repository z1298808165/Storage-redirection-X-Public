// FUSE 回调实现与目录项枚举。
//
// 这里只负责「内核请求进来之后怎么应答」：`Filesystem` trait 的每个回调，以及只服务
// readdir/readdirplus 的目录项收集。Fs 自身的状态、句柄与策略解析留在 mod.rs，
// 回调改动不会碰到状态结构。

use super::attrs::file_type_from_std;
use super::helpers::{
    elapsed_ns, fuse_open_operation_name, fuse_setattr_operation_name, open_flags_write, paths_eq,
};
use super::inode::{add_dir_entry_refs, remap_inode_path, remove_dir_entry_refs};
use super::metadata::{
    adjust_metadata_mode, chmod_path, chown_path, cstring_path, errno_from_code, errno_from_io,
    fix_existing_path_metadata, fix_path_metadata, last_errno, rename_noreplace, truncate_path,
    utimens_path,
};
use super::perf::DirectoryCacheMissReason;
use super::policy::OperationKind;
use super::{
    CachedDirCandidates, DIR_CANDIDATE_CACHE_TTL, DirEntry, DirEntryCandidate,
    DirectorySourceSignature, DirectorySourceState, FUSE_READ_BUFFER, FuseRedirectFs, FuseState,
    MAX_READ_SIZE, MEDIA_RW_GID, MEDIA_RW_UID, NewEntryRoute, OpenFile, ROOT_INO, TTL,
    estimate_cached_dir_candidates_bytes,
};
use crate::platform::paths;
use fuser::{
    AccessFlags, CopyFileRangeFlags, Errno, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, InitFlags, KernelConfig, LockOwner, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyDirectoryPlus, ReplyEmpty, ReplyEntry, ReplyOpen,
    ReplyStatfs, ReplyWrite, Request, TimeOrNow, WriteFlags,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Instant, SystemTime};

/// sqlite `-shm` 边车的最小合法尺寸（一个 wal-index 区域）。
///
/// 与 MediaProvider 侧 hook 的 `SQLITE_SHM_MIN_SIZE`（`hook/media_fuse.rs`）保持同一
/// 取值；本模块被 bin 经 `#[path]` 独立重编译且不包含 hook 模块，因此这里本地定义。
const SQLITE_SHM_MIN_SIZE_BYTES: u64 = 32 * 1024;

/// 处置被毒化的自有 media sqlite `-shm` 边车：删除后返回是否已删除。
///
/// `-shm` 的合法尺寸只有 0 与 ≥32KiB 两种；落在两者之间（真机实测：支付宝内
/// XRadiant 的 `XRadiant.db-shm` 变成 3 字节全零）即为毒化态——应用侧的 WAL 能力
/// 探针靠「ftruncate 到小尺寸观察大小是否变化」判定介质可用性，毒化后探针永远
/// 失败并自锁（“共享内存不可用”→降级 TRUNCATE 仍需 checkpoint→IOERR_SHMOPEN），
/// 且该状态无法被应用自己打破。
///
/// **检测必须直读 f2fs 真实路径**：毒化往往由绕过 media view 的写入造成（外部
/// 直写、MP 侧处理路径等），此时 media view 的 fuse 节点缓存仍持有陈旧尺寸，
/// 按 backend_path 判定会把毒化误读成健康（duchamp 18:00 实测）。删除则双管
/// 齐下：先经 media view 路径 unlink（让 MediaProvider 在自己的节点簿记里显式
/// 失效该名称，供管理器等经 MP FUSE 的读取方保持一致），再删 f2fs 真实文件兜底
/// （保证毒化数据消失）——与人工删除 shm 的治愈路径一致。对任何消费方都无损：
/// 文件内容本就是无效垃圾，SQLite 下次打开会从 `-wal` 重建 wal-index。
fn heal_poisoned_sqlite_shm_backend(
    user_id: i32,
    view_root: Option<&Path>,
    rel: &str,
    package_name: &str,
) -> bool {
    // f2fs 真实路径直读：/data/media/<user>/<rel> 与 rel 的目录结构一一对应。
    let real_path = std::path::PathBuf::from(format!("/data/media/{user_id}/{rel}"));
    let Ok(metadata) = std::fs::metadata(&real_path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    let size = metadata.len();
    if size == 0 || size >= SQLITE_SHM_MIN_SIZE_BYTES {
        return false;
    }

    // 删除顺序：先经 media view 路径 unlink——MediaProvider 会在自己的节点簿记里
    // 正确失效该名称并删除后端文件；若此步后 f2fs 真实文件仍存在（视图 unlink
    // 失败或视图未绑定），再以 root 直删兜底。顺序颠倒会让 MP 的缓存
    // 节点指向已消失的文件（幽灵节点），应用后续 O_CREAT 全部失败（18:36 实测）。
    let mut removed_view = false;
    let mut removed_real = false;
    if let Some(view_root) = view_root
        && let Ok(c_view) = cstring_path(&view_root.join(rel))
    {
        // SAFETY: c_view 以 NUL 结尾，unlinkat 调用期间保持有效，仅按路径读取。
        removed_view =
            unsafe { libc::syscall(libc::SYS_unlinkat, libc::AT_FDCWD, c_view.as_ptr(), 0) } == 0;
    }
    if std::fs::metadata(&real_path).is_ok()
        && let Ok(c_real) = cstring_path(&real_path)
    {
        // SAFETY: c_real 以 NUL 结尾，unlinkat 调用期间保持有效，仅按路径读取。
        removed_real =
            unsafe { libc::syscall(libc::SYS_unlinkat, libc::AT_FDCWD, c_real.as_ptr(), 0) } == 0;
    }

    if removed_real || removed_view {
        log::info!(
            "fuse sqlite shm poisoned backend removed real={} view={} size={} pkg={} rel={}",
            removed_real,
            removed_view,
            size,
            package_name,
            rel
        );
        return true;
    }
    let errno = last_errno();
    log::warn!(
        "fuse sqlite shm poisoned backend unlink failed errno={} {} size={} pkg={} rel={}",
        errno,
        crate::platform::errno::text(errno),
        size,
        package_name,
        rel
    );
    false
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
            self.policy.session().package_name,
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

    fn lookup(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let _perf = self.perf.observe(&self.perf.lookup_calls);
        let Some(parent_rel) = self.path_for_ino(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match Self::child_rel(&parent_rel, name) {
            Ok(rel) => {
                let policy = self.policy_for_read_request(req, Some(parent), Some(&rel));
                // 毒化 shm 在解析阶段就地处置：删除后本次 LOOKUP 自然落到 ENOENT
                // 负缓存，应用随后的 O_CREAT 打开会重建全新 inode（自愈入口）。
                if policy.is_own_media_sqlite_shm_rel(&rel) {
                    heal_poisoned_sqlite_shm_backend(
                        policy.user_id,
                        policy.media_sqlite_view_root(),
                        &rel,
                        &policy.package_name,
                    );
                }
                self.reply_entry_for_rel(&policy, &rel, reply)
            }
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

    fn getattr(&self, req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let _perf = self.perf.observe(&self.perf.metadata_calls);
        let policy = self.policy_for_read_request(req, Some(ino), None);
        match self
            .backend_for_ino(&policy, ino)
            .and_then(|backend| self.visible_attr_for_backend(&policy, ino, &backend))
        {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(errno) => reply.error(errno),
        }
    }

    fn readlink(&self, req: &Request, ino: INodeNo, reply: ReplyData) {
        let _perf = self.perf.observe(&self.perf.metadata_calls);
        let policy = self.policy_for_read_request(req, Some(ino), None);
        match self.backend_for_ino(&policy, ino).and_then(|backend| {
            std::fs::read_link(&backend.path)
                .map(|path| path.as_os_str().as_bytes().to_vec())
                .map_err(errno_from_io)
        }) {
            Ok(bytes) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn opendir(&self, req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let _perf = self.perf.observe(&self.perf.open_calls);
        let policy = self.policy_for_read_request(req, Some(ino), None);
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
            let backend = match policy.backend_for_relative(&rel, OperationKind::Read) {
                Some(backend) => backend,
                None => {
                    reply.error(Errno::ENOENT);
                    return;
                }
            };
            if !backend.path.is_dir() && !policy.is_virtual_dir(&rel) {
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
                let (candidates, sources) = collect_dir_entry_candidates(&policy, &rel);
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
                let estimated_bytes =
                    estimate_cached_dir_candidates_bytes(&rel, &candidates, &sources);
                if let Some(previous) = state.dir_candidate_cache.remove(&rel) {
                    state.dir_candidate_cache_bytes = state
                        .dir_candidate_cache_bytes
                        .saturating_sub(previous.estimated_bytes);
                }
                if estimated_bytes <= state.dir_candidate_cache_byte_budget {
                    if state.dir_candidate_cache.len() >= state.dir_candidate_cache_capacity
                        && state.dir_candidate_cache_capacity
                            < state.dir_candidate_cache_max_capacity
                    {
                        state.dir_candidate_cache_capacity = state
                            .dir_candidate_cache_capacity
                            .saturating_mul(2)
                            .min(state.dir_candidate_cache_max_capacity);
                        self.perf.record_dir_cache_capacity(
                            state.dir_candidate_cache_capacity,
                            state.dir_candidate_cache_max_capacity,
                        );
                    }
                    while !state.dir_candidate_cache.is_empty()
                        && (state.dir_candidate_cache.len() >= state.dir_candidate_cache_capacity
                            || state
                                .dir_candidate_cache_bytes
                                .saturating_add(estimated_bytes)
                                > state.dir_candidate_cache_byte_budget)
                    {
                        let Some(evicted_key) = state.dir_candidate_cache.keys().next().cloned()
                        else {
                            break;
                        };
                        if let Some(evicted) = state.dir_candidate_cache.remove(&evicted_key) {
                            state.dir_candidate_cache_bytes = state
                                .dir_candidate_cache_bytes
                                .saturating_sub(evicted.estimated_bytes);
                            self.perf.record_dir_cache_eviction();
                        }
                    }
                    state.dir_candidate_cache_bytes = state
                        .dir_candidate_cache_bytes
                        .saturating_add(estimated_bytes);
                    state.dir_candidate_cache.insert(
                        rel.clone(),
                        CachedDirCandidates {
                            created_at: Instant::now(),
                            estimated_bytes,
                            candidates: candidates.clone(),
                            sources,
                        },
                    );
                } else {
                    self.perf.record_dir_cache_oversize();
                }
                self.perf.record_dir_cache_budget(
                    state.dir_candidate_cache_bytes,
                    state.dir_candidate_cache_byte_budget,
                );
                self.perf
                    .record_dir_cache_peak_entries(state.dir_candidate_cache.len());
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
        req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectoryPlus,
    ) {
        let _perf = self.perf.observe(&self.perf.read_calls);
        let policy = self.policy_for_read_request(req, Some(ino), None);
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
            let Some(backend) = policy.backend_for_relative(&entry.rel, OperationKind::Read) else {
                continue;
            };
            let Ok(attr) = self.visible_attr_for_backend(&policy, entry.ino, &backend) else {
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

    fn open(&self, req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let _perf = self.perf.observe(&self.perf.open_calls);
        let policy = if open_flags_write(flags.0) {
            self.policy_for_request(req)
        } else {
            self.policy_for_read_request(req, Some(ino), None)
        };
        let backend = match self.backend_for_ino(&policy, ino) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only && open_flags_write(flags.0) {
            policy.emit_monitor_read_only_deny(fuse_open_operation_name(flags.0), &backend);
            reply.error(Errno::EROFS);
            return;
        }
        // 毒化 shm 自愈兜底：内核 dcache 命中时 open 不会先走 lookup，这里补一处
        // 同样的处置——删除后端文件并让本次 open 返回 ENOENT，应用随后的 O_CREAT
        // 重开会重建全新 inode。只读打开不处置（不因读请求删文件）。
        if open_flags_write(flags.0)
            && policy.is_own_media_sqlite_shm_rel(&backend.rel)
            && heal_poisoned_sqlite_shm_backend(
                policy.user_id,
                policy.media_sqlite_view_root(),
                &backend.rel,
                &policy.package_name,
            )
        {
            reply.error(Errno::ENOENT);
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
                    _backing: None,
                    is_read_only: backend.is_read_only,
                },
            );
            fh
        };
        if self.passthrough_enabled.load(Ordering::Relaxed) {
            match reply.open_backing(&file) {
                Ok(backing) => {
                    let backing = Arc::new(backing);
                    {
                        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                        if let Some(open_file) = state.files.get_mut(&fh) {
                            open_file._backing = Some(Arc::clone(&backing));
                        }
                    }
                    reply.opened_passthrough(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE, &backing)
                }
                Err(error) => {
                    self.passthrough_enabled.store(false, Ordering::Relaxed);
                    log::debug!(
                        "fuse passthrough disabled after backing open failure pkg={} rel={} err={}",
                        policy.package_name,
                        backend.rel,
                        error
                    );
                    reply.opened(FileHandle(fh), FopenFlags::empty());
                }
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
        req: &Request,
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
        let policy = self.policy_for_request(req);
        let file = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(open_file) = state.files.get(&fh.into()) else {
                reply.error(Errno::EBADF);
                return;
            };
            if open_file.is_read_only {
                let rel = open_file.rel.clone();
                drop(state);
                if let Some(backend) = policy.backend_for_relative(&rel, OperationKind::Write) {
                    policy.emit_monitor_read_only_deny("write", &backend);
                }
                reply.error(Errno::EROFS);
                return;
            }
            let Some(file) = open_file.file.clone() else {
                drop(state);
                match self.backend_for_ino(&policy, ino) {
                    Ok(backend) if backend.is_read_only => {
                        policy.emit_monitor_read_only_deny("write", &backend);
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
        req: &Request,
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
        let policy = self.policy_for_request(req);
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
            if let Some(backend) = policy.backend_for_relative(&output_rel, OperationKind::Write) {
                policy.emit_monitor_read_only_deny("copy_file_range", &backend);
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
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        flags: i32,
        reply: ReplyCreate,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let NewEntryRoute {
            policy,
            rel,
            backend,
        } = match self.route_new_entry(req, parent, name, "create") {
            Ok(route) => route,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
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
            policy.uid,
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
        let attr = match self.attr_for_backend(&policy, ino, &backend) {
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
                    _backing: None,
                    is_read_only: false,
                },
            );
            state.clear_dir_candidate_cache();
            fh
        };
        policy.emit_monitor_create(&backend);
        if self.passthrough_enabled.load(Ordering::Relaxed) {
            match reply.open_backing(&file) {
                Ok(backing) => {
                    let backing = Arc::new(backing);
                    {
                        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                        if let Some(open_file) = state.files.get_mut(&fh) {
                            open_file._backing = Some(Arc::clone(&backing));
                        }
                    }
                    reply.created_passthrough(
                        &TTL,
                        &attr,
                        Generation(0),
                        FileHandle(fh),
                        FopenFlags::empty(),
                        &backing,
                    );
                }
                Err(error) => {
                    self.passthrough_enabled.store(false, Ordering::Relaxed);
                    log::debug!(
                        "fuse passthrough disabled after backing create failure pkg={} rel={} err={}",
                        policy.package_name,
                        backend.rel,
                        error
                    );
                    reply.created(
                        &TTL,
                        &attr,
                        Generation(0),
                        FileHandle(fh),
                        FopenFlags::empty(),
                    );
                }
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
        req: &Request,
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
        let NewEntryRoute {
            policy,
            rel,
            backend,
        } = match self.route_new_entry(req, parent, name, "mknod") {
            Ok(route) => route,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
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
            policy.uid,
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
        match self.attr_for_backend(&policy, ino, &backend) {
            Ok(attr) => {
                policy.emit_monitor_create(&backend);
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
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        mode: u32,
        umask: u32,
        reply: ReplyEntry,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let NewEntryRoute {
            policy,
            rel,
            backend,
        } = match self.route_new_entry(req, parent, name, "mkdir") {
            Ok(route) => route,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let mode = mode & !umask;
        match std::fs::create_dir(&backend.path) {
            Ok(()) => fix_path_metadata(
                &backend.path,
                policy.uid,
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
        policy.emit_monitor_create(&backend);
        self.reply_entry_for_rel(&policy, &rel, reply);
    }

    fn unlink(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let policy = self.policy_for_request(req);
        self.remove_child(&policy, parent, name, false, reply);
    }

    fn rmdir(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let policy = self.policy_for_request(req);
        self.remove_child(&policy, parent, name, true, reply);
    }

    fn rename(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        newparent: INodeNo,
        newname: &OsStr,
        flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let policy = self.policy_for_request(req);
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
        let old_backend = match self.backend_for_relative(&policy, &old_rel, OperationKind::Write) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let new_backend = match self.backend_for_relative(&policy, &new_rel, OperationKind::Write) {
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
            policy.emit_monitor_read_only_deny_with_from(
                "rename",
                record_backend,
                Some(&old_backend),
                libc::EROFS,
            );
            reply.error(Errno::EROFS);
            return;
        }
        if let Err(errno) = self.ensure_parent_for_backend(&policy, &new_backend) {
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
                    policy.uid,
                    new_backend.is_shared_public_backend,
                );
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                remap_inode_path(&mut state, &old_rel, &new_rel);
                state.clear_dir_candidate_cache();
                reply.ok();
            }
            Err(errno) => reply.error(errno),
        }
    }

    fn setattr(
        &self,
        req: &Request,
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
        let policy = self.policy_for_request(req);
        let backend = match self.backend_for_ino(&policy, ino) {
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
            policy.emit_monitor_read_only_deny(
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

        match self.attr_for_backend(&policy, ino, &backend) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(errno) => reply.error(errno),
        }
    }

    fn access(&self, req: &Request, ino: INodeNo, mask: AccessFlags, reply: ReplyEmpty) {
        let policy = if mask.contains(AccessFlags::W_OK) {
            self.policy_for_request(req)
        } else {
            self.policy_for_read_request(req, Some(ino), None)
        };
        let backend = match self.backend_for_ino(&policy, ino) {
            Ok(backend) => backend,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only && mask.contains(AccessFlags::W_OK) {
            policy.emit_monitor_read_only_deny_with_errno("access:write", &backend, libc::EACCES);
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
        // SAFETY: c_path 在本作用域内存活且以 NUL 结尾，access 只读取路径不写入任何缓冲。
        let ret = unsafe { libc::access(c_path.as_ptr(), mask.bits()) };
        if ret == 0 {
            reply.ok();
        } else {
            reply.error(errno_from_code(last_errno()));
        }
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        // 文件系统容量是会话级语义，与调用方身份无关，固定用会话绑定策略的真实根。
        let path = self.policy.session().real_root.as_path();
        let c_path = match cstring_path(path) {
            Ok(path) => path,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: stat 为本作用域独占的 MaybeUninit，statvfs 成功时才会初始化它。
        let ret = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if ret != 0 {
            reply.error(errno_from_code(last_errno()));
            return;
        }
        // SAFETY: 上面 statvfs 返回 0，stat 已被完整初始化。
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
        req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        _datasync: bool,
        reply: ReplyEmpty,
    ) {
        let policy = self.policy_for_request(req);
        {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            if !state.dirs.contains_key(&fh.into()) {
                reply.error(Errno::EBADF);
                return;
            }
        }
        let backend = match self.backend_for_ino(&policy, ino) {
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
    policy: &super::policy::RedirectPolicy,
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
    policy: &super::policy::RedirectPolicy,
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
    policy: &super::policy::RedirectPolicy,
    parent_rel: &str,
    entries: &mut Vec<DirEntryCandidate>,
    seen: &mut HashMap<String, usize>,
) {
    use crate::platform::paths;
    for prefix in policy.rule_prefixes.iter() {
        let Some(child_name) = super::policy::visible_prefix_child(parent_rel, prefix) else {
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
