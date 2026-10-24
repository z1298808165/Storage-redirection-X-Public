// FUSE 回调实现与目录项枚举。
//
// 这里只负责「内核请求进来之后怎么应答」：`Filesystem` trait 的每个回调，以及只服务
// readdir/readdirplus 的目录项收集。Fs 自身的状态、句柄与策略解析留在 mod.rs，
// 回调改动不会碰到状态结构。

use super::attrs::file_type_from_std;
use super::helpers::{
    elapsed_ns, fuse_open_operation_name, fuse_setattr_operation_name, open_flags_create,
    open_flags_write, paths_eq,
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
    ATTR_TTL, CachedDirCandidates, DIR_CANDIDATE_CACHE_TTL, DirEntry, DirEntryCandidate,
    DirectorySourceSignature, DirectorySourceState, ENTRY_TTL, FUSE_READ_BUFFER, FuseRedirectFs,
    FuseState, MAX_READ_SIZE, MEDIA_RW_GID, MEDIA_RW_UID, NewEntryRoute, OpenFile, ROOT_INO,
    estimate_cached_dir_candidates_bytes,
};
use crate::platform::paths;
use fuser::{
    AccessFlags, CopyFileRangeFlags, Errno, FileHandle, FileType, Filesystem, FopenFlags,
    Generation, INodeNo, InitFlags, KernelConfig, LockOwner, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyDirectoryPlus, ReplyEmpty, ReplyEntry, ReplyLseek,
    ReplyOpen, ReplyStatfs, ReplyWrite, ReplyXattr, Request, TimeOrNow, WriteFlags,
};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
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
///
/// 满尺寸（≥32KiB）的 `-shm` 也可能毒化：wal-index 头 salt 与同级 `-wal` 文件头
/// 失配（跨 FUSE 视图 inode 分裂、异常会话残留）。SQLite 信任已初始化的 shm 头
/// 而不复查 `-wal` 文件，会把未 checkpoint 的 WAL 数据整体判无效，应用静默回滚到
/// 旧检查点（跨应用共享库掉登录），随后的写入覆写 WAL 造成不可逆数据丢失。此类
/// 毒化按 [`sqlite_shm_wal_index_stale`] 内容级判定，删除同样无损。
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
    if size == 0 {
        return false;
    }
    let stale_wal_index =
        size >= SQLITE_SHM_MIN_SIZE_BYTES && sqlite_shm_wal_index_stale(&real_path);
    if size >= SQLITE_SHM_MIN_SIZE_BYTES && !stale_wal_index {
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
            "fuse sqlite shm poisoned backend removed real={} view={} size={} stale_wal_index={} pkg={} rel={}",
            removed_real,
            removed_view,
            size,
            stale_wal_index,
            package_name,
            rel
        );
        return true;
    }
    let errno = last_errno();
    log::warn!(
        "fuse sqlite shm poisoned backend unlink failed errno={} {} size={} stale_wal_index={} pkg={} rel={}",
        errno,
        crate::platform::errno::text(errno),
        size,
        stale_wal_index,
        package_name,
        rel
    );
    false
}

/// 由 `-shm` 边车路径推出同级 `-wal` 路径：后缀按小写不敏感识别，替换为小写
/// `-wal`，库名主干保持原样。
fn sqlite_sidecar_wal_path(shm_path: &Path) -> Option<std::path::PathBuf> {
    let file_name = shm_path.file_name()?.to_str()?;
    let lower = file_name.to_ascii_lowercase();
    let stem_len = lower.len().checked_sub("-shm".len())?;
    if stem_len == 0 || !lower.ends_with("-shm") {
        return None;
    }
    let mut wal_path = shm_path.to_path_buf();
    wal_path.set_file_name(std::ffi::OsString::from(format!(
        "{}-wal",
        &file_name[..stem_len]
    )));
    Some(wal_path)
}

/// 读取文件前 `len` 字节；文件不足 `len` 字节或不可读时返回 `None`。
fn read_file_prefix(path: &Path, len: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = File::open(path).ok()?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// 满尺寸 `-shm` 的内容级毒化判定：wal-index 头与同级 `-wal` 文件头 salt 失配。
///
/// SQLite 恢复 wal-index 时按 `memcpy` 原样拷贝 `-wal` 文件头（大端字节模式）的
/// 8 字节 salt 到 wal-index 头偏移 32..40，不做字节序转换，因此毒化判定用原始
/// 字节比对即可。两者失配说明 wal-index 停留在旧一代 WAL 或来自分裂视图。
/// `-shm` 头未初始化（isInit≠1）或 `-wal` 头缺失/魔数非法时 SQLite 自己会重建，
/// 一律不判定为毒化，保持保守。
fn sqlite_shm_wal_index_stale(real_shm_path: &Path) -> bool {
    let Some(wal_path) = sqlite_sidecar_wal_path(real_shm_path) else {
        return false;
    };
    let Some(shm) = read_file_prefix(real_shm_path, 48) else {
        return false;
    };
    if shm[12] != 1 {
        return false;
    }
    let Some(wal) = read_file_prefix(&wal_path, 32) else {
        return false;
    };
    let wal_magic = u32::from_be_bytes([wal[0], wal[1], wal[2], wal[3]]);
    if wal_magic != 0x377f_0682 && wal_magic != 0x377f_0683 {
        return false;
    }
    shm[32..40] != wal[16..24]
}

/// backend 单次 pwrite 短写后被循环补写观测到的累计次数。
static SHORT_WRITE_RECOVERED: AtomicU64 = AtomicU64::new(0);

/// 把 `data` 完整写入 backend 的 `offset`，返回写入总长度（恒等于 `data.len()`）。
///
/// FUSE WRITE 契约：应答必须恰好等于请求字节数，否则返回错误（fuser 协议解析层
/// 的注释同样如此声明）。backend 文件的单次 pwrite 在信号中断、或 backend 叠在
/// 另一层受限文件系统上时可能返回短写——把短写计数原样应答会让内核把短写透传给
/// 应用且丢失 errno（应用侧表现为诡异短写 + 陈旧 errno，夸克下载引擎视之为致命
/// 错误进入重建文件死循环），writeback 路径则直接判 IO 失败。因此这里循环补写至
/// 请求长度；无法推进（Ok(0)）或底层报错时返回真实 errno。
fn write_full_at(file: &File, mut data: &[u8], mut offset: u64) -> std::io::Result<usize> {
    let total = data.len();
    let mut first_write = 0usize;
    while !data.is_empty() {
        match file.write_at(data, offset) {
            Ok(0) => return Err(std::io::Error::from_raw_os_error(libc::ENOSPC)),
            Ok(n) => {
                if first_write == 0 {
                    first_write = n;
                }
                data = &data[n..];
                offset = offset.saturating_add(n as u64);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    if first_write < total {
        let seen = SHORT_WRITE_RECOVERED.fetch_add(1, Ordering::Relaxed) + 1;
        if seen <= 32 || seen.is_multiple_of(1024) {
            log::warn!(
                "fuse write short recovered count={} want={} first_write={} end_offset={offset}",
                seen,
                total,
                first_write
            );
        }
    }
    Ok(total)
}

/// xattr 首查/取值两段式应答的中间产物：`size==0` 的探测请求回长度，其余回数据。
pub(super) enum XattrOutcome {
    Size(u32),
    Data(Vec<u8>),
}

impl Filesystem for FuseRedirectFs {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        // 三层判定（自动开启，兼容性由端到端探针保障）：
        // 1. 探测状态：broken（本 boot 探针发现内核缺陷）→ 关闭。daemon 在首次
        //    共享宿主挂载后执行探针，发现缺陷会重建宿主，重建后的会话在此读到
        //    broken；ok 或未知（尚未探测）→ 继续；
        // 2. 内核能力协商：FUSE_PASSTHROUGH 不可用自动走纯用户态（不抛错给应用）；
        // 3. `.fuse_passthrough_disabled` 逃生口：紧急排障时无条件关闭。
        let probe_state = crate::config::fuse_passthrough_probe_state();
        let passthrough_supported = config.capabilities().contains(InitFlags::FUSE_PASSTHROUGH);
        let passthrough_enabled = probe_state.unwrap_or(true)
            && passthrough_supported
            && !crate::config::is_fuse_passthrough_disabled();
        let stack_depth_enabled = config.set_max_stack_depth(2).is_ok();
        // 并发额度是**连接级**的，而共享宿主会话只有一条连接。曾据此推测开机风暴时额度
        // 打满导致 fuse_get_req 排队，实测把额度提到 512/384 后该类卡死并未消失
        // （真机 2026-10-16 三轮开机采样均仍有），说明不是队列拥塞，而是挂载子进程解析到
        // 了连接已中止的死 FUSE 层（fc->blocked 常驻，只有 SIGKILL 能打断）。因此额度维持
        // 原值，根因另案处理：子进程不应在 FUSE 路径上做元数据操作。
        let max_background_enabled = config.set_max_background(128).is_ok();
        let congestion_enabled = config.set_congestion_threshold(96).is_ok();
        let max_write_enabled = config.set_max_write(4 * 1024 * 1024).is_ok();
        // 内核写回缓存：小块写先落内核页缓存、按大块回传，砍掉每请求同步往返。
        // 真机 2026-10-15：同步往返模式下 4K 写 6.9MB/s（每请求 ~550μs），远低于系统
        // FUSE 的 353MB/s。注意 writeback_cache 不是 legacy mount data 选项（传了会
        // EINVAL），只能在 INIT 应答里请求 FUSE_WRITEBACK_CACHE 能力位。
        let writeback_enabled = config
            .add_capabilities(InitFlags::FUSE_WRITEBACK_CACHE)
            .is_ok();
        // passthrough 必须在 INIT 应答里申请能力位，否则内核拒绝所有 backing 注册，
        // open_backing 首败即触发会话级关闭，数据面永远走 daemon 往返（真机 2026-10-15：
        // 大文件 1.9GB 写期间宿主会话 daemon CPU 124%，487MB/s 封顶；内核直通后大 IO
        // 绕过 daemon，接近原生吞吐）。probe_state ok 只代表内核数据面可用，不代表
        // 本会话协商过能力位。
        let passthrough_negotiated =
            passthrough_enabled && config.add_capabilities(InitFlags::FUSE_PASSTHROUGH).is_ok();
        self.passthrough_enabled
            .store(passthrough_negotiated, Ordering::Relaxed);
        self.writeback_enabled
            .store(writeback_enabled, Ordering::Relaxed);
        log::info!(
            "fuse init pkg={} kernel_abi={} passthrough_supported={} passthrough_enabled={} passthrough_negotiated={} probe_state={:?} passthrough_disabled_by_marker={} stack_depth={} max_background={} congestion={} max_write={} writeback={}",
            self.policy.session().package_name,
            config.kernel_abi(),
            passthrough_supported,
            passthrough_enabled,
            passthrough_negotiated,
            probe_state,
            crate::config::is_fuse_passthrough_disabled(),
            stack_depth_enabled,
            max_background_enabled,
            congestion_enabled,
            max_write_enabled,
            writeback_enabled
        );
        Ok(())
    }

    fn lookup(&self, req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let _perf = self.perf.observe(&self.perf.lookup_calls);
        let Some(parent_rel) = self.path_for_ino(parent) else {
            self.log_reply_error("lookup", req.uid(), None, Errno::ENOENT);
            reply.error(Errno::ENOENT);
            return;
        };
        match Self::child_rel(&parent_rel, name) {
            Ok(rel) => {
                let policy = self.policy_for_read_request(req, Some(parent), Some(&rel));
                // 毒化 shm 在解析阶段就地处置：删除后本次 LOOKUP 自然落到 ENOENT
                // 负缓存，应用随后的 O_CREAT 打开会重建全新 inode（自愈入口）。
                if policy.should_heal_media_sqlite_shm_rel(&rel) {
                    heal_poisoned_sqlite_shm_backend(
                        policy.user_id,
                        policy.media_sqlite_view_root(),
                        &rel,
                        &policy.package_name,
                    );
                }
                self.reply_entry_for_rel(req.uid(), &policy, &rel, reply)
            }
            Err(errno) => {
                Self::log_new_entry_errno(
                    &self.policy_for_read_request(req, Some(parent), None),
                    name,
                    errno,
                );
                reply.error(errno)
            }
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
        let mut policy = self.policy_for_read_request(req, Some(ino), None);
        // fstat on open fd：fd 可能已跨进程（binder dup），按请求者 uid 重新路由
        // 会把合法 fstat 打成 ENOENT，回退到 inode 属主策略（文件身份已固定）。
        // 不以 fh 是否存在为门控：内核对 fstat 未必置 FUSE_GETATTR_FH，而非属主
        // 经路径 lookup 根本拿不到属主私有 inode，能引用该 ino 即 fd 途径。
        if let Some(owner_policy) = self.inode_owner_policy_for_fd_stat(ino, req.uid()) {
            policy = owner_policy;
        }
        // 宿主共享会话的系统读取者路径反解：MP 扫描/元数据协商对重定向树的
        if let Some(rel) = self.path_for_ino(ino)
            && let Some(owner_policy) = self.system_reader_policy_on_miss(req.uid(), &policy, &rel)
        {
            policy = owner_policy;
        }
        match self
            .backend_for_ino(&policy, ino)
            .and_then(|backend| self.visible_attr_for_backend(&policy, ino, &backend))
        {
            Ok(attr) => reply.attr(&ATTR_TTL, &attr),
            Err(errno) => {
                let rel = self.path_for_ino(ino);
                self.log_reply_error("getattr", req.uid(), rel.as_deref(), errno);
                reply.error(errno)
            }
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
        let mut policy = self.policy_for_read_request(req, Some(ino), None);
        let track_dir_perf = crate::logging::is_debug_logging_enabled();
        let first_lock_started = track_dir_perf.then(std::time::Instant::now);
        let (mut rel, mut path_version) = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(rel) = state.paths_by_inode.get(&ino.0).cloned() else {
                reply.error(Errno::ENOENT);
                return;
            };
            // MP 扫描重定向树目录靠系统读取者重试才能看到条目；rel 在锁内一并取出。
            if let Some(owner_policy) = self.system_reader_policy_on_miss(req.uid(), &policy, &rel)
            {
                policy = owner_policy;
            }
            (
                rel,
                state.inode_path_versions.get(&ino.0).copied().unwrap_or(0),
            )
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
                &ENTRY_TTL,
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
        let is_create = open_flags_create(flags.0);
        let is_mutation = open_flags_write(flags.0) || is_create;
        let mut policy = if is_mutation {
            self.policy_for_request(req)
        } else {
            self.policy_for_read_request(req, Some(ino), None)
        };
        // 读取类 open 走系统读取者 miss 重试；写入类保持宿主拒绝语义。
        if !is_mutation
            && let Some(rel) = self.path_for_ino(ino)
            && let Some(owner_policy) = self.system_reader_policy_on_miss(req.uid(), &policy, &rel)
        {
            policy = owner_policy;
        }
        let backend = match self.backend_for_ino(&policy, ino) {
            Ok(backend) => backend,
            Err(errno) => {
                Self::log_unexpected_open_errno(&policy, "", errno);
                reply.error(errno);
                return;
            }
        };
        if backend.is_read_only && is_mutation {
            log::warn!(
                "fuse open read-only deny pkg={} rel={} policy_fp={:016x} monitor_only={} op={}",
                policy.package_name,
                backend.rel,
                policy.fingerprint,
                policy.is_monitor_only,
                fuse_open_operation_name(flags.0)
            );
            policy.emit_monitor_read_only_deny(fuse_open_operation_name(flags.0), &backend);
            reply.error(Errno::EROFS);
            return;
        }
        // 毒化 shm 自愈兜底：内核 dcache 命中时 open 不会先走 lookup，这里补一处
        // 同样的处置——删除后端文件并让本次 open 返回 ENOENT，应用随后的 O_CREAT
        // 重开会重建全新 inode。只读打开不处置（不因读请求删文件）。
        if open_flags_write(flags.0)
            && policy.should_heal_media_sqlite_shm_rel(&backend.rel)
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
                Self::log_unexpected_open_errno(&policy, &backend.rel, errno);
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
        if is_mutation {
            policy.emit_monitor_mutation(fuse_open_operation_name(flags.0), &backend, None);
        }
        // `-shm` 禁止 passthrough：setattr 的 ftruncate 钳制只覆盖 FUSE SETATTR 通道，
        // passthrough backing fd 上的 ftruncate 绕过宿主直落 f2fs，应用 WAL 能力探针
        // 会借此反复制造毒化 inode（自愈只能救到下一次打开）。
        let shm_userspace_only =
            policy.should_exclude_media_sqlite_shm_passthrough_rel(&backend.rel);
        if self.passthrough_enabled.load(Ordering::Relaxed) && !shm_userspace_only {
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
                    reply.opened(FileHandle(fh), self.file_open_flags());
                }
            }
        } else {
            reply.opened(FileHandle(fh), self.file_open_flags());
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
                Err(error) => {
                    // 采样错误探针：writeback 模式下内核 write_begin 的读改写失败
                    // 会以「应用侧 pwrite 短写」呈现，这里唯一能看到失败根因。
                    static READ_ERROR_LOGGED: AtomicU64 = AtomicU64::new(0);
                    let seen = READ_ERROR_LOGGED.fetch_add(1, Ordering::Relaxed) + 1;
                    if seen <= 32 || seen.is_multiple_of(1024) {
                        log::warn!(
                            "fuse read error count={} offset={offset} requested={requested} errno={}",
                            seen,
                            error.raw_os_error().unwrap_or(-1)
                        );
                    }
                    reply.error(errno_from_io(error));
                }
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
        let write_started = std::time::Instant::now();
        match write_full_at(&file, data, offset) {
            Ok(n) => {
                let elapsed = write_started.elapsed();
                if elapsed.as_millis() >= 2 {
                    // 慢写探针：定位大块回写在 handler 侧的真实耗时（debug 级）。
                    log::debug!(
                        "fuse write slow us={} bytes={offset}+{}",
                        elapsed.as_micros(),
                        data.len()
                    );
                }
                // 监视关闭时跳过 ino→路径解析与规则匹配：这是写入热路径上唯一的
                // 额外查找链，monitor 关闭时 emit_monitor_mutation 必然早退。
                if policy.is_file_monitor_enabled
                    && let Some(backend) = self
                        .path_for_ino(ino)
                        .and_then(|rel| policy.backend_for_relative(&rel, OperationKind::Write))
                {
                    policy.emit_monitor_mutation("write", &backend, None);
                }
                reply.written(n as u32);
            }
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

    fn fallocate(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        length: u64,
        mode: i32,
        reply: ReplyEmpty,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        // 原生 MP FUSE 支持 fallocate（MPEG4Writer 录制预分配依赖它，真机 2026-10-20
        // 实证：缺失时返回 EOPNOTSUPP，写入方降级为无预分配路径）。这里透传到后端
        // fd，与原生语义对齐。
        let Some(file) = ({
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            let Some(open_file) = state.files.get(&fh.into()) else {
                reply.error(Errno::EBADF);
                return;
            };
            if open_file.is_read_only {
                reply.error(Errno::EBADF);
                return;
            }
            open_file.file.clone()
        }) else {
            reply.error(Errno::ENOSYS);
            return;
        };
        let Some(offset) = libc::off_t::try_from(offset).ok() else {
            reply.error(Errno::EINVAL);
            return;
        };
        let Some(length) = libc::off_t::try_from(length).ok() else {
            reply.error(Errno::EINVAL);
            return;
        };
        // SAFETY: file 的 RawFd 在调用期间有效；fallocate 仅按 (offset, length) 操作空间分配。
        let ret = unsafe { libc::fallocate(file.as_raw_fd(), mode, offset, length) };
        if ret == 0 {
            reply.ok();
        } else {
            reply.error(errno_from_code(last_errno()));
        }
    }

    fn symlink(
        &self,
        req: &Request,
        parent: INodeNo,
        link_name: &OsStr,
        target: &Path,
        reply: ReplyEntry,
    ) {
        let _perf = self.perf.observe(&self.perf.mutation_calls);
        let NewEntryRoute {
            policy,
            rel,
            backend,
        } = match self.route_new_entry(req, parent, link_name, "symlink") {
            Ok(route) => route,
            Err(errno) => {
                Self::log_new_entry_errno(&self.policy_for_request(req), link_name, errno);
                reply.error(errno);
                return;
            }
        };
        let target_bytes = target.as_os_str().as_bytes();
        let c_target = match std::ffi::CString::new(target_bytes) {
            Ok(value) => value,
            Err(_) => {
                reply.error(Errno::EINVAL);
                return;
            }
        };
        let c_path = match cstring_path(&backend.path) {
            Ok(path) => path,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        // SAFETY: 两个 CString 在调用期间存活且以 NUL 结尾，symlink 只读取不写入。
        let ret = unsafe { libc::symlink(c_target.as_ptr(), c_path.as_ptr()) };
        if ret != 0 {
            reply.error(errno_from_code(last_errno()));
            return;
        }
        fix_path_metadata(
            &backend.path,
            policy.uid,
            0o777,
            backend.is_shared_public_backend,
            false,
        );
        self.invalidate_dir_candidate_cache();
        let ino = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, &rel);
            // 同 create：登记属主供跨进程 fstat 回退。
            Self::remember_policy_for_inode_locked(&mut state, ino, &rel, &policy);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.visible_attr_for_backend(&policy, ino, &backend) {
            Ok(attr) => reply.entry(&ENTRY_TTL, &attr, Generation(0)),
            Err(errno) => {
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                Self::remove_lookup_locked(&mut state, ino, 1);
                reply.error(errno);
            }
        }
    }

    fn getxattr(&self, req: &Request, ino: INodeNo, name: &OsStr, size: u32, reply: ReplyXattr) {
        let policy = self.policy_for_fd_stat_or_request(req, ino);
        let outcome = self.with_backend_for_xattr(&policy, ino, "getxattr", |c_path| {
            let c_name = match std::ffi::CString::new(name.as_bytes()) {
                Ok(value) => value,
                Err(_) => return Err(Errno::EINVAL),
            };
            // SAFETY: c_path/c_name 在调用期间存活且以 NUL 结尾；首查长度，空缓冲合法。
            let needed = unsafe {
                libc::lgetxattr(c_path.as_ptr(), c_name.as_ptr(), std::ptr::null_mut(), 0)
            };
            if needed < 0 {
                return Err(errno_from_code(last_errno()));
            }
            if size == 0 {
                return Ok(XattrOutcome::Size(needed as u32));
            }
            if (needed as u64) > size as u64 {
                return Err(Errno::ERANGE);
            }
            let mut buffer = vec![0u8; needed as usize];
            // SAFETY: buffer 长度即上面查得的 needed，lgetxattr 只写入 buffer 范围内。
            let written = unsafe {
                libc::lgetxattr(
                    c_path.as_ptr(),
                    c_name.as_ptr(),
                    buffer.as_mut_ptr() as *mut libc::c_void,
                    buffer.len(),
                )
            };
            if written < 0 {
                return Err(errno_from_code(last_errno()));
            }
            buffer.truncate(written as usize);
            Ok(XattrOutcome::Data(buffer))
        });
        match outcome {
            Ok(XattrOutcome::Size(value)) => reply.size(value),
            Ok(XattrOutcome::Data(bytes)) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn setxattr(
        &self,
        req: &Request,
        ino: INodeNo,
        name: &OsStr,
        value: &[u8],
        flags: i32,
        position: u32,
        reply: ReplyEmpty,
    ) {
        if position != 0 {
            // position 仅 pwritev 风格的 resource fork 语义使用，Linux 上恒为 0。
            reply.error(Errno::EINVAL);
            return;
        }
        let policy = self.policy_for_fd_stat_or_request(req, ino);
        let outcome = self.with_backend_for_xattr(&policy, ino, "setxattr", |c_path| {
            let c_name = match std::ffi::CString::new(name.as_bytes()) {
                Ok(value) => value,
                Err(_) => return Err(Errno::EINVAL),
            };
            // SAFETY: c_path/c_name 存活且以 NUL 结尾；value 为调用方传入的只读缓冲。
            let ret = unsafe {
                libc::lsetxattr(
                    c_path.as_ptr(),
                    c_name.as_ptr(),
                    value.as_ptr() as *const libc::c_void,
                    value.len(),
                    flags,
                )
            };
            if ret == 0 {
                Ok(XattrOutcome::Data(Vec::new()))
            } else {
                Err(errno_from_code(last_errno()))
            }
        });
        match outcome {
            Ok(_) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn listxattr(&self, req: &Request, ino: INodeNo, size: u32, reply: ReplyXattr) {
        let policy = self.policy_for_fd_stat_or_request(req, ino);
        let outcome = self.with_backend_for_xattr(&policy, ino, "listxattr", |c_path| {
            // SAFETY: c_path 存活且以 NUL 结尾；首查长度，空缓冲合法。
            let needed = unsafe { libc::llistxattr(c_path.as_ptr(), std::ptr::null_mut(), 0) };
            if needed < 0 {
                return Err(errno_from_code(last_errno()));
            }
            if size == 0 {
                return Ok(XattrOutcome::Size(needed as u32));
            }
            if (needed as u64) > size as u64 {
                return Err(Errno::ERANGE);
            }
            let mut buffer = vec![0u8; needed as usize];
            // SAFETY: buffer 长度即上面查得的 needed，llistxattr 只写入 buffer 范围内。
            let written = unsafe {
                libc::llistxattr(
                    c_path.as_ptr(),
                    buffer.as_mut_ptr() as *mut libc::c_char,
                    buffer.len(),
                )
            };
            if written < 0 {
                return Err(errno_from_code(last_errno()));
            }
            buffer.truncate(written as usize);
            Ok(XattrOutcome::Data(buffer))
        });
        match outcome {
            Ok(XattrOutcome::Size(value)) => reply.size(value),
            Ok(XattrOutcome::Data(bytes)) => reply.data(&bytes),
            Err(errno) => reply.error(errno),
        }
    }

    fn removexattr(&self, req: &Request, ino: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        let policy = self.policy_for_fd_stat_or_request(req, ino);
        let outcome = self.with_backend_for_xattr(&policy, ino, "removexattr", |c_path| {
            let c_name = match std::ffi::CString::new(name.as_bytes()) {
                Ok(value) => value,
                Err(_) => return Err(Errno::EINVAL),
            };
            // SAFETY: c_path/c_name 存活且以 NUL 结尾，lremovexattr 只读取。
            let ret = unsafe { libc::lremovexattr(c_path.as_ptr(), c_name.as_ptr()) };
            if ret == 0 {
                Ok(XattrOutcome::Data(Vec::new()))
            } else {
                Err(errno_from_code(last_errno()))
            }
        });
        match outcome {
            Ok(_) => reply.ok(),
            Err(errno) => reply.error(errno),
        }
    }

    fn lseek(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: i64,
        whence: i32,
        reply: ReplyLseek,
    ) {
        let file = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            match state.files.get(&fh.into()).and_then(|f| f.file.clone()) {
                Some(file) => file,
                None => {
                    reply.error(Errno::EBADF);
                    return;
                }
            }
        };
        // 内核仅对 SEEK_HOLE/SEEK_DATA 下发 FUSE lseek（普通定位在页缓存层完成）。
        // 后端文件非稀疏语义：DATA 返回起始偏移，HOLE 返回文件尾（EOF 处的洞）。
        let size = match file.metadata() {
            Ok(metadata) => metadata.len() as i64,
            Err(error) => {
                reply.error(errno_from_io(error));
                return;
            }
        };
        match whence {
            libc::SEEK_DATA => {
                if offset >= size {
                    reply.error(Errno::ENXIO);
                } else {
                    reply.offset(offset.max(0));
                }
            }
            libc::SEEK_HOLE => {
                if offset >= size {
                    reply.error(Errno::ENXIO);
                } else {
                    reply.offset(size);
                }
            }
            _ => reply.error(Errno::EINVAL),
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
                Self::log_new_entry_errno(&self.policy_for_request(req), name, errno);
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
            // 登记属主：该文件 fd 经 binder 跨进程后，fstat（GETATTR 带 fh）
            // 需要回退到创建者策略而不是按请求者 uid 重路由。
            Self::remember_policy_for_inode_locked(&mut state, ino, &rel, &policy);
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
        // 与 open 同理：新建的 `-shm` 也不得给 passthrough backing，否则应用首个
        // WAL 能力探针（自愈重建后必然紧随一次建连）就会把全新 inode 再次截成
        // 毒化尺寸，自愈循环永不收敛。
        let shm_userspace_only =
            policy.should_exclude_media_sqlite_shm_passthrough_rel(&backend.rel);
        if self.passthrough_enabled.load(Ordering::Relaxed) && !shm_userspace_only {
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
                        &ATTR_TTL,
                        &attr,
                        Generation(0),
                        FileHandle(fh),
                        self.file_open_flags(),
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
                        &ATTR_TTL,
                        &attr,
                        Generation(0),
                        FileHandle(fh),
                        self.file_open_flags(),
                    );
                }
            }
        } else {
            reply.created(
                &ATTR_TTL,
                &attr,
                Generation(0),
                FileHandle(fh),
                self.file_open_flags(),
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
                Self::log_unexpected_open_errno(&policy, &backend.rel, errno);
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
            // 同 create：登记属主供跨进程 fstat 回退。
            Self::remember_policy_for_inode_locked(&mut state, ino, &rel, &policy);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.attr_for_backend(&policy, ino, &backend) {
            Ok(attr) => {
                policy.emit_monitor_create(&backend);
                reply.entry(&ENTRY_TTL, &attr, Generation(0));
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
        self.reply_entry_for_rel(req.uid(), &policy, &rel, reply);
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
        // 重命名既可能把临时文件首次落到目标路径，也可能覆盖已有目标。即使用户过滤了
        // `rename*`，目标文件本身仍应留下可检索的创建/更新记录；否则下载器常见的
        // “临时文件 -> 最终文件”提交动作会让最终文件在监视页完全消失。
        let destination_existed = std::fs::symlink_metadata(&new_backend.path).is_ok();
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
                policy.emit_monitor_mutation(
                    if destination_existed {
                        "write"
                    } else {
                        "create"
                    },
                    &new_backend,
                    Some(&old_backend),
                );
                policy.emit_monitor_mutation("rename", &new_backend, Some(&old_backend));
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
        let mut policy = self.policy_for_request(req);
        // ftruncate/fchmod 等经跨进程 fd 到达时同样不能按请求者 uid 重路由
        // （fstat 同理，见 getattr 的属主回退注释）。
        if let Some(owner_policy) = self.inode_owner_policy_for_fd_stat(ino, req.uid()) {
            policy = owner_policy;
        }
        let backend = match self.backend_for_ino(&policy, ino) {
            Ok(backend) => backend,
            Err(errno) => {
                let rel = self.path_for_ino(ino);
                self.log_reply_error("setattr", req.uid(), rel.as_deref(), errno);
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
        let size = size.map(|requested| {
            let file_name = backend.rel.rsplit('/').next().unwrap_or(&backend.rel);
            let is_sqlite_shm = file_name.len() > "-shm".len()
                && file_name.to_ascii_lowercase().ends_with("-shm")
                && paths::is_sqlite_database_or_sidecar_path(&backend.rel);
            if is_sqlite_shm && requested > 0 && requested < SQLITE_SHM_MIN_SIZE_BYTES {
                SQLITE_SHM_MIN_SIZE_BYTES
            } else {
                requested
            }
        });
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
            Ok(attr) => reply.attr(&ATTR_TTL, &attr),
            Err(errno) => reply.error(errno),
        }
    }

    fn access(&self, req: &Request, ino: INodeNo, mask: AccessFlags, reply: ReplyEmpty) {
        let mut policy = if mask.contains(AccessFlags::W_OK) {
            self.policy_for_request(req)
        } else {
            self.policy_for_read_request(req, Some(ino), None)
        };
        if let Some(rel) = self.path_for_ino(ino)
            && let Some(owner_policy) = self.system_reader_policy_on_miss(req.uid(), &policy, &rel)
        {
            policy = owner_policy;
        }
        let backend = match self.backend_for_ino(&policy, ino) {
            Ok(backend) => backend,
            Err(errno) => {
                let rel = self.path_for_ino(ino);
                self.log_reply_error("access", req.uid(), rel.as_deref(), errno);
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
        // 对齐原生 FuseDaemon 的内部目录隐藏语义：.transforms 与 .picker_transcoded
        // 是 MediaProvider 的工作目录，原生 FUSE 在存储根级对应用隐藏。后端根即
        // /data/media/0，裸枚举会把这两个名字暴露到应用视图（例：接入共享宿主但
        // 未配置任何规则的 MT 管理器在 /storage/emulated/0 直接可见），因此仅在
        // 视图根级枚举时按文件名跳过（注意 is_filtered_media_provider_path 匹配
        // 的是带斜杠的完整路径形态，裸文件名必须直接比对），不触碰深层同名用户
        // 目录。本改动只恢复枚举可见性；按已知路径的 lookup/open 不受限，这是
        // 刻意收窄——宿主自身与 media 视图会话仍需按路径访问这些目录。
        if parent_rel.is_empty() && matches!(name.as_str(), ".transforms" | ".picker_transcoded") {
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
