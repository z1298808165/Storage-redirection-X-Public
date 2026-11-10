use super::MountPlanner;
use super::planner_paths::{
    BindLandingSample, BindLandingVerdict, allowed_real_metadata_path_is_excluded,
    fix_allowed_real_directory_metadata, fix_read_only_public_metadata,
    fix_real_public_directory_metadata, fuse_backed_bind_landing_verdict,
    is_android_private_backend_path, is_android_private_package_root,
    is_android_private_storage_path, is_data_media_shared_public_directory, is_mount_point,
    is_shared_public_storage_directory, parent_preserving_backend_alias, paths_have_same_inode,
    remount_bind_read_only_inner, remount_bind_read_write_inner, shared_storage_root,
    should_apply_allowed_real_writable_metadata, should_apply_app_writable_metadata, stat_inode,
    stat_is_directory, storage_emulated_repair_mount, topmost_mount_is_fuse,
};
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::{fs, paths};
use libc::{
    CLONE_NEWNS, MNT_DETACH, MS_BIND, MS_REC, chmod, chown, mount, readlink, stat as c_stat,
    umount2, unshare,
};
use std::collections::VecDeque;
use std::ffi::CString;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const MEDIA_RW_UID: u32 = 1023;
pub(super) const MEDIA_RW_GID: u32 = 1023;
// 目录模式只在 `platform::fs` 定义一次，避免多处副本各自漂移（曾出现 0o2773 与
// 0o2771 两套公共目录口径并存）。
pub(super) use crate::platform::fs::MAPPED_DIR_MODE;
pub(super) const ALLOWED_REAL_TREE_METADATA_LIMIT: usize = 256;
pub(super) const READ_ONLY_TREE_METADATA_LIMIT: usize = 512;
const BIND_SUCCESS_LOG_STEP: u64 = 128;
const BIND_VERIFY_PASS_LOG_STEP: u64 = 256;
static BIND_SUCCESS_COUNT: AtomicU64 = AtomicU64::new(0);
static BIND_VERIFY_PASS_COUNT: AtomicU64 = AtomicU64::new(0);

#[inline]
fn should_log_step(count: u64, step: u64) -> bool {
    count == 1 || count.is_multiple_of(step)
}

/// `open_tree(2)` 的 `OPEN_TREE_CLONE`：克隆出一个游离挂载（fd 携带，不附着命名空间）。
const OPEN_TREE_CLONE: libc::c_uint = 0x0000_0001;
/// `open_tree(2)` 的 `AT_RECURSIVE`：连同子树一起克隆。
const AT_RECURSIVE: libc::c_uint = 0x0000_8000;
/// `move_mount(2)` 的 `MOVE_MOUNT_F_EMPTY_PATH`：源由 fd 指定。
const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 0x0000_0004;
/// `move_mount(2)` 的 `MOVE_MOUNT_T_EMPTY_PATH`：目标由 fd 指定。
const MOVE_MOUNT_T_EMPTY_PATH: libc::c_uint = 0x0000_0040;
/// `AID_EVERYBODY`：`/storage` 是 `shell:everybody 0710`，应用都在该组里，工作进程必须补上。
const AID_EVERYBODY: libc::gid_t = 9997;

impl MountPlanner {
    // 按需 unshare，并隔离目标应用命名空间内的全部挂载传播。
    pub(crate) fn ensure_mount_namespace_prepared(&mut self) -> bool {
        if self.is_namespace_ready {
            return true;
        }

        if self.should_unshare {
            let ret = unsafe { unshare(CLONE_NEWNS) };
            if ret != 0 {
                log::error!("mount ns: unshare failed errno={}", last_errno());
                return false;
            }
        }

        // 重定向同时覆盖 /storage、/data/media 与 /mnt 下的多个存储别名，因此必须
        // 切断两类传播：本命名空间新建的挂载不得外播，存储别名子树不得接受入向传播
        // （MediaProvider 重启/切用户会把真实存储叠到我们的视图层之上）。
        //
        // 这两项目标只需要「全部 shared 挂载 + 存储别名子树」私有化即可达成，不能沿用
        // 整树 `MS_REC|MS_PRIVATE /`：那会把内核里既有的 slave 关系一并切断，使应用
        // 命名空间的 mountinfo 里一行 `master:` 标签都不剩（真机实测 111 行 → 0 行），
        // 成为比任何字符串都明显的结构性差异。见 `mount::propagation` 模块文档。
        if !super::propagation::isolate_storage_mounts(self.user_id) {
            // 复核不通过（仍有 shared 挂载，或存储子树仍带传播关系）：退回整树私有化。
            // 这是安全性兜底，隐藏性因此降级，日志留痕便于定位是哪台设备不满足目标。
            log::warn!(
                "mount ns: storage isolation incomplete pkg={} user={}",
                self.package_name,
                self.user_id
            );
            if !super::propagation::make_tree_private_fallback() {
                return false;
            }
        }

        let mut buf = [0u8; 256];
        let Ok(ns_path) = CString::new("/proc/self/ns/mnt") else {
            return false;
        };
        let len = unsafe { readlink(ns_path.as_ptr(), buf.as_mut_ptr() as *mut _, buf.len() - 1) };
        if len > 0 {
            let text = String::from_utf8_lossy(&buf[..len as usize]);
            log::info!("mount ns ready ns={}", text);
        } else {
            log::warn!("mount ns: readlink failed errno={}", last_errno());
        }

        self.is_namespace_ready = true;
        true
    }

    pub(super) fn ensure_directory_exists(&self, path: &str, should_chown: bool) -> bool {
        if path.is_empty() {
            log::warn!("mount dir: mkdir skipped empty path");
            return false;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            log::debug!("mount dir: skip public storage root mkdir path={}", path);
            return true;
        };
        if !fs::create_directory(&metadata_path, -1) {
            log::warn!(
                "mount dir: mkdir failed path={} metadata_path={}",
                path,
                metadata_path
            );
            return false;
        }

        if should_chown {
            self.ensure_writable_directory_chain(&metadata_path, self.app_uid);
        }

        true
    }

    // 创建或修正映射目录的所有者，使应用可写入
    /// 预建一批路径映射的目标目录（daemon 与 companion 共用）。
    ///
    /// bind 布局在建立映射挂载时由本规划器顺带把目标目录建好；FUSE 布局没有对应的挂载
    /// 动作，策略层只做逐请求解析，因此必须在接入前显式建好——下载提供器等系统代写链路
    /// 会先判定目标父目录是否存在，缺失即放弃写入（CI 场景 39：`Failed to create parent`）。
    ///
    /// 失败只记 debug：目标建不出来时策略仍照常生效，写请求会像以前一样报错，不能因为
    /// 「预建没成功」就把整次挂载判成失败。
    pub(crate) fn prepare_mapping_target_dirs(
        &self,
        mappings: &[crate::domain::PathMapping],
        owner_uid: i32,
    ) {
        for mapping in mappings {
            let target = mapping.final_path.trim_end_matches('/');
            if target.is_empty() {
                continue;
            }
            if !self.ensure_writable_mapped_directory(target, owner_uid) {
                log::debug!(
                    "mapping target dir prepare failed pkg={} path={}",
                    self.package_name,
                    target
                );
            }
        }
    }

    /// 映射目录的「组 / mode」目标值，按路径形态二选一。
    ///
    /// - **私有树**（`Android/data|media|obb/<pkg>` 之下）：回到原生口径——组交给父目录
    ///   setgid 继承（`gid` 传 `u32::MAX` 即「不改」），mode 用私有树原生 `02770`。
    ///   这三支的原生组各不相同（1078 / 1023 / 1079），写死 `media_rw(1023)` 会让目录
    ///   长期偏离原生形态，也让 `owner_migration` 每次开机都要把它改回来。
    /// - **公共目标**（`Download` 等映射目标）：沿用映射目录口径——组 `media_rw(1023)`、
    ///   mode `02771`（other-execute 供直连后端时穿越）。
    fn mapped_dir_target(&self, backend_path: &str) -> (u32, libc::mode_t) {
        if is_android_private_backend_path(backend_path, self.user_id) {
            (u32::MAX, 0o2770)
        } else {
            (MEDIA_RW_GID, MAPPED_DIR_MODE)
        }
    }

    pub(crate) fn ensure_writable_mapped_directory(&self, path: &str, owner_uid: i32) -> bool {
        if path.is_empty() {
            log::warn!("mount dir: metadata skipped empty path");
            return false;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            log::debug!("mount dir: skip public storage root metadata path={}", path);
            return true;
        };
        log::debug!(
            "mount dir prep path={} metadata_path={} owner={}",
            path,
            metadata_path,
            owner_uid
        );
        let is_existing = fs::is_directory(&metadata_path);
        let (target_gid, target_mode) = self.mapped_dir_target(&metadata_path);
        let creation_uid = if crate::metadata_repair::enabled() {
            owner_uid
        } else {
            -1
        };
        if !is_existing && !fs::create_directory(&metadata_path, creation_uid) {
            log::warn!(
                "mount dir: missing and mkdir failed path={} metadata_path={}",
                path,
                metadata_path
            );
            return false;
        }
        // Android 私有外部存储目录由 MediaProvider 按包名和系统版本维护。已有目录的
        // owner/mode 可能是 media_rw、应用 UID 或厂商专用组合，均不能因为一次重定向挂载
        // 被强制改成映射目录的固定值；否则应用会失去原本可用的 data/media/obb 访问权限。
        // 缺失目录仍允许继续走下面的初始化，以便首次创建应用私有目标。
        if is_existing
            && is_android_private_backend_path(&metadata_path, self.user_id)
            && !is_android_private_package_root(&metadata_path, self.user_id)
        {
            log::debug!(
                "mount dir: keep existing Android private metadata path={} metadata_path={}",
                path,
                metadata_path
            );
            return true;
        }
        if !is_existing && self.is_file_monitor_enabled {
            let display_path = self.monitor_display_path_for_backend(&metadata_path);
            crate::logging::write_mount_prep_record(
                &self.package_name,
                &display_path,
                &metadata_path,
            );
        }

        if !crate::metadata_repair::enabled() {
            return true;
        }

        let Ok(c_path) = CString::new(metadata_path.as_str()) else {
            return false;
        };

        // 真实公共目录必须保持 media_rw 所有权；应用可写性由目录模式提供。
        let effective_uid = if owner_uid < 0 {
            None
        } else if is_shared_public_storage_directory(&metadata_path, self.user_id) {
            Some(MEDIA_RW_UID)
        } else {
            Some(owner_uid as u32)
        };

        // 已存在且属主已经就位的目录保持原样，一个元数据系统调用都不发。
        //
        // 真实共享存储里的既有目录（尤其 Android/data|media|obb/<pkg>）由 MediaProvider
        // 按它自己的模式创建和维护，硬覆盖成 MAPPED_DIR_MODE 会换掉既有权限位，反而让
        // 原本能正常访问自有私有目录的应用变成不可写。该函数有多个调用点都指向应用
        // 自己的目标目录，属主已是目标 uid 时目录对应用必然可用，无需再干预；只有目录
        // 是本次新创建、或属主确实与应用不一致时才做完整修正。
        if is_existing && let Some(uid) = effective_uid {
            let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
            // SAFETY: c_path 由 CString 保证以 NUL 结尾；st 是栈上有效存储，stat 只写入该结构。
            if unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) } == 0 {
                // SAFETY: 上一步返回 0 表示 st 已被完整初始化。
                let st = unsafe { st.assume_init() };
                if st.st_uid == uid {
                    log::debug!(
                        "mount dir: keep existing metadata path={} metadata_path={} uid={} mode={:o}",
                        path,
                        metadata_path,
                        uid,
                        st.st_mode & 0o7777
                    );
                    return true;
                }
            }
        }

        if let Some(uid) = effective_uid {
            // SAFETY: chown 只依赖 c_path 指向的有效 NUL 结尾字符串，无借用指针参数；
            // `target_gid` 取 `u32::MAX` 时是 chown 的标准「不改组」语义。
            let ret = unsafe { chown(c_path.as_ptr(), uid, target_gid) };
            if ret != 0 {
                let error_no = last_errno();
                log::warn!(
                    "mount dir: chown failed errno={} {} path={} metadata_path={}",
                    error_no,
                    errno_text(error_no),
                    path,
                    metadata_path
                );
            }
        }

        // 仅在目录新创建或属主刚被修正时才覆盖目标模式。
        // SAFETY: chmod 只依赖 NUL 结尾路径与 mode，`target_mode` 由 mapped_dir_target 派生。
        let ret = unsafe { chmod(c_path.as_ptr(), target_mode) };
        if ret != 0 {
            let error_no = last_errno();
            log::warn!(
                "mount dir: chmod failed errno={} {} path={} metadata_path={}",
                error_no,
                errno_text(error_no),
                path,
                metadata_path
            );
        }

        true
    }

    /// 把映射目标目录归调用应用所有，并把模式收敛到 MAPPED_DIR_MODE。
    ///
    /// 以真实后端为访问路径的设备（锚点 /data/media 直绑等裸权限场景）由内核按真实
    /// 属主与模式位裁决访问：media_rw 属主的目录对应用只剩 other 位，覆盖不了
    /// 「应用创建文件 + 列目录 + 按名访问」的完整语义（真机实测：属主改为应用后
    /// 全部可见）。FUSE 虚拟化设备的应用视图不经过真实属主与模式位，此修正无副作用。
    /// 仅映射目标目录调用本方法；Download 等共享公共目录仍保持 media_rw 所有权。
    /// 幂等：属主已是应用且模式已是 MAPPED_DIR_MODE 时不发任何元数据系统调用。
    pub(super) fn ensure_app_owned_mapping_target(&self, path: &str, owner_uid: i32) -> bool {
        if path.is_empty() || owner_uid < 0 {
            return true;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return true;
        };
        if !fs::is_directory(&metadata_path) {
            // 映射目标目录缺失时必须在这里补建，不能留给后续调用方：应用视图是把
            // 该目录 bind 到请求路径，源不存在时应用侧写入直接 ENOENT；系统代写者
            // （下载提供器）对请求路径的 mkdir 也会因为映射目标不存在被 MediaProvider
            // 判为拒绝，表现成 DownloadManager 的 "Failed to create parent
            // .../Download/SrtDlOwner"（run 40277331532 五版本一致实证：目标目录由
            // 重定向在访问判定之后才被动创建）。
            let creation_uid = if crate::metadata_repair::enabled() {
                owner_uid
            } else {
                -1
            };
            if !fs::create_directory(&metadata_path, creation_uid) {
                log::warn!(
                    "mount dir: mapping target mkdir failed errno={} path={} metadata_path={}",
                    fs::last_create_directory_errno(),
                    path,
                    metadata_path
                );
                return true;
            }
            log::debug!(
                "mount dir: mapping target created path={} metadata_path={} owner={}",
                path,
                metadata_path,
                owner_uid
            );
        }
        let Ok(c_path) = CString::new(metadata_path.as_str()) else {
            return false;
        };
        let mut st = std::mem::MaybeUninit::<c_stat>::uninit();
        // SAFETY: c_path 由 CString 保证以 NUL 结尾；st 是栈上有效存储，stat 只写入该结构。
        if unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) } != 0 {
            return true;
        }
        // SAFETY: 上一步返回 0 表示 st 已被完整初始化。
        let st = unsafe { st.assume_init() };
        let current_mode = st.st_mode & 0o7777;
        let (target_gid, target_mode) = self.mapped_dir_target(&metadata_path);
        if st.st_uid == owner_uid as u32 && current_mode == target_mode {
            return true;
        }
        // SAFETY: chown 只依赖 c_path 指向的有效 NUL 结尾字符串，无借用指针参数；
        // `target_gid` 取 `u32::MAX` 时是 chown 的标准「不改组」语义。
        let chown_ret = unsafe { chown(c_path.as_ptr(), owner_uid as u32, target_gid) };
        if chown_ret != 0 {
            let error_no = last_errno();
            log::warn!(
                "mount dir: mapping target chown failed errno={} {} path={} metadata_path={}",
                error_no,
                errno_text(error_no),
                path,
                metadata_path
            );
        }
        // SAFETY: chmod 只依赖 c_path 指向的有效 NUL 结尾字符串与 mode，无借用指针参数；
        // `target_mode` 由 mapped_dir_target 按路径形态派生。
        let chmod_ret = unsafe { chmod(c_path.as_ptr(), target_mode) };
        if chmod_ret != 0 {
            let error_no = last_errno();
            log::warn!(
                "mount dir: mapping target chmod failed errno={} {} path={} metadata_path={}",
                error_no,
                errno_text(error_no),
                path,
                metadata_path
            );
        }
        log::debug!(
            "mount dir: mapping target app ownership fixed path={} metadata_path={} uid={} mode={:o}",
            path,
            metadata_path,
            owner_uid,
            current_mode
        );
        true
    }

    fn monitor_display_path_for_backend(&self, backend_path: &str) -> String {
        let normalized = paths::normalize(backend_path);
        let storage_root = paths::storage_user_root_for_user(self.user_id);
        let mut candidate_roots = Vec::with_capacity(2);
        let default_target = paths::default_redirect_target(&self.package_name, self.user_id);
        candidate_roots.push(self.to_data_media_backend_path(&default_target));
        if !self.redirect_target.is_empty() {
            candidate_roots.push(self.to_data_media_backend_path(&self.redirect_target));
        }

        for root in candidate_roots {
            if root.is_empty() {
                continue;
            }
            if let Some(relative) = paths::relative_child_path(&normalized, &root) {
                return paths::join(&storage_root, relative);
            }
        }

        paths::data_media_to_storage_path(&normalized)
    }

    pub(super) fn ensure_read_only_tree_accessible(&self, path: &str) {
        let Some(root) = self.metadata_operations_path(path) else {
            return;
        };
        if !is_data_media_shared_public_directory(&root, self.user_id) || !fs::is_directory(&root) {
            return;
        }

        let mut visited = 0usize;
        let mut pending = VecDeque::from([root.clone()]);
        while let Some(current) = pending.pop_front() {
            if visited >= READ_ONLY_TREE_METADATA_LIMIT {
                log::warn!(
                    "mount dir: readonly metadata scan limit reached root={} limit={}",
                    path,
                    READ_ONLY_TREE_METADATA_LIMIT
                );
                break;
            }
            visited += 1;

            fix_read_only_public_metadata(&current);

            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let child = entry.path().to_string_lossy().replace('\\', "/");
                if child.is_empty() {
                    continue;
                }
                if entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false) {
                    pending.push_back(child);
                } else {
                    fix_read_only_public_metadata(&child);
                }
            }
        }
    }

    pub(super) fn ensure_app_writable_directory_chain(&self, path: &str, owner_uid: i32) {
        self.ensure_writable_directory_chain(path, owner_uid);
    }

    /// 把**本次新建**的私有树目录归位成原生形态：属主是应用、组保持父目录 setgid 继承、
    /// mode 原生 `02770`。
    ///
    /// 为什么必须在这里补：`create_directory` 以 `uid < 0` 建目录时只 mkdir 不改属主
    /// （留下 root），而紧随其后的 `ensure_writable_mapped_directory` 会因「既有私有目录
    /// 不得改写」整体跳过——两者叠加就把重定向根落成 `root:media_rw 2750`：组位只有 r-x，
    /// 系统代写进程（MediaProvider，附加组 media_rw/ext_data_rw）在沙盒里建不出目录，
    /// 写入回退到真实公共路径（2026-11-10 真机：场景 41 重建沙盒根后实测该形态）。
    ///
    /// 只对该次新建的目录调用；既有目录仍由系统原生语义与 owner_migration 负责，不在挂载
    /// 流程里重复改写。组不写死常量：`Android/data|media|obb` 的原生组由父目录 setgid 继承，
    /// 父目录给什么就是什么。
    pub(super) fn ensure_native_private_dir_owner(&self, path: &str, owner_uid: i32) {
        if !crate::metadata_repair::enabled() || owner_uid < 0 {
            return;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return;
        };
        if !is_android_private_backend_path(&metadata_path, self.user_id) {
            return;
        }
        let Ok(c_path) = CString::new(metadata_path.as_str()) else {
            return;
        };
        // SAFETY: c_path 以 NUL 结尾并在调用期间保持有效；gid 传 -1 表示不改组。
        let chown_ret = unsafe { chown(c_path.as_ptr(), owner_uid as u32, u32::MAX) };
        if chown_ret != 0 {
            log::warn!(
                "mount dir: native private chown failed errno={} path={}",
                last_errno(),
                metadata_path
            );
        }
        // SAFETY: 同上，chmod 只依赖 NUL 结尾路径与 mode。
        let chmod_ret = unsafe { chmod(c_path.as_ptr(), 0o2770) };
        if chmod_ret != 0 {
            log::warn!(
                "mount dir: native private chmod failed errno={} path={}",
                last_errno(),
                metadata_path
            );
        }
    }

    pub(super) fn ensure_real_public_directory_exists(&self, path: &str) -> bool {
        if path.is_empty() {
            log::warn!("mount dir: real public mkdir skipped empty path");
            return false;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            log::debug!("mount dir: skip public storage root real path={}", path);
            return true;
        };
        let should_fix_public_metadata =
            is_data_media_shared_public_directory(&metadata_path, self.user_id);
        let uid = if should_fix_public_metadata && crate::metadata_repair::enabled() {
            MEDIA_RW_UID as i32
        } else {
            -1
        };
        if !fs::create_directory(&metadata_path, uid) {
            log::warn!(
                "mount dir: real public mkdir failed path={} metadata_path={}",
                path,
                metadata_path
            );
            return false;
        }

        if should_fix_public_metadata && crate::metadata_repair::enabled() {
            self.fix_real_public_directory_metadata_chain(&metadata_path);
            self.fix_allowed_real_directory_metadata_chain(&metadata_path);
        }

        true
    }

    pub(super) fn ensure_allowed_real_existing_directory_tree_writable(
        &self,
        path: &str,
        excluded_real_paths: &[String],
    ) {
        let Some(root) = self.metadata_operations_path(path) else {
            return;
        };
        if !is_data_media_shared_public_directory(&root, self.user_id) || !fs::is_directory(&root) {
            return;
        }

        let mut visited = 0usize;
        let mut pending = VecDeque::from([root]);
        while let Some(current) = pending.pop_front() {
            if visited >= ALLOWED_REAL_TREE_METADATA_LIMIT {
                log::warn!(
                    "mount dir: allowed real metadata scan limit reached root={} limit={}",
                    path,
                    ALLOWED_REAL_TREE_METADATA_LIMIT
                );
                break;
            }
            visited += 1;

            if allowed_real_metadata_path_is_excluded(&current, excluded_real_paths) {
                continue;
            }
            if !is_data_media_shared_public_directory(&current, self.user_id) {
                continue;
            }
            if fs::is_directory(&current) {
                fix_allowed_real_directory_metadata(&current);
            }

            let Ok(entries) = std::fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                if !file_type.is_dir() {
                    continue;
                }
                let child = entry.path().to_string_lossy().replace('\\', "/");
                if !child.is_empty() {
                    pending.push_back(child);
                }
            }
        }
    }

    fn fix_allowed_real_directory_metadata_chain(&self, path: &str) {
        let root = paths::data_media_user_root_for_user(self.user_id);
        let original = path.to_string();
        let mut current = original.clone();
        while !current.is_empty() {
            if current == root {
                break;
            }

            if should_apply_allowed_real_writable_metadata(&current, &original, self.user_id)
                && fs::is_directory(&current)
            {
                fix_allowed_real_directory_metadata(&current);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    fn fix_real_public_directory_metadata_chain(&self, path: &str) {
        let mut current = path.to_string();
        while !current.is_empty() {
            if is_data_media_shared_public_directory(&current, self.user_id)
                && fs::is_directory(&current)
            {
                fix_real_public_directory_metadata(&current);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    pub(super) fn ensure_shared_mapping_parent_chain(&self, path: &str) {
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return;
        };
        let Some(stop_dir) = shared_storage_root(&metadata_path) else {
            return;
        };

        let mut current = parent_preserving_backend_alias(&metadata_path);
        while !current.is_empty() {
            if current == stop_dir {
                break;
            }
            if is_android_private_storage_path(&current, &stop_dir) {
                break;
            }

            if crate::metadata_repair::enabled() && fs::is_directory(&current) {
                self.ensure_shared_mapping_directory(&current);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    fn ensure_shared_mapping_directory(&self, path: &str) {
        if !crate::metadata_repair::enabled() {
            return;
        }
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return;
        };
        let Ok(c_path) = CString::new(metadata_path.as_str()) else {
            return;
        };

        let mut st = std::mem::MaybeUninit::<libc::stat>::uninit();
        let ret = unsafe { c_stat(c_path.as_ptr(), st.as_mut_ptr()) };
        let existing_stat = if ret == 0 {
            Some(unsafe { st.assume_init() })
        } else {
            let error_no = last_errno();
            log::warn!(
                "mount dir: shared parent stat failed errno={} {} path={} metadata_path={}",
                error_no,
                errno_text(error_no),
                path,
                metadata_path
            );
            None
        };

        if let Some(st) = existing_stat {
            let ret = unsafe { chown(c_path.as_ptr(), st.st_uid, MEDIA_RW_GID) };
            if ret != 0 {
                let error_no = last_errno();
                log::warn!(
                    "mount dir: shared parent chgrp failed errno={} {} path={} metadata_path={}",
                    error_no,
                    errno_text(error_no),
                    path,
                    metadata_path
                );
            }

            let mode = (st.st_mode as libc::mode_t) & 0o7777;
            let fixed_mode = mode | 0o2010;
            if fixed_mode != mode {
                let ret = unsafe { chmod(c_path.as_ptr(), fixed_mode) };
                if ret != 0 {
                    let error_no = last_errno();
                    log::warn!(
                        "mount dir: shared parent chmod failed errno={} {} path={} metadata_path={}",
                        error_no,
                        errno_text(error_no),
                        path,
                        metadata_path
                    );
                }
            }
        }
    }

    fn ensure_writable_directory_chain(&self, path: &str, owner_uid: i32) {
        let Some(metadata_path) = self.metadata_operations_path(path) else {
            return;
        };
        let stop_dir = paths::storage_user_root(&metadata_path)
            .or_else(|| paths::data_media_user_root(&metadata_path));
        let mut current = metadata_path;
        while !current.is_empty() {
            if let Some(stop) = stop_dir.as_deref()
                && current == stop
            {
                break;
            }

            if fs::is_directory(&current)
                && should_apply_app_writable_metadata(&current, self.user_id)
            {
                let _ = self.ensure_writable_mapped_directory(&current, owner_uid);
            }

            let parent = parent_preserving_backend_alias(&current);
            if parent.is_empty() || parent == "/" || parent == current {
                break;
            }
            current = parent;
        }
    }

    pub(super) fn to_data_media_backend_path(&self, storage_path: &str) -> String {
        paths::storage_to_data_media_for_user(storage_path, self.user_id).unwrap_or_default()
    }

    pub(super) fn normalize_path(&self, path: &str) -> String {
        paths::normalize(path)
    }

    pub(super) fn resolve_user_path(&self, path: &str) -> String {
        paths::resolve_user_path(path, self.user_id)
    }

    pub(super) fn resolve_placeholders(&self, path: &str) -> String {
        let has_redirect_placeholder =
            path.contains("${REDIRECT_TARGET}") || path.contains("$REDIRECT_TARGET");
        if has_redirect_placeholder && self.redirect_target.is_empty() {
            log::warn!(
                "mount path: redirect target missing, cannot expand placeholder path={}",
                path
            );
        }

        paths::resolve_placeholders(path, &self.app_data_dir, &self.redirect_target)
    }

    pub(super) fn bind_mount(&self, source: &str, target: &str, is_recursive: bool) -> bool {
        self.bind_mount_inner(source, target, is_recursive, true)
    }

    pub(super) fn bind_mount_overlay(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
    ) -> bool {
        self.bind_mount_inner(source, target, is_recursive, false)
    }

    pub(super) fn bind_mount_read_write_overlay(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
    ) -> bool {
        let use_recursive = self.should_use_recursive_bind(source, target, is_recursive);
        let previously_recorded = self
            .mounted_targets
            .borrow()
            .iter()
            .any(|item| item == target);
        if !self.bind_mount_overlay(source, target, use_recursive) {
            return false;
        }

        if self.remount_bind_read_write(target, use_recursive) {
            log::info!("readwrite bind ok src={} dst={}", source, target);
            return true;
        }

        let Ok(c_target) = CString::new(target) else {
            return false;
        };
        // SAFETY: c_target 为本次绑定目标的有效 C 字符串，调用期间保持存活。
        let ret = unsafe { umount2(c_target.as_ptr(), MNT_DETACH) };
        if ret == 0 && !previously_recorded {
            // 只撤销本次新增记录，保留同一路径上先前已成功安装的下层挂载。
            self.mounted_targets
                .borrow_mut()
                .retain(|item| item != target);
        }
        if ret != 0 {
            let error_no = last_errno();
            log::warn!(
                "readwrite bind cleanup failed dst={} errno={} {}",
                target,
                error_no,
                errno_text(error_no)
            );
        }
        false
    }

    /// 以**应用的身份**执行一段路径相关操作：切 fsuid + 补上 `everybody` 组。
    ///
    /// 为什么两个都要切（真机 2026-11-04）：
    /// - `fsuid`：视图是 FUSE，会话只按 uid 登记策略；用 root 解析视图内路径会被 fail-closed 兜底拒。
    /// - 补充组：`/storage` 是 `drwx--x--- shell everybody`（0710），应用因为在 `everybody(9997)`
    ///   组里才能穿过；工作进程只有 root 的组，于是遍历 `/storage` 直接 EACCES —— 这正是
    ///   `bind via fds: target open failed … errno=13 app_uid=10284` 的成因。
    ///
    /// 只切 fsuid 与组：挂载权限仍由 euid=0 的 capability 保证，`mount(2)`/`move_mount(2)` 不受影响。
    fn with_app_credentials<T>(&self, action: impl FnOnce() -> T) -> T {
        // SAFETY: getgroups 只读取当前进程的补充组。
        let mut previous = [0 as libc::gid_t; 64];
        let previous_count =
            unsafe { libc::getgroups(previous.len() as libc::c_int, previous.as_mut_ptr()) };
        // SAFETY: setfsuid 只影响文件权限检查使用的 fsuid，返回旧值用于恢复。
        let prev_fsuid = unsafe { libc::syscall(libc::SYS_setfsuid, self.app_uid as u32) as i64 };
        // SAFETY: setgroups 需要 CAP_SETGID（euid 仍为 0），失败不致命：退化为仅切 fsuid。
        let groups = [AID_EVERYBODY];
        unsafe { libc::setgroups(groups.len(), groups.as_ptr()) };
        let output = action();
        // SAFETY: 恢复补充组与 fsuid，顺序与设置相反。
        if previous_count > 0 {
            unsafe { libc::setgroups(previous_count as usize, previous.as_ptr()) };
        }
        // SAFETY: 恢复调用前的 fsuid。
        unsafe { libc::syscall(libc::SYS_setfsuid, prev_fsuid as u32) };
        output
    }

    /// 用 fd 搬运完成绑定：源以 root 打开（`/data/adb` 只有 root 能进），目标以**应用凭据**
    /// 打开（视图只有应用 uid 的策略能解析，且 `/storage` 需要 everybody 组），再 `move_mount`
    /// 把两者接起来。`mount(MS_BIND)` 做不到这件事：它用同一份凭据解析两侧。
    fn bind_mount_via_fds(&self, source: &str, target: &str, recursive: bool) -> bool {
        let Ok(c_source) = CString::new(source) else {
            return false;
        };
        let mut open_flags = OPEN_TREE_CLONE;
        if recursive {
            open_flags |= AT_RECURSIVE;
        }
        // SAFETY: c_source 是 NUL 结尾的合法路径；open_tree 只克隆挂载，不改动源。
        let src_fd = unsafe {
            libc::syscall(
                libc::SYS_open_tree,
                libc::AT_FDCWD,
                c_source.as_ptr(),
                open_flags as libc::c_uint,
            ) as libc::c_int
        };
        if src_fd < 0 {
            log::debug!(
                "bind via fds: open_tree failed src={} errno={}",
                source,
                last_errno()
            );
            return false;
        }
        let dst_fd = self.with_app_credentials(|| {
            let Ok(c_target) = CString::new(target) else {
                return -1;
            };
            // O_PATH|O_DIRECTORY|O_CLOEXEC：只取目录句柄，不触发 FUSE 的 OPEN。
            const O_PATH: libc::c_int = 0o10000000;
            const O_DIRECTORY: libc::c_int = 0o200000;
            // SAFETY: c_target 是 NUL 结尾的合法路径。
            unsafe {
                libc::open(c_target.as_ptr(), O_PATH | O_DIRECTORY | libc::O_CLOEXEC) as libc::c_int
            }
        });
        if dst_fd < 0 {
            log::debug!(
                "bind via fds: target open failed dst={} errno={} app_uid={}",
                target,
                last_errno(),
                self.app_uid
            );
            // SAFETY: src_fd 由上方 open_tree 打开，此处是唯一清理点。
            unsafe { libc::close(src_fd) };
            return false;
        }
        let empty = c"";
        // SAFETY: src_fd/dst_fd 都是有效 fd；两端都走 EMPTY_PATH，不做路径解析。
        let ret = unsafe {
            libc::syscall(
                libc::SYS_move_mount,
                src_fd,
                empty.as_ptr(),
                dst_fd,
                empty.as_ptr(),
                (MOVE_MOUNT_F_EMPTY_PATH | MOVE_MOUNT_T_EMPTY_PATH) as libc::c_uint,
            ) as libc::c_int
        };
        let errno = last_errno();
        // SAFETY: 两个 fd 由本函数打开，搬运用完即关。
        unsafe {
            libc::close(src_fd);
            libc::close(dst_fd);
        }
        if ret != 0 {
            log::debug!(
                "bind via fds: move_mount failed src={} dst={} errno={} {}",
                source,
                target,
                errno,
                errno_text(errno)
            );
            return false;
        }
        true
    }

    fn bind_mount_inner(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
        allow_same_inode_shortcut: bool,
    ) -> bool {
        let use_recursive = self.should_use_recursive_bind(source, target, is_recursive);
        if allow_same_inode_shortcut && paths_have_same_inode(source, target) {
            // 只有目标本来就是挂载点时，remount 成可写才有意义。目标只是普通目录
            // （源与目标本身指向同一目录时就是这种情况）时，`MS_BIND | MS_REMOUNT`
            // 必然返回 EINVAL；先用一次挂载表查询把它筛掉，再走下面的直接绑定。
            let target_is_mount_point = is_mount_point(target);
            // 目标最上层若是 FUSE 层（MediaProvider 视图、`root` 指向沙箱），不能走
            // remount 短路：remount 只改权限、不换源，会保留这层被 MediaProvider 拒绝
            // 访问的 FUSE 视图（Android 13 场景 29 热重载后视图根列举为空、所有路径
            // ENOENT）。此时必须跳过短路，走下面正常的 ext4 绑定，在 FUSE 层之上叠一层
            // 可访问的 ext4 视图。
            let target_is_fuse_layer = target_is_mount_point && topmost_mount_is_fuse(target);
            if target_is_mount_point
                && !target_is_fuse_layer
                && self.remount_bind_read_write(target, use_recursive)
            {
                self.record_mounted_target(target);
                log::debug!("bind skip existing src={} dst={}", source, target);
                return true;
            }
            if target_is_fuse_layer {
                log::info!(
                    "bind skip shortcut on fuse layer, force rebind src={} dst={}",
                    source,
                    target
                );
            } else if target_is_mount_point {
                log::warn!(
                    "bind existing remount rw failed, retry bind src={} dst={}",
                    source,
                    target
                );
            } else {
                log::debug!(
                    "bind same inode target not mounted, bind directly src={} dst={}",
                    source,
                    target
                );
            }
        }

        let Ok(c_source) = CString::new(source) else {
            return false;
        };
        let Ok(c_target) = CString::new(target) else {
            return false;
        };

        let mut flags = MS_BIND;
        if use_recursive {
            flags |= MS_REC;
        }

        // 记录绑定前的目标 inode，供 FUSE 承载路径的落地校验判断「这次 stat 读到的究竟
        // 是绑定后的视图还是绑定前那一层」。目标本来不存在时保持 None，调用方会跳过
        // 「未变化」比较、只按非目录这一事实判定。
        *self.pre_bind_target_inode.borrow_mut() = stat_inode(target);

        // 优先用 fd 搬运：`mount(MS_BIND)` 无法同时满足源（root 可见）与目标（应用凭据可见）
        // 的要求，见 `bind_mount_via_fds`。失败再回落到传统路径绑定。
        let mut ret = if self.bind_mount_via_fds(source, target, use_recursive) {
            0
        } else {
            // SAFETY: c_source/c_target 为 NUL 结尾的合法路径，调用期间保持存活；
            // 这是回落路径，语义与改动前完全一致。
            unsafe {
                mount(
                    c_source.as_ptr(),
                    c_target.as_ptr(),
                    std::ptr::null(),
                    flags as libc::c_ulong,
                    std::ptr::null(),
                )
            }
        };
        if ret != 0 {
            let errno = last_errno();
            // bind 失败的真实 errno 必须可回查：自有私有恢复等调用方在失败日志里
            // 报的是 create_directory 的陈旧 errno（真机上表现为误导性的 errno=0）。
            crate::mount::own_private_pending::note_bind_errno(errno);
            if errno == libc::ENOTCONN && self.repair_disconnected_storage_parent(target) {
                ret = unsafe {
                    mount(
                        c_source.as_ptr(),
                        c_target.as_ptr(),
                        std::ptr::null(),
                        flags as libc::c_ulong,
                        std::ptr::null(),
                    )
                };
                if ret == 0 {
                    log::warn!(
                        "bind retry ok after storage parent repair src={} dst={}",
                        source,
                        target
                    );
                }
            }
        }
        if ret != 0 {
            let error_no = last_errno();
            log::error!(
                "bind mount failed src={} dst={} errno={} {}",
                source,
                target,
                error_no,
                errno_text(error_no)
            );
            return false;
        }

        let is_fuse_backed = self.is_public_storage_alias_path(source)
            || self.is_public_storage_alias_path(target)
            || self.is_real_storage_anchor_path(source)
            || self.is_real_storage_anchor_path(target);

        // 记账前必须确认这层绑定在**应用行走的视图里**真的把目标变成了一个目录。
        //
        // mount(MS_BIND) 返回 0 只说明内核接受了这次绑定，不说明应用后续 lookup 能命中：
        // 公共存储别名路径与真实存储锚点都压在系统 FUSE 的挂载点上，绑定源若取自本模块
        // 沙箱锚点，就会出现「mountinfo 里有这层、应用行走进不去」的分离（Android 13
        // 场景 29 实测）。旧实现直接 record_mounted_target 并 return true，把「syscall
        // 成功」当成「落地有效」写进状态文件与挂载身份账本，使后续所有自检都继承这个
        // 乐观结论，无法区分「挂载在、应用能命中」与「挂载在、应用走不进去」。
        //
        // 这里只把「目标不再是目录」这一确定无歧义的失效形态拦下来（应用侧表现为
        // ENOTDIR）；判定要求 source/target 的 st_dev 与 st_ino 都不同于绑定前，避免把
        // 「绑定未落地」误报成「绑定失败」。root 视角会被 FUSE 放行，因此本判定成功
        // **不**代表应用可见，只代表已排除「目标被换成非目录」这一形态。
        if is_fuse_backed {
            let verified = self.verify_fuse_backed_bind_landing(source, target);
            if !verified {
                return false;
            }
            log::debug!(
                "bind verify fuse-backed verified src={} dst={}",
                source,
                target
            );
            self.record_mounted_target(target);
            let bind_count = BIND_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if should_log_step(bind_count, BIND_SUCCESS_LOG_STEP) {
                log::debug!("bind ok src={} dst={} n={}", source, target, bind_count);
            }
            log::debug!(
                "bind verify skipped fuse-backed path src={} dst={}",
                source,
                target
            );
            return true;
        }

        self.record_mounted_target(target);

        let bind_count = BIND_SUCCESS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if should_log_step(bind_count, BIND_SUCCESS_LOG_STEP) {
            log::debug!("bind ok src={} dst={} n={}", source, target, bind_count);
        }
        let Ok(c_source_stat) = CString::new(source) else {
            return true;
        };
        let Ok(c_target_stat) = CString::new(target) else {
            return true;
        };
        let mut st_source = std::mem::MaybeUninit::<c_stat>::uninit();
        let mut st_target = std::mem::MaybeUninit::<c_stat>::uninit();
        let is_source_ok =
            unsafe { libc::stat(c_source_stat.as_ptr(), st_source.as_mut_ptr()) } == 0;
        let is_target_ok =
            unsafe { libc::stat(c_target_stat.as_ptr(), st_target.as_mut_ptr()) } == 0;
        if is_source_ok && is_target_ok {
            let st_source = unsafe { st_source.assume_init() };
            let st_target = unsafe { st_target.assume_init() };
            let is_same =
                st_source.st_dev == st_target.st_dev && st_source.st_ino == st_target.st_ino;
            if is_same {
                let verify_ok_count = BIND_VERIFY_PASS_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                if should_log_step(verify_ok_count, BIND_VERIFY_PASS_LOG_STEP) {
                    log::debug!(
                        "bind verify ok src={} dst={} n={}",
                        source,
                        target,
                        verify_ok_count
                    );
                }
            } else {
                log::warn!(
                    "bind verify mismatch src={} dst={} sdev={} tdev={} sino={} tino={}",
                    source,
                    target,
                    st_source.st_dev,
                    st_target.st_dev,
                    st_source.st_ino,
                    st_target.st_ino
                );
            }
        } else {
            log::warn!(
                "bind verify failed src={} dst={} reason=stat",
                source,
                target
            );
        }
        true
    }

    fn should_use_recursive_bind(&self, source: &str, target: &str, requested: bool) -> bool {
        if !requested {
            return false;
        }
        // 应用自己的 Android/data|media|obb/<包名> 必须递归绑定。包名根下的
        // Tencent/QQfile_recv 等目录可能在挂载后才出现；非递归 bind 只覆盖空的
        // 包名根，应用随后访问子目录会得到 ENOENT。该例外仍限定在当前应用的
        // 私有包名根，不会放开公共存储树的递归绑定。
        if self.is_android_private_package_target(target) {
            return true;
        }
        if self.is_public_storage_alias_path(source)
            || self.is_public_storage_alias_path(target)
            || self.is_real_storage_anchor_path(source)
            || self.is_real_storage_anchor_path(target)
            || paths::data_media_user_root(source).is_some()
            || paths::data_media_user_root(target).is_some()
        {
            log::debug!(
                "bind recursive disabled for storage tree src={} dst={}",
                source,
                target
            );
            return false;
        }
        true
    }

    fn is_android_private_package_target(&self, path: &str) -> bool {
        let normalized = paths::normalize(path);
        ["data", "media", "obb"].iter().any(|kind| {
            let suffix = format!("/Android/{}/{}", kind, self.package_name);
            normalized.ends_with(&suffix)
        })
    }

    fn repair_disconnected_storage_parent(&self, target: &str) -> bool {
        let Some((source, mount_point)) = storage_emulated_repair_mount(target, self.user_id)
        else {
            return false;
        };
        if !fs::is_directory(&source) {
            log::warn!(
                "storage parent repair skipped source missing src={} dst={} target={}",
                source,
                mount_point,
                target
            );
            return false;
        }

        let Ok(c_source) = CString::new(source.as_str()) else {
            return false;
        };
        let Ok(c_mount_point) = CString::new(mount_point.as_str()) else {
            return false;
        };

        let _ = unsafe { umount2(c_mount_point.as_ptr(), MNT_DETACH) };
        let ret = unsafe {
            mount(
                c_source.as_ptr(),
                c_mount_point.as_ptr(),
                std::ptr::null(),
                MS_BIND as libc::c_ulong,
                std::ptr::null(),
            )
        };
        if ret != 0 {
            let error_no = last_errno();
            log::warn!(
                "storage parent repair failed src={} dst={} target={} errno={} {}",
                source,
                mount_point,
                target,
                error_no,
                errno_text(error_no)
            );
            return false;
        }

        self.record_mounted_target(&mount_point);
        log::warn!(
            "storage parent repaired src={} dst={} target={}",
            source,
            mount_point,
            target
        );
        true
    }

    pub(super) fn bind_mount_read_only(
        &self,
        source: &str,
        target: &str,
        is_recursive: bool,
    ) -> bool {
        let use_recursive = self.should_use_recursive_bind(source, target, is_recursive);
        let previously_recorded = self
            .mounted_targets
            .borrow()
            .iter()
            .any(|item| item == target);
        if !self.bind_mount(source, target, use_recursive) {
            return false;
        }

        if self.remount_bind_read_only(target, use_recursive) {
            log::info!("readonly bind ok src={} dst={}", source, target);
            return true;
        }

        let Ok(c_target) = CString::new(target) else {
            return false;
        };
        // SAFETY: c_target 为本次绑定目标的有效 C 字符串，调用期间保持存活。
        let ret = unsafe { umount2(c_target.as_ptr(), MNT_DETACH) };
        if ret == 0 && !previously_recorded {
            // 只撤销本次新增记录，保留同一路径上先前已成功安装的下层挂载。
            self.mounted_targets
                .borrow_mut()
                .retain(|item| item != target);
        }
        if ret != 0 {
            let error_no = last_errno();
            log::warn!(
                "readonly bind cleanup failed dst={} errno={} {}",
                target,
                error_no,
                errno_text(error_no)
            );
        }
        false
    }

    fn remount_bind_read_only(&self, target: &str, is_recursive: bool) -> bool {
        if remount_bind_read_only_inner(target, is_recursive) {
            return true;
        }
        if is_recursive && remount_bind_read_only_inner(target, false) {
            log::warn!("readonly remount recursive fallback ok dst={}", target);
            return true;
        }
        false
    }

    fn remount_bind_read_write(&self, target: &str, is_recursive: bool) -> bool {
        if remount_bind_read_write_inner(target, is_recursive) {
            return true;
        }
        if !is_recursive && remount_bind_read_write_inner(target, true) {
            log::warn!("readwrite remount recursive fallback ok dst={}", target);
            return true;
        }
        if is_recursive && remount_bind_read_write_inner(target, false) {
            log::warn!("readwrite remount nonrecursive fallback ok dst={}", target);
            return true;
        }
        false
    }

    fn record_mounted_target(&self, target: &str) {
        if target.is_empty() {
            return;
        }
        let mut targets = self.mounted_targets.borrow_mut();
        if !targets.iter().any(|item| item == target) {
            targets.push(target.to_string());
        }
    }

    /// 校验一次已返回成功的 FUSE 承载绑定是否真的把 `target` 换成了目录。
    ///
    /// 返回 `false` 时调用方必须把这层绑定当作不成立：不计账、不上报成功。
    /// 判定委托给 [`fuse_backed_bind_landing_verdict`]，这里只负责采样与日志。
    fn verify_fuse_backed_bind_landing(&self, source: &str, target: &str) -> bool {
        // 绑定前没有可读 inode 时传 `None`：此时不做「未变化」比较，只按非目录这一事实判定。
        let before = *self.pre_bind_target_inode.borrow();
        let target_is_directory = stat_is_directory(target);
        let target_inode_after = stat_inode(target);
        let sample = BindLandingSample {
            target_is_directory,
            target_inode_after,
            target_inode_before: before,
        };
        match fuse_backed_bind_landing_verdict(sample) {
            BindLandingVerdict::Accepted => {
                log::debug!(
                    "bind verify fuse-backed verified src={} dst={}",
                    source,
                    target
                );
                true
            }
            BindLandingVerdict::Inconclusive => {
                // stat 读不出目录性，或读数前后未变化：沿用原有「FUSE 承载路径不做 inode
                // 校验」的口径，按成功放行。这里刻意不误杀挂载——无法判定不等于失败。
                log::debug!(
                    "bind verify fuse-backed inconclusive src={} dst={} reason={}",
                    source,
                    target,
                    if target_is_directory.is_none() {
                        "stat_failed"
                    } else {
                        "inode_unchanged"
                    }
                );
                true
            }
            BindLandingVerdict::NotDirectory => {
                let source_inode = stat_inode(source);
                log::error!(
                    "bind verify fuse-backed not directory src={} dst={} sdev={} sino={} tdev={} tino={}",
                    source,
                    target,
                    source_inode.map_or(0, |item| item.0),
                    source_inode.map_or(0, |item| item.1),
                    target_inode_after.map_or(0, |item| item.0),
                    target_inode_after.map_or(0, |item| item.1)
                );
                false
            }
        }
    }

    // 映射目标源以锚点路径形式返回时，map 侧也需要经此换算到真实后端再做元数据
    // 判定，因此对同层模块可见。
    pub(super) fn metadata_operations_path(&self, path: &str) -> Option<String> {
        if path.is_empty() {
            return None;
        }
        if let Some(backend_path) = self.real_storage_anchor_backend_path(path) {
            return Some(backend_path);
        }
        if self.is_storage_root_redirected && self.is_public_storage_alias_path(path) {
            return Some(path.to_string());
        }
        if let Some(backend_path) = paths::storage_to_data_media_for_user(path, self.user_id) {
            return Some(backend_path);
        }
        if self.is_user_storage_alias_path(path) {
            return None;
        }
        Some(path.to_string())
    }

    fn is_user_storage_alias_path(&self, path: &str) -> bool {
        if path.is_empty() {
            return false;
        }
        let normalized = paths::normalize(path);
        paths::is_same_or_child(
            &normalized,
            &paths::storage_user_root_for_user(self.user_id),
        )
    }

    fn is_public_storage_alias_path(&self, path: &str) -> bool {
        if path.starts_with("/data/media/") || path == "/data/media" {
            return false;
        }
        self.is_user_storage_alias_path(path)
    }

    fn real_storage_anchor_backend_path(&self, path: &str) -> Option<String> {
        let anchor_root = self.real_storage_anchor_root();
        let relative = paths::relative_child_path(path, &anchor_root)?;
        if relative.is_empty() {
            return None;
        }
        Some(paths::join(
            &paths::data_media_user_root_for_user(self.user_id),
            relative,
        ))
    }

    fn is_real_storage_anchor_path(&self, path: &str) -> bool {
        paths::is_same_or_child(path, &self.real_storage_anchor_root())
    }

    /// 锚点的**可用根**（内容 = 该用户存储根），与建立时的口径同源。
    fn real_storage_anchor_root(&self) -> String {
        super::apply_targets::real_storage_anchor_usable_path(self.user_id)
    }
}
