use crate::domain::{PathMapping, sort_path_mappings_shortest_request_first};
use crate::platform::{fs, module_paths, paths};
use fuser::{
    AccessFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, InitFlags, KernelConfig, MountOption, OpenAccMode, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs,
    ReplyWrite, Request, SessionACL, TimeOrNow, WriteFlags,
};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_millis(250);
const ROOT_INO: u64 = 1;
const MAX_READ_SIZE: usize = 256 * 1024;
const MEDIA_RW_GID: u32 = 1023;
const MAPPED_DIR_MODE: libc::mode_t = 0o2773;
const SHARED_PUBLIC_DIR_MODE: u32 = 0o2770;
const MAX_SCOPED_FUSE_ROOTS: usize = 4;
const FILE_MONITOR_LOG_TAG: &str = "FileMonitorOp";
const READ_ONLY_DENY_EXTRA: &str = "deny_reason=read_only_rule";
const DUPLICATE_MONITOR_CREATE_WINDOW_MS: i64 = 1500;
const MAX_RECENT_MONITOR_CREATES: usize = 256;

static RECENT_MONITOR_CREATES: Lazy<Mutex<HashMap<String, i64>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
pub struct FuseRedirectConfig {
    pub package_name: String,
    pub uid: i32,
    pub app_data_dir: String,
    pub redirect_target: String,
    pub mount_root: Option<String>,
    pub real_root_override: Option<String>,
    pub is_file_monitor_enabled: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
    pub is_mapping_mode_only: bool,
}

impl FuseRedirectConfig {
    fn user_id(&self) -> i32 {
        crate::platform::user_id_from_uid(self.uid)
    }
}

pub fn mount_blocking_with_ready(
    config: FuseRedirectConfig,
    ready_sock: Option<libc::c_int>,
) -> bool {
    let user_id = config.user_id();
    let mount_point = fuse_mount_point(&config, user_id);
    let metadata_dir = mount_point_metadata_dir(&mount_point, user_id);
    if !fs::create_directory(&metadata_dir, config.uid) {
        log::error!(
            "fuse redirect mount point missing: {} metadata={}",
            mount_point,
            metadata_dir
        );
        send_ready_result(ready_sock, -1);
        return false;
    }

    let fs = match FuseRedirectFs::new(config) {
        Some(fs) => fs,
        None => {
            send_ready_result(ready_sock, -1);
            return false;
        }
    };
    let mut mount_options = fuser::Config::default();
    mount_options.mount_options = vec![
        MountOption::FSName("srx_fuse_redirect".to_string()),
        MountOption::Subtype("srx".to_string()),
        MountOption::RW,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoAtime,
        MountOption::Async,
    ];
    mount_options.acl = SessionACL::All;
    mount_options.n_threads = Some(4);
    mount_options.clone_fd = true;

    log::info!(
        "fuse redirect mount start pkg={} uid={} user={} mp={} rel={} real={} map_only={} allow={} excl={} sandbox={} ro={} map={}",
        fs.policy.package_name,
        fs.policy.uid,
        user_id,
        mount_point,
        fs.policy.mount_rel,
        fs.policy.real_root.display(),
        fs.policy.is_mapping_mode_only,
        fs.policy.allowed_real_paths.len(),
        fs.policy.excluded_real_paths.len(),
        fs.policy.sandboxed_paths.len(),
        fs.policy.read_only_paths.len(),
        fs.policy.path_mappings.len()
    );

    match fuser::mount2_with_ready(fs, &mount_point, &mount_options, |ready| {
        send_ready_result(ready_sock, if ready { 0 } else { -1 });
    }) {
        Ok(()) => true,
        Err(error) => {
            log::warn!(
                "fuse redirect session ended with error mp={} err={}",
                mount_point,
                error
            );
            false
        }
    }
}

fn fuse_mount_point(config: &FuseRedirectConfig, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(raw_mount_root) = config.mount_root.as_deref() else {
        return storage_root;
    };
    let mut mount_root = paths::resolve_user_path(&paths::normalize(raw_mount_root), user_id);
    if !paths::is_absolute(&mount_root) {
        mount_root = paths::normalize(&paths::join(&storage_root, &mount_root));
    }
    if paths::eq_ignore_case(&mount_root, &storage_root)
        || paths::is_child(&mount_root, &storage_root)
    {
        mount_root
    } else {
        storage_root
    }
}

fn mount_point_metadata_dir(mount_point: &str, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    if paths::eq_ignore_case(mount_point, &storage_root) {
        return paths::data_media_user_root_for_user(user_id);
    }
    paths::storage_to_data_media_for_user(mount_point, user_id)
        .unwrap_or_else(|| mount_point.to_string())
}

pub fn scoped_mount_roots_for_wildcard_rules<'a>(
    uid: i32,
    rules: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut roots = Vec::new();
    for raw in rules {
        let raw = raw.trim_start();
        let raw = raw.strip_prefix('!').unwrap_or(raw).trim_start();
        let mut resolved = paths::resolve_user_path(&paths::normalize(raw), user_id);
        if resolved.is_empty()
            || paths::has_unsafe_segments(&resolved)
            || !paths::contains_wildcards(&resolved)
        {
            continue;
        }
        if !paths::is_absolute(&resolved) {
            resolved = paths::normalize(&paths::join(&storage_root, &resolved));
        }
        if !paths::is_child(&resolved, &storage_root)
            && !paths::eq_ignore_case(&resolved, &storage_root)
        {
            continue;
        }
        let prefix = paths::concrete_prefix_before_wildcard(&resolved);
        let root = scoped_mount_root_for_wildcard_prefix(&prefix, &storage_root);
        roots.push(root);
    }
    compact_scoped_mount_roots(roots, &storage_root)
}

fn scoped_mount_root_for_wildcard_prefix(prefix: &str, storage_root: &str) -> String {
    if prefix.is_empty() || !paths::is_child(prefix, storage_root) {
        return storage_root.to_string();
    }
    if let Some(top_level) = top_level_storage_child(prefix, storage_root) {
        if should_promote_scoped_media_mount_root(&top_level, storage_root) {
            return top_level;
        }
    }
    prefix.to_string()
}

fn should_promote_scoped_media_mount_root(top_level: &str, storage_root: &str) -> bool {
    let Some(relative) = paths::relative_child_path(top_level, storage_root) else {
        return false;
    };
    matches!(
        relative.to_ascii_lowercase().as_str(),
        "dcim" | "pictures" | "movies" | "music"
    )
}

pub fn scoped_mount_roots_for_hybrid_rules(
    uid: i32,
    allowed_real_paths: &[String],
    excluded_real_paths: &[String],
    sandboxed_paths: &[String],
    read_only_paths: &[String],
    path_mappings: &[PathMapping],
    is_mapping_mode_only: bool,
) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut roots = scoped_mount_roots_for_wildcard_rules(
        uid,
        allowed_real_paths
            .iter()
            .chain(excluded_real_paths.iter())
            .chain(sandboxed_paths.iter())
            .chain(read_only_paths.iter())
            .map(String::as_str),
    );

    if is_mapping_mode_only {
        for sandboxed_path in sandboxed_paths {
            let sandboxed_root =
                resolve_concrete_scoped_rule_parent(sandboxed_path, user_id, &storage_root);
            if !sandboxed_root.is_empty() {
                roots.push(sandboxed_root);
            }
        }
    }

    let normalized_read_only_paths = normalize_rule_list(read_only_paths.to_vec(), user_id);
    let (read_only_includes, read_only_excludes) =
        paths::split_exclusion_rules(&normalized_read_only_paths);
    let read_only_excludes =
        paths::overlapping_exclusion_rules(&read_only_includes, &read_only_excludes);
    let scoped_path_mappings = resolve_scoped_path_mappings(path_mappings, user_id, &storage_root);
    for read_only_root in &read_only_includes {
        if paths::contains_wildcards(read_only_root) {
            continue;
        }
        if read_only_excludes.iter().any(|excluded| {
            !paths::contains_wildcards(excluded) && paths::is_child(excluded, read_only_root)
        }) || scoped_path_mappings
            .iter()
            .any(|(request_path, final_path)| {
                (!paths::contains_wildcards(&request_path)
                    && paths::is_child(&request_path, read_only_root))
                    || (!paths::contains_wildcards(&final_path)
                        && paths::is_child(&final_path, read_only_root))
            })
        {
            roots.push(read_only_root.clone());
        }
    }

    compact_scoped_mount_roots(roots, &storage_root)
}

fn resolve_scoped_path_mappings(
    path_mappings: &[PathMapping],
    user_id: i32,
    storage_root: &str,
) -> Vec<(String, String)> {
    let mut resolved = Vec::with_capacity(path_mappings.len());
    for mapping in path_mappings {
        let request_path = resolve_scoped_rule_path(&mapping.request_path, user_id, storage_root);
        let final_path = resolve_scoped_rule_path(&mapping.final_path, user_id, storage_root);
        if request_path.is_empty()
            || final_path.is_empty()
            || paths::eq_ignore_case(&request_path, &final_path)
            || paths::is_android_data_or_obb_path(&final_path)
        {
            continue;
        }
        resolved.push((request_path, final_path));
    }
    resolved
}

fn resolve_concrete_scoped_rule_parent(path: &str, user_id: i32, storage_root: &str) -> String {
    let resolved = resolve_scoped_rule_path(path, user_id, storage_root);
    if resolved.is_empty()
        || paths::contains_wildcards(&resolved)
        || paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }

    let parent = paths::parent(&resolved);
    if paths::eq_ignore_case(&parent, storage_root) || paths::is_child(&parent, storage_root) {
        parent
    } else {
        String::new()
    }
}

fn resolve_scoped_rule_path(path: &str, user_id: i32, storage_root: &str) -> String {
    let mut resolved = paths::resolve_user_path(&paths::normalize(path), user_id);
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return String::new();
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if !paths::is_child(&resolved, storage_root) && !paths::eq_ignore_case(&resolved, storage_root)
    {
        return String::new();
    }
    resolved
}

fn compact_scoped_mount_roots(mut roots: Vec<String>, storage_root: &str) -> Vec<String> {
    paths::sort_dedup_paths_case_insensitive(&mut roots);
    roots.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    let mut effective: Vec<String> = Vec::new();
    for root in roots {
        if effective
            .iter()
            .any(|kept| paths::eq_ignore_case(kept, &root) || paths::is_child(&root, kept))
        {
            continue;
        }
        effective.push(root);
    }

    if effective.len() <= MAX_SCOPED_FUSE_ROOTS {
        return effective;
    }

    let mut top_level: Vec<String> = effective
        .iter()
        .map(|root| {
            top_level_storage_child(root, storage_root).unwrap_or_else(|| storage_root.to_string())
        })
        .collect();
    paths::sort_dedup_paths_case_insensitive(&mut top_level);
    if top_level.len() <= MAX_SCOPED_FUSE_ROOTS {
        return top_level;
    }

    vec![storage_root.to_string()]
}

fn top_level_storage_child(path: &str, storage_root: &str) -> Option<String> {
    if paths::eq_ignore_case(path, storage_root) {
        return None;
    }
    let rel = paths::relative_child_path(path, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    Some(paths::join(storage_root, first))
}

fn send_ready_result(sock: Option<libc::c_int>, result: i32) {
    let Some(sock) = sock else {
        return;
    };
    let _ = unsafe {
        libc::send(
            sock,
            &result as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    unsafe { libc::close(sock) };
}

struct FuseRedirectFs {
    policy: RedirectPolicy,
    state: Mutex<FuseState>,
}

impl FuseRedirectFs {
    fn new(config: FuseRedirectConfig) -> Option<Self> {
        let policy = RedirectPolicy::new(config)?;
        let mut inodes = HashMap::new();
        let mut paths_by_inode = HashMap::new();
        inodes.insert(String::new(), ROOT_INO);
        paths_by_inode.insert(ROOT_INO, String::new());

        Some(Self {
            policy,
            state: Mutex::new(FuseState {
                next_ino: ROOT_INO + 1,
                next_fh: 1,
                inodes,
                paths_by_inode,
                files: HashMap::new(),
                dirs: HashMap::new(),
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
        INodeNo(ino)
    }

    fn path_for_ino(&self, ino: INodeNo) -> Option<String> {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
        if backend.is_shared_public_backend && attr.kind == FileType::Directory {
            attr.uid = self.policy.uid as u32;
            attr.gid = MEDIA_RW_GID;
            attr.perm = SHARED_PUBLIC_DIR_MODE as u16;
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
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            Self::ino_for_path_locked(&mut state, rel)
        };
        match self.visible_attr_for_backend(ino, &backend) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(errno) => reply.error(errno),
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
        if fs::is_directory(&parent) || fs::create_directory(&parent, self.policy.uid) {
            fix_mapped_dir_metadata(&parent, self.policy.uid);
            Ok(())
        } else {
            Err(Errno::EIO)
        }
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
}

impl Filesystem for FuseRedirectFs {
    fn init(&mut self, _req: &Request, config: &mut KernelConfig) -> std::io::Result<()> {
        let _ = config.add_capabilities(InitFlags::FUSE_PASSTHROUGH);
        let _ = config.set_max_stack_depth(2);
        let _ = config.set_max_background(32);
        let _ = config.set_congestion_threshold(24);
        let _ = config.set_max_write(1024 * 1024);
        Ok(())
    }

    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(parent_rel) = self.path_for_ino(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match Self::child_rel(&parent_rel, name) {
            Ok(rel) => self.reply_entry_for_rel(&rel, reply),
            Err(errno) => reply.error(errno),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self
            .backend_for_ino(ino)
            .and_then(|backend| self.visible_attr_for_backend(ino, &backend))
        {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(errno) => reply.error(errno),
        }
    }

    fn readlink(&self, _req: &Request, ino: INodeNo, reply: ReplyData) {
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
        let rel = match self.path_for_ino(ino) {
            Some(rel) => rel,
            None => {
                reply.error(Errno::ENOENT);
                return;
            }
        };
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

        let fh = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            let fh = state.next_handle();
            let entries = build_dir_entries(&mut state, &self.policy, ino, &rel);
            state.dirs.insert(fh, entries);
            fh
        };
        reply.opened(FileHandle(fh), FopenFlags::FOPEN_CACHE_DIR);
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let entries = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            if let Some(entries) = state.dirs.get(&fh.into()) {
                entries.clone()
            } else {
                let Some(rel) = state.paths_by_inode.get(&ino.0).cloned() else {
                    reply.error(Errno::ENOENT);
                    return;
                };
                let Some(backend) = self.policy.backend_for_relative(&rel, OperationKind::Read)
                else {
                    reply.error(Errno::ENOENT);
                    return;
                };
                if !backend.path.is_dir() && !self.policy.is_virtual_dir(&rel) {
                    reply.error(Errno::ENOTDIR);
                    return;
                }
                let entries = build_dir_entries(&mut state, &self.policy, ino, &rel);
                state.dirs.insert(fh.into(), entries.clone());
                entries
            }
        };

        for (index, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry.ino, (index + 1) as u64, entry.kind, entry.name) {
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
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.dirs.remove(&fh.into());
        reply.ok();
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
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
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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

        match reply.open_backing(&file) {
            Ok(backing) => {
                reply.opened_passthrough(FileHandle(fh), FopenFlags::FOPEN_KEEP_CACHE, &backing)
            }
            Err(_) => reply.opened(FileHandle(fh), FopenFlags::empty()),
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
        let file = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
        let mut buf = vec![0u8; (size as usize).min(MAX_READ_SIZE)];
        match file.read_at(&mut buf, offset) {
            Ok(n) => {
                buf.truncate(n);
                reply.data(&buf);
            }
            Err(error) => reply.error(errno_from_io(error)),
        }
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
        let file = {
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.files.remove(&fh.into());
        reply.ok();
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
        let ino = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            Self::ino_for_path_locked(&mut state, &rel)
        };
        let attr = match self.attr_for_backend(ino, &backend) {
            Ok(attr) => attr,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let fh = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            let fh = state.next_handle();
            state.files.insert(
                fh,
                OpenFile {
                    rel,
                    file: file.try_clone().ok().map(Arc::new),
                    is_read_only: false,
                },
            );
            fh
        };
        self.policy.emit_monitor_create(&backend);
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
        self.reply_entry_for_rel(&rel, reply);
    }

    fn unlink(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
        self.remove_child(parent, name, false, reply);
    }

    fn rmdir(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEmpty) {
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
        if !flags.is_empty() {
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
        match std::fs::rename(&old_backend.path, &new_backend.path) {
            Ok(()) => {
                fix_existing_path_metadata(
                    &new_backend.path,
                    self.policy.uid,
                    new_backend.is_shared_public_backend,
                );
                let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
                remap_inode_path(&mut state, &old_rel, &new_rel);
                reply.ok();
            }
            Err(error) => reply.error(errno_from_io(error)),
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
            let uid = uid.unwrap_or(if backend.is_shared_public_backend {
                self.policy.uid as u32
            } else {
                u32::MAX
            });
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
        if let Some(size) = size {
            if let Err(errno) = truncate_path(&backend.path, size) {
                reply.error(errno);
                return;
            }
        }
        if atime.is_some() || mtime.is_some() {
            if let Err(errno) = utimens_path(&backend.path, atime, mtime) {
                reply.error(errno);
                return;
            }
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
}

impl FuseRedirectFs {
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
                let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
                remove_inode_path(&mut state, &rel);
                reply.ok();
            }
            Err(error) => reply.error(errno_from_io(error)),
        }
    }
}

#[derive(Clone)]
struct RedirectPolicy {
    package_name: String,
    uid: i32,
    user_id: i32,
    storage_root: String,
    mount_rel: String,
    real_root: PathBuf,
    redirect_root: PathBuf,
    rule_prefixes: Vec<RulePrefix>,
    allowed_real_paths: Vec<String>,
    excluded_real_paths: Vec<String>,
    sandboxed_paths: Vec<String>,
    read_only_paths: Vec<String>,
    read_only_excluded_paths: Vec<String>,
    path_mappings: Vec<PathMapping>,
    is_mapping_mode_only: bool,
    is_file_monitor_enabled: bool,
}

impl RedirectPolicy {
    fn new(config: FuseRedirectConfig) -> Option<Self> {
        let user_id = config.user_id();
        let storage_root = paths::storage_user_root_for_user(user_id);
        let mount_root = fuse_mount_point(&config, user_id);
        let mount_rel = paths::relative_child_path(&mount_root, &storage_root)
            .unwrap_or("")
            .trim_matches('/')
            .to_string();
        let real_root = real_backend_root_for_config(&config, user_id);
        let redirect_storage = paths::resolve_user_path(
            &paths::resolve_placeholders(
                &paths::normalize(&config.redirect_target),
                &config.app_data_dir,
                &config.redirect_target,
            ),
            user_id,
        );
        let redirect_root_string =
            paths::storage_to_data_media_for_user(&redirect_storage, user_id).unwrap_or_default();
        if redirect_root_string.is_empty() {
            log::error!("fuse redirect target invalid: {}", config.redirect_target);
            return None;
        }
        if !fs::create_directory(&redirect_root_string, config.uid)
            && !fs::is_directory(&redirect_root_string)
        {
            log::error!(
                "fuse redirect target mkdir failed: {}",
                redirect_root_string
            );
            return None;
        }
        fix_mapped_dir_metadata(&redirect_root_string, config.uid);

        let mut path_mappings = resolve_path_mappings(
            &config.path_mappings,
            user_id,
            &storage_root,
            &config.app_data_dir,
            &config.redirect_target,
        );
        sort_path_mappings_shortest_request_first(&mut path_mappings);

        let rule_prefixes = build_rule_prefixes(
            &config.allowed_real_paths,
            &config.excluded_real_paths,
            &config.sandboxed_paths,
            &config.read_only_paths,
            &path_mappings,
            user_id,
            &storage_root,
            &mount_root,
        );

        let normalized_read_only_paths = normalize_rule_list(config.read_only_paths, user_id);
        let (read_only_paths, read_only_excluded_paths) =
            paths::split_exclusion_rules(&normalized_read_only_paths);
        let read_only_excluded_paths =
            paths::overlapping_exclusion_rules(&read_only_paths, &read_only_excluded_paths);

        Some(Self {
            package_name: config.package_name,
            uid: config.uid,
            user_id,
            storage_root,
            mount_rel,
            real_root,
            redirect_root: PathBuf::from(redirect_root_string),
            rule_prefixes,
            allowed_real_paths: normalize_rule_list(config.allowed_real_paths, user_id),
            excluded_real_paths: normalize_rule_list(config.excluded_real_paths, user_id),
            sandboxed_paths: normalize_rule_list(config.sandboxed_paths, user_id),
            read_only_paths,
            read_only_excluded_paths,
            path_mappings,
            is_mapping_mode_only: config.is_mapping_mode_only,
            is_file_monitor_enabled: config.is_file_monitor_enabled,
        })
    }

    fn backend_for_relative(&self, rel: &str, operation: OperationKind) -> Option<BackendPath> {
        let rel = sanitize_relative(rel)?;
        let storage_path = self.storage_path_for_rel(&rel);
        let resolved_storage_path = paths::normalize(&storage_path);
        let decision = self.backend_decision(&resolved_storage_path, operation);
        let backend = match decision.kind {
            BackendKind::Real => {
                if let Some(mapped_target) = self.resolve_mapping(&resolved_storage_path) {
                    self.backend_path_for_storage(&mapped_target, BackendKind::Real)?
                } else {
                    self.real_backend_for_rel(&rel)
                }
            }
            BackendKind::Redirect => self.redirect_backend_for_rel(&rel),
        };

        let is_shared_public_backend = self.is_shared_public_backend_path(&backend);
        Some(BackendPath {
            rel,
            path: backend,
            is_read_only: decision.is_read_only,
            is_shared_public_backend,
        })
    }

    fn resolve_mapping(&self, storage_path: &str) -> Option<String> {
        for mapping in self.path_mappings.iter().rev() {
            if paths::matches(&mapping.request_path, storage_path, true) {
                let Some(suffix) = paths::child_suffix(storage_path, &mapping.request_path) else {
                    return Some(mapping.final_path.clone());
                };
                return Some(format!(
                    "{}{}",
                    mapping.final_path.trim_end_matches('/'),
                    suffix
                ));
            }
        }
        None
    }

    fn is_read_only(&self, storage_path: &str) -> bool {
        if let Some(mapped_target) = self.resolve_mapping(storage_path) {
            if self.matches_any(&self.excluded_real_paths, &mapped_target) {
                return false;
            }
            if self.matches_any(&self.read_only_excluded_paths, &mapped_target) {
                return false;
            }
            return self.matches_any(&self.read_only_paths, &mapped_target);
        }
        if self.matches_any(&self.excluded_real_paths, storage_path) {
            return false;
        }
        if self.matches_any(&self.read_only_excluded_paths, storage_path) {
            return false;
        }
        if self.matches_any(&self.read_only_paths, storage_path) {
            return true;
        }
        false
    }

    fn backend_decision(&self, storage_path: &str, operation: OperationKind) -> BackendDecision {
        let is_read_only = self.is_read_only(storage_path);
        let kind = if self.resolve_mapping(storage_path).is_some() {
            BackendKind::Real
        } else if self.is_mapping_mode_only {
            if self.matches_any(&self.sandboxed_paths, storage_path) {
                BackendKind::Redirect
            } else {
                BackendKind::Real
            }
        } else if self.matches_any(&self.excluded_real_paths, storage_path) {
            BackendKind::Redirect
        } else if self.matches_any(&self.read_only_excluded_paths, storage_path) {
            BackendKind::Real
        } else if self.matches_any(&self.allowed_real_paths, storage_path)
            || matches!(operation, OperationKind::Read)
                && (is_read_only || self.has_real_child_rule(storage_path))
        {
            BackendKind::Real
        } else {
            BackendKind::Redirect
        };
        BackendDecision { kind, is_read_only }
    }

    fn matches_any(&self, rules: &[String], storage_path: &str) -> bool {
        let pending_display_path = media_store_pending_display_path(storage_path);
        rules.iter().any(|rule| {
            paths::matches(rule, storage_path, true)
                || pending_display_path
                    .as_deref()
                    .is_some_and(|display_path| paths::matches(rule, display_path, true))
        })
    }

    fn has_real_child_rule(&self, storage_path: &str) -> bool {
        self.allowed_real_paths
            .iter()
            .chain(self.read_only_paths.iter())
            .chain(
                self.path_mappings
                    .iter()
                    .map(|mapping| &mapping.request_path),
            )
            .any(|rule| rule_may_match_path_or_descendant(rule, storage_path))
    }

    fn is_virtual_dir(&self, rel: &str) -> bool {
        let storage_path = self.storage_path_for_rel(rel);
        self.rule_prefixes.iter().any(|prefix| {
            let prefix_storage_path = self.storage_path_for_rel(&prefix.rel);
            paths::eq_ignore_case(&prefix.rel, rel)
                || paths::is_child(&prefix_storage_path, &storage_path)
                || (!prefix.full_prefix
                    && rule_has_path_prefix(&storage_path, &prefix_storage_path))
        })
    }

    fn real_backend_for_rel(&self, rel: &str) -> PathBuf {
        let rel = self.full_storage_rel(rel);
        self.real_backend_for_storage_rel(&rel)
    }

    fn redirect_backend_for_rel(&self, rel: &str) -> PathBuf {
        let rel = self.full_storage_rel(rel);
        self.redirect_backend_for_storage_rel(&rel)
    }

    fn real_backend_for_storage_rel(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.real_root.clone()
        } else {
            self.real_root.join(rel)
        }
    }

    fn redirect_backend_for_storage_rel(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.redirect_root.clone()
        } else {
            self.redirect_root.join(rel)
        }
    }

    fn backend_path_for_storage(&self, storage_path: &str, kind: BackendKind) -> Option<PathBuf> {
        let rel = paths::relative_child_path(storage_path, &self.storage_root)?;
        Some(match kind {
            BackendKind::Real => self.real_backend_for_storage_rel(rel),
            BackendKind::Redirect => self.redirect_backend_for_storage_rel(rel),
        })
    }

    fn storage_path_for_rel(&self, rel: &str) -> String {
        let rel = self.full_storage_rel(rel);
        if rel.is_empty() {
            self.storage_root.clone()
        } else {
            paths::join(&self.storage_root, &rel)
        }
    }

    fn emit_monitor_create(&self, backend: &BackendPath) {
        if !self.is_file_monitor_enabled {
            return;
        }
        let display_path = self.storage_path_for_rel(&backend.rel);
        if display_path.is_empty()
            || crate::config::SettingsHub::instance()
                .should_filter_monitor_record(&display_path, "fuse_create")
        {
            return;
        }
        let backend_path = backend.path.to_string_lossy();
        if should_skip_duplicate_monitor_create(
            &self.package_name,
            &display_path,
            backend_path.as_ref(),
        ) {
            return;
        }
        log::info!(
            target: FILE_MONITOR_LOG_TAG,
            "{}|{}|{}|CREATE|{}|ret=0|errno=0|identify_method=fuse_redirect|identify_reliability=high|op=fuse_create|source=fuse_redirect|backend={}",
            build_monitor_timestamp(),
            self.package_name,
            self.package_name,
            display_path,
            backend_path
        );
    }

    fn emit_monitor_read_only_deny(&self, operation_name: &str, backend: &BackendPath) {
        self.emit_monitor_read_only_deny_with_from(operation_name, backend, None, libc::EROFS);
    }

    fn emit_monitor_read_only_deny_with_errno(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        error_no: i32,
    ) {
        self.emit_monitor_read_only_deny_with_from(operation_name, backend, None, error_no);
    }

    fn emit_monitor_read_only_deny_with_from(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        from_backend: Option<&BackendPath>,
        error_no: i32,
    ) {
        let Some(line) =
            self.monitor_read_only_deny_line(operation_name, backend, from_backend, error_no)
        else {
            return;
        };
        log::info!(target: FILE_MONITOR_LOG_TAG, "{}", line);
    }

    fn monitor_read_only_deny_line(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        from_backend: Option<&BackendPath>,
        error_no: i32,
    ) -> Option<String> {
        if !self.is_file_monitor_enabled {
            return None;
        }
        let display_path = self.storage_path_for_rel(&backend.rel);
        if display_path.is_empty()
            || crate::config::SettingsHub::instance()
                .should_filter_monitor_record(&display_path, "")
        {
            return None;
        }
        let backend_path = backend.path.to_string_lossy();
        let event_kind = monitor_event_kind_for_operation(operation_name);
        let mut line = format!(
            "{}|{}|{}|{}|{}|ret=-1|errno={}|identify_method=fuse_redirect|identify_reliability=high|op={}|source=fuse_redirect|backend={}|{}",
            build_monitor_timestamp(),
            self.package_name,
            self.package_name,
            event_kind,
            display_path,
            error_no,
            operation_name,
            backend_path,
            READ_ONLY_DENY_EXTRA
        );
        if let Some(from_backend) = from_backend {
            let from_path = self.storage_path_for_rel(&from_backend.rel);
            if !from_path.is_empty() && from_path != display_path {
                line.push_str("|from=");
                line.push_str(&from_path);
            }
        }
        Some(line)
    }

    fn full_storage_rel(&self, rel: &str) -> String {
        let rel = rel.trim_matches('/');
        if self.mount_rel.is_empty() {
            rel.to_string()
        } else if rel.is_empty() {
            self.mount_rel.clone()
        } else {
            paths::join(&self.mount_rel, rel)
        }
    }

    fn is_shared_public_backend_path(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        let relative =
            paths::relative_child_path(&path, &paths::data_media_user_root_for_user(self.user_id))
                .or_else(|| {
                    paths::relative_child_path(&path, &real_storage_anchor_for_user(self.user_id))
                });
        let Some(relative) = relative else {
            return false;
        };
        !is_android_app_private_relative_path(relative)
    }
}

fn real_backend_root_for_config(config: &FuseRedirectConfig, user_id: i32) -> PathBuf {
    if let Some(root) = config.real_root_override.as_deref() {
        let normalized = paths::normalize(root);
        if paths::eq_ignore_case(&normalized, &real_storage_anchor_for_user(user_id)) {
            return PathBuf::from(normalized);
        }
        log::warn!("fuse real root override ignored: {}", root);
    }
    PathBuf::from(paths::data_media_user_root_for_user(user_id))
}

fn real_storage_anchor_for_user(user_id: i32) -> String {
    paths::join(module_paths::REAL_STORAGE_TMP_DIR, &user_id.to_string())
}

fn media_store_pending_display_path(path: &str) -> Option<String> {
    let slash = path.rfind('/')?;
    let file_name = &path[slash + 1..];
    let pending_tail = file_name.strip_prefix(".pending-")?;
    let display_name_start = pending_tail.find('-')? + 1;
    if display_name_start >= pending_tail.len() {
        return None;
    }

    Some(format!(
        "{}/{}",
        path[..slash].trim_end_matches('/'),
        &pending_tail[display_name_start..]
    ))
}

fn build_monitor_timestamp() -> String {
    let mut now: libc::time_t = 0;
    unsafe { libc::time(&mut now as *mut _) };

    let mut tm_value: libc::tm = unsafe { std::mem::zeroed() };
    let tm_ptr = unsafe { libc::localtime_r(&now as *const _, &mut tm_value as *mut _) };
    if tm_ptr.is_null() {
        return String::new();
    }

    let mut buffer = [0u8; 32];
    let format = b"%Y-%m-%d %H:%M:%S\0";
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            format.as_ptr() as *const _,
            &tm_value as *const _,
        )
    };
    if written == 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buffer[..written]).to_string()
}

fn should_skip_duplicate_monitor_create(
    package_name: &str,
    display_path: &str,
    backend_path: &str,
) -> bool {
    if package_name.is_empty() || display_path.is_empty() || backend_path.is_empty() {
        return false;
    }
    let key = format!("{}|{}|{}", package_name, display_path, backend_path);
    let now_ms = paths::monotonic_ms();
    let Ok(mut recent) = RECENT_MONITOR_CREATES.lock() else {
        return false;
    };
    if let Some(last_ms) = recent.get_mut(&key) {
        if now_ms.saturating_sub(*last_ms) < DUPLICATE_MONITOR_CREATE_WINDOW_MS {
            *last_ms = now_ms;
            return true;
        }
        *last_ms = now_ms;
        return false;
    }
    if recent.len() >= MAX_RECENT_MONITOR_CREATES {
        recent.retain(|_, last_ms| {
            now_ms.saturating_sub(*last_ms) < DUPLICATE_MONITOR_CREATE_WINDOW_MS
        });
    }
    recent.insert(key, now_ms);
    false
}

struct BackendPath {
    rel: String,
    path: PathBuf,
    is_read_only: bool,
    is_shared_public_backend: bool,
}

struct FuseState {
    next_ino: u64,
    next_fh: u64,
    inodes: HashMap<String, u64>,
    paths_by_inode: HashMap<u64, String>,
    files: HashMap<u64, OpenFile>,
    dirs: HashMap<u64, Vec<DirEntry>>,
}

impl FuseState {
    fn next_handle(&mut self) -> u64 {
        let fh = self.next_fh;
        self.next_fh = self.next_fh.saturating_add(1).max(1);
        fh
    }
}

struct OpenFile {
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackendKind {
    Real,
    Redirect,
}

#[derive(Clone, Copy)]
enum OperationKind {
    Read,
    Write,
}

struct BackendDecision {
    kind: BackendKind,
    is_read_only: bool,
}

#[derive(Clone)]
struct RulePrefix {
    rel: String,
    full_prefix: bool,
}

fn build_dir_entries(
    state: &mut FuseState,
    policy: &RedirectPolicy,
    ino: INodeNo,
    rel: &str,
) -> Vec<DirEntry> {
    let parent_rel = paths::parent(rel);
    let parent_ino = if rel.is_empty() {
        ROOT_INO
    } else {
        *state.inodes.get(&parent_rel).unwrap_or(&ROOT_INO)
    };
    let mut entries = vec![
        DirEntry {
            ino,
            kind: FileType::Directory,
            name: ".".to_string(),
        },
        DirEntry {
            ino: INodeNo(parent_ino),
            kind: FileType::Directory,
            name: "..".to_string(),
        },
    ];

    let mut seen = HashMap::<String, usize>::new();
    append_backend_dir_entries(
        state,
        policy,
        rel,
        &policy.redirect_backend_for_rel(rel),
        &mut entries,
        &mut seen,
    );
    append_backend_dir_entries(
        state,
        policy,
        rel,
        &policy.real_backend_for_rel(rel),
        &mut entries,
        &mut seen,
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
                state,
                policy,
                rel,
                &policy.real_backend_for_storage_rel(target_rel),
                &mut entries,
                &mut seen,
            );
        }
    }
    append_rule_prefix_entries(state, policy, rel, &mut entries, &mut seen);

    entries
}

fn append_backend_dir_entries(
    state: &mut FuseState,
    policy: &RedirectPolicy,
    parent_rel: &str,
    backend: &Path,
    entries: &mut Vec<DirEntry>,
    seen: &mut HashMap<String, usize>,
) {
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
        if !paths_eq(&item.path(), &child_backend.path) && !policy.is_virtual_dir(&child_rel) {
            continue;
        }
        let kind = item
            .file_type()
            .map(file_type_from_std)
            .unwrap_or(FileType::RegularFile);
        insert_dir_entry(state, entries, seen, child_rel, name, kind);
    }
}

fn append_rule_prefix_entries(
    state: &mut FuseState,
    policy: &RedirectPolicy,
    parent_rel: &str,
    entries: &mut Vec<DirEntry>,
    seen: &mut HashMap<String, usize>,
) {
    for prefix in policy.rule_prefixes.iter() {
        let Some(child_name) = visible_prefix_child(parent_rel, prefix) else {
            continue;
        };
        let child_rel = if parent_rel.is_empty() {
            child_name.clone()
        } else {
            paths::join(parent_rel, &child_name)
        };
        insert_dir_entry(
            state,
            entries,
            seen,
            child_rel,
            child_name,
            FileType::Directory,
        );
    }
}

fn insert_dir_entry(
    state: &mut FuseState,
    entries: &mut Vec<DirEntry>,
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
    let child_ino = FuseRedirectFs::ino_for_path_locked(state, &child_rel);
    let index = entries.len();
    entries.push(DirEntry {
        ino: child_ino,
        kind,
        name,
    });
    seen.insert(key, index);
}

fn build_rule_prefixes(
    allowed_real_paths: &[String],
    excluded_real_paths: &[String],
    sandboxed_paths: &[String],
    read_only_paths: &[String],
    path_mappings: &[PathMapping],
    user_id: i32,
    storage_root: &str,
    mount_root: &str,
) -> Vec<RulePrefix> {
    let mut prefixes = Vec::new();
    for rule in allowed_real_paths
        .iter()
        .chain(excluded_real_paths.iter())
        .chain(sandboxed_paths.iter())
        .chain(read_only_paths.iter())
    {
        if let Some(prefix) = visible_rule_prefix(rule, user_id, storage_root, mount_root) {
            prefixes.push(prefix);
        }
    }
    for mapping in path_mappings {
        if let Some(prefix) =
            visible_rule_prefix(&mapping.request_path, user_id, storage_root, mount_root)
        {
            prefixes.push(prefix);
        }
    }
    prefixes.sort_by(|left, right| left.rel.cmp(&right.rel));
    prefixes.dedup_by(|left, right| paths::eq_ignore_case(&left.rel, &right.rel));
    prefixes
}

fn visible_rule_prefix(
    rule: &str,
    user_id: i32,
    storage_root: &str,
    mount_root: &str,
) -> Option<RulePrefix> {
    let rule = rule.trim_start();
    let rule = rule.strip_prefix('!').unwrap_or(rule).trim_start();
    let full_prefix = !paths::contains_wildcards(rule);
    let rule_prefix = concrete_rule_prefix(rule)?;
    let mut resolved = paths::resolve_user_path(&paths::normalize(&rule_prefix), user_id);
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return None;
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if !paths::is_child(&resolved, storage_root) && !paths::eq_ignore_case(&resolved, storage_root)
    {
        return None;
    }
    if !paths::eq_ignore_case(&resolved, mount_root) && !paths::is_child(&resolved, mount_root) {
        return None;
    }
    let rel = paths::relative_child_path(&resolved, mount_root)
        .unwrap_or("")
        .trim_matches('/')
        .to_string();
    if rel.is_empty() {
        return None;
    }
    Some(RulePrefix { rel, full_prefix })
}

fn concrete_rule_prefix(rule: &str) -> Option<String> {
    let normalized = paths::normalize(rule);
    if !paths::contains_wildcards(&normalized) {
        return Some(normalized);
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    let mut kept = Vec::new();
    for part in parts {
        if part.contains('*') || part.contains('?') {
            break;
        }
        kept.push(part);
    }
    let prefix = kept.join("/");
    if prefix.is_empty() || prefix == "/" {
        None
    } else {
        Some(prefix)
    }
}

fn visible_prefix_child(parent_rel: &str, prefix: &RulePrefix) -> Option<String> {
    let prefix_rel = prefix.rel.trim_matches('/');
    if prefix_rel.is_empty() {
        return None;
    }
    if parent_rel.is_empty() {
        return prefix_rel.split('/').next().map(ToString::to_string);
    }
    let parent = parent_rel.trim_matches('/');
    if prefix_rel.eq_ignore_ascii_case(parent) {
        return None;
    }
    let parent_prefix = format!("{}/", parent);
    if !prefix_rel
        .get(..parent_prefix.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(&parent_prefix))
    {
        return None;
    }
    let rest = &prefix_rel[parent_prefix.len()..];
    if rest.is_empty() {
        return None;
    }
    Some(rest.split('/').next().unwrap_or(rest).to_string())
}

fn rule_has_path_prefix(rule: &str, path: &str) -> bool {
    if rule.is_empty() || path.is_empty() {
        return false;
    }
    if paths::matches(rule, path, true) {
        return true;
    }
    let rule_norm = paths::normalize(rule);
    let path_norm = paths::normalize(path);
    if paths::contains_wildcards(&rule_norm) {
        return false;
    }
    paths::is_child(&path_norm, &rule_norm) || paths::eq_ignore_case(&path_norm, &rule_norm)
}

fn rule_may_match_path_or_descendant(rule: &str, path: &str) -> bool {
    if rule.is_empty() || path.is_empty() {
        return false;
    }
    if paths::matches(rule, path, true) {
        return true;
    }

    let rule_segments: Vec<&str> = rule.split('/').filter(|part| !part.is_empty()).collect();
    let path_segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if path_segments.is_empty() || path_segments.len() > rule_segments.len() {
        return false;
    }

    let rule_prefix = format!("/{}", rule_segments[..path_segments.len()].join("/"));
    let path_prefix = format!("/{}", path_segments.join("/"));
    paths::matches(&rule_prefix, &path_prefix, false)
}

fn paths_eq(left: &Path, right: &Path) -> bool {
    left == right
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

fn normalize_rule_list(paths_in: Vec<String>, user_id: i32) -> Vec<String> {
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

fn resolve_path_mappings(
    mappings: &[PathMapping],
    user_id: i32,
    storage_root: &str,
    app_data_dir: &str,
    redirect_target: &str,
) -> Vec<PathMapping> {
    let mut resolved = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let Some(current) = resolve_storage_rule(
            &mapping.request_path,
            user_id,
            storage_root,
            app_data_dir,
            redirect_target,
        ) else {
            continue;
        };
        let Some(target) = resolve_storage_rule(
            &mapping.final_path,
            user_id,
            storage_root,
            app_data_dir,
            redirect_target,
        ) else {
            continue;
        };
        if paths::eq_ignore_case(&current, &target) {
            continue;
        }
        if paths::is_android_data_or_obb_path(&target) {
            continue;
        }
        resolved.push(PathMapping::new(current, target));
    }
    resolved
}

fn resolve_storage_rule(
    path: &str,
    user_id: i32,
    storage_root: &str,
    app_data_dir: &str,
    redirect_target: &str,
) -> Option<String> {
    let mut resolved = paths::resolve_user_path(
        &paths::resolve_placeholders(&paths::normalize(path), app_data_dir, redirect_target),
        user_id,
    );
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return None;
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if paths::is_child(&resolved, storage_root) {
        Some(resolved)
    } else {
        None
    }
}

fn sanitize_relative(rel: &str) -> Option<String> {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return Some(String::new());
    }
    if trimmed.contains('\0') || paths::has_unsafe_segments(trimmed) || paths::is_absolute(trimmed)
    {
        return None;
    }
    Some(paths::normalize(trimmed).trim_matches('/').to_string())
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

fn monitor_event_kind_for_operation(operation_name: &str) -> &'static str {
    match operation_name.split(':').next().unwrap_or(operation_name) {
        "open" => "OPEN",
        "write" => "WRITE",
        "create" => "CREATE",
        "mkdir" => "MKDIR",
        "rename" => "RENAME",
        "unlink" => "UNLINK",
        "rmdir" => "RMDIR",
        "truncate" => "TRUNCATE",
        "chmod" | "chown" | "utimens" | "setattr" => "ATTRIB",
        "access" => "ACCESS",
        _ => "WRITE",
    }
}

fn cstring_path(path: &Path) -> Result<CString, Errno> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)
}

fn fix_mapped_dir_metadata(path: &str, owner_uid: i32) {
    if let Ok(c_path) = CString::new(path) {
        let _ = unsafe { libc::chown(c_path.as_ptr(), owner_uid as u32, MEDIA_RW_GID) };
        let _ = unsafe { libc::chmod(c_path.as_ptr(), MAPPED_DIR_MODE) };
    }
}

fn fix_path_metadata(
    path: &Path,
    owner_uid: i32,
    mode: u32,
    is_shared_public_backend: bool,
    is_dir: bool,
) {
    let mode = adjust_metadata_mode(mode, is_shared_public_backend, is_dir);
    if let Ok(c_path) = cstring_path(path) {
        let _ = unsafe { libc::chown(c_path.as_ptr(), owner_uid as u32, MEDIA_RW_GID) };
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

fn fix_existing_path_metadata(path: &Path, owner_uid: i32, is_shared_public_backend: bool) {
    if !is_shared_public_backend {
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    let mode = metadata.permissions().mode() & 0o7777;
    fix_path_metadata(
        path,
        owner_uid,
        mode,
        is_shared_public_backend,
        metadata.is_dir(),
    );
}

fn adjust_metadata_mode(mode: u32, is_shared_public_backend: bool, is_dir: bool) -> u32 {
    let mode = mode & 0o7777;
    if !is_shared_public_backend {
        return mode;
    }
    if is_dir {
        return SHARED_PUBLIC_DIR_MODE;
    }
    let owner_bits_for_group = (mode & 0o700) >> 3;
    (mode | owner_bits_for_group) & !0o007
}

fn chmod_path(path: &Path, mode: u32) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    if unsafe { libc::chmod(c_path.as_ptr(), mode as libc::mode_t) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

fn chown_path(path: &Path, uid: u32, gid: u32) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    let uid = if uid == u32::MAX { !0 } else { uid };
    let gid = if gid == u32::MAX { !0 } else { gid };
    if unsafe { libc::chown(c_path.as_ptr(), uid, gid) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

fn truncate_path(path: &Path, size: u64) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    if unsafe { libc::truncate(c_path.as_ptr(), size as libc::off_t) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

fn utimens_path(
    path: &Path,
    atime: Option<TimeOrNow>,
    mtime: Option<TimeOrNow>,
) -> Result<(), Errno> {
    let c_path = cstring_path(path)?;
    let times = [
        time_or_now_to_timespec(atime),
        time_or_now_to_timespec(mtime),
    ];
    if unsafe { libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0) } == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
    }
}

fn time_or_now_to_timespec(value: Option<TimeOrNow>) -> libc::timespec {
    match value {
        Some(TimeOrNow::SpecificTime(time)) => match time.duration_since(UNIX_EPOCH) {
            Ok(duration) => libc::timespec {
                tv_sec: duration.as_secs() as libc::time_t,
                tv_nsec: duration.subsec_nanos() as libc::c_long,
            },
            Err(_) => libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
        },
        Some(TimeOrNow::Now) => libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_NOW as libc::c_long,
        },
        None => libc::timespec {
            tv_sec: 0,
            tv_nsec: libc::UTIME_OMIT as libc::c_long,
        },
    }
}

fn remove_inode_path(state: &mut FuseState, rel: &str) {
    if let Some(ino) = state.inodes.remove(rel) {
        state.paths_by_inode.remove(&ino);
    }
}

fn remap_inode_path(state: &mut FuseState, old_rel: &str, new_rel: &str) {
    if let Some(ino) = state.inodes.remove(old_rel) {
        state.inodes.insert(new_rel.to_string(), ino);
        state.paths_by_inode.insert(ino, new_rel.to_string());
    }
}

fn errno_from_io(error: std::io::Error) -> Errno {
    errno_from_code(error.raw_os_error().unwrap_or(libc::EIO))
}

fn errno_from_code(code: i32) -> Errno {
    Errno::from_i32(code)
}

fn last_errno() -> i32 {
    unsafe { *libc::__errno() }
}

fn is_android_app_private_relative_path(relative: &str) -> bool {
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    match parts.next() {
        Some("data" | "media" | "obb") => parts.next().is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackendKind, FuseRedirectConfig, OperationKind, RedirectPolicy, adjust_metadata_mode,
        concrete_rule_prefix, is_android_app_private_relative_path,
        rule_may_match_path_or_descendant, scoped_mount_roots_for_hybrid_rules,
        scoped_mount_roots_for_wildcard_rules, visible_rule_prefix,
    };
    use crate::domain::PathMapping;

    #[test]
    fn wildcard_visible_prefix_stops_before_first_wildcard_segment() {
        let prefix = visible_rule_prefix(
            "/storage/emulated/0/Download/*/foo",
            0,
            "/storage/emulated/0",
            "/storage/emulated/0",
        )
        .expect("prefix");
        assert_eq!(prefix.rel, "Download");
        assert!(!prefix.full_prefix);
    }

    #[test]
    fn wildcard_visible_prefix_is_relative_to_scoped_mount_root() {
        let prefix = visible_rule_prefix(
            "/storage/emulated/0/Download/Sub/*/foo",
            0,
            "/storage/emulated/0",
            "/storage/emulated/0/Download",
        )
        .expect("prefix");
        assert_eq!(prefix.rel, "Sub");
        assert!(!prefix.full_prefix);
    }

    #[test]
    fn wildcard_visible_prefix_skips_rules_outside_scoped_mount_root() {
        assert!(
            visible_rule_prefix(
                "/storage/emulated/0/Pictures/*",
                0,
                "/storage/emulated/0",
                "/storage/emulated/0/Download",
            )
            .is_none()
        );
    }

    #[test]
    fn concrete_rule_prefix_never_exposes_wildcard_segment() {
        assert_eq!(
            concrete_rule_prefix("/storage/emulated/0/Download/*/foo").as_deref(),
            Some("/storage/emulated/0/Download")
        );
        assert_eq!(
            concrete_rule_prefix("/storage/emulated/0/*/foo").as_deref(),
            Some("/storage/emulated/0")
        );
    }

    #[test]
    fn scoped_mount_roots_pick_minimal_concrete_prefixes() {
        let roots = scoped_mount_roots_for_wildcard_rules(
            10000,
            [
                "Download/A*",
                "!Download/private/*",
                "Pictures/Camera/IMG_????.jpg",
            ],
        );
        assert_eq!(
            roots,
            vec![
                "/storage/emulated/0/Download".to_string(),
                "/storage/emulated/0/Pictures".to_string(),
            ]
        );
    }

    #[test]
    fn scoped_mount_roots_promote_media_top_level_for_nested_wildcards() {
        let roots = scoped_mount_roots_for_wildcard_rules(
            10000,
            ["DCIM/SrtFuseQQ/SrtAllowed*", "Download/SrtFuseQ?/Media"],
        );

        assert_eq!(
            roots,
            vec![
                "/storage/emulated/0/DCIM".to_string(),
                "/storage/emulated/0/Download".to_string(),
            ]
        );
    }

    #[test]
    fn scoped_mount_roots_fall_back_to_storage_root_for_first_segment_wildcard() {
        let roots = scoped_mount_roots_for_wildcard_rules(10000, ["*/secret"]);
        assert_eq!(roots, vec!["/storage/emulated/0".to_string()]);
    }

    #[test]
    fn scoped_mount_roots_compact_excess_roots_to_top_level() {
        let roots = scoped_mount_roots_for_wildcard_rules(
            10000,
            [
                "Download/A*",
                "Pictures/A*",
                "Movies/A*",
                "Music/A*",
                "Documents/A*",
            ],
        );
        assert_eq!(roots, vec!["/storage/emulated/0".to_string()]);
    }

    #[test]
    fn scoped_mount_roots_include_read_only_parent_with_writable_mapping_child() {
        let roots = scoped_mount_roots_for_hybrid_rules(
            10288,
            &[],
            &[],
            &[],
            &[
                "Download".to_string(),
                "!Download/ThirdParty/QQ".to_string(),
            ],
            &[PathMapping::new(
                "Download/QQ".to_string(),
                "Download/ThirdParty/QQ".to_string(),
            )],
            false,
        );

        assert_eq!(roots, vec!["/storage/emulated/0/Download".to_string()]);
    }

    #[test]
    fn scoped_mount_roots_ignore_read_only_parent_for_private_mapping_target() {
        let roots = scoped_mount_roots_for_hybrid_rules(
            10288,
            &[],
            &[],
            &[],
            &["Download".to_string()],
            &[PathMapping::new(
                "Download/QQ".to_string(),
                "Android/data/com.tencent.mobileqq/files".to_string(),
            )],
            false,
        );

        assert!(roots.is_empty());
    }

    #[test]
    fn scoped_mount_roots_include_top_level_sandbox_parent() {
        let roots = scoped_mount_roots_for_hybrid_rules(
            10288,
            &[],
            &[],
            &[".CMRcs".to_string()],
            &[],
            &[],
            true,
        );

        assert_eq!(roots, vec!["/storage/emulated/0".to_string()]);
    }

    #[test]
    fn scoped_mount_roots_include_nested_sandbox_parent() {
        let roots = scoped_mount_roots_for_hybrid_rules(
            10288,
            &[],
            &[],
            &["Download/AppCache".to_string()],
            &[],
            &[],
            true,
        );

        assert_eq!(roots, vec!["/storage/emulated/0/Download".to_string()]);
    }

    #[test]
    fn scoped_mount_roots_ignore_concrete_sandbox_parent_outside_map_only() {
        let roots = scoped_mount_roots_for_hybrid_rules(
            10288,
            &[],
            &[],
            &[".CMRcs".to_string()],
            &[],
            &[],
            false,
        );

        assert!(roots.is_empty());
    }

    #[test]
    fn scoped_fuse_mapping_target_inherits_read_only_parent_without_exclusion() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "com.tencent.mobileqq".to_string(),
            uid: 10288,
            app_data_dir: "/data/user/0/com.tencent.mobileqq".to_string(),
            redirect_target: "/storage/emulated/0/Android/data/com.tencent.mobileqq/sdcard"
                .to_string(),
            mount_root: Some("/storage/emulated/0/Download".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: false,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: vec!["Download".to_string()],
            path_mappings: vec![PathMapping::new(
                "Download/QQ".to_string(),
                "Download/第三方下载/QQ".to_string(),
            )],
            is_mapping_mode_only: false,
        })
        .expect("policy");

        assert!(policy.is_read_only("/storage/emulated/0/Download/QQ/flash_file_test_tmp.txt"));
    }

    #[test]
    fn scoped_fuse_mapping_target_read_only_respects_excluded_real_path() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "com.tencent.mobileqq".to_string(),
            uid: 10288,
            app_data_dir: "/data/user/0/com.tencent.mobileqq".to_string(),
            redirect_target: "/storage/emulated/0/Android/data/com.tencent.mobileqq/sdcard"
                .to_string(),
            mount_root: Some("/storage/emulated/0/Download".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: false,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: vec!["Download/第三方下载/QQ".to_string()],
            sandboxed_paths: Vec::new(),
            read_only_paths: vec!["Download".to_string()],
            path_mappings: vec![PathMapping::new(
                "Download/QQ".to_string(),
                "Download/第三方下载/QQ".to_string(),
            )],
            is_mapping_mode_only: false,
        })
        .expect("policy");

        let decision = policy.backend_decision(
            "/storage/emulated/0/Download/QQ/flash_file_test_tmp.txt",
            OperationKind::Write,
        );

        assert!(matches!(decision.kind, BackendKind::Real));
        assert!(!decision.is_read_only);
    }

    #[test]
    fn scoped_fuse_read_only_exclusion_writes_real_backend() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "me.fakerqu.test.storageredirect".to_string(),
            uid: 10288,
            app_data_dir: "/data/user/0/me.fakerqu.test.storageredirect".to_string(),
            redirect_target:
                "/storage/emulated/0/Android/data/me.fakerqu.test.storageredirect/sdcard"
                    .to_string(),
            mount_root: Some("/storage/emulated/0/Download/SrtMonitorLocked".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: vec![
                "Download/SrtMonitorLocked".to_string(),
                "!Download/SrtMonitorLocked/Writable".to_string(),
            ],
            path_mappings: Vec::new(),
            is_mapping_mode_only: false,
        })
        .expect("policy");

        let decision = policy.backend_decision(
            "/storage/emulated/0/Download/SrtMonitorLocked/Writable/a.bin",
            OperationKind::Write,
        );

        assert!(matches!(decision.kind, BackendKind::Real));
        assert!(!decision.is_read_only);
    }

    #[test]
    fn scoped_fuse_read_only_matches_mediastore_pending_display_names() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "me.fakerqu.test.storageredirect".to_string(),
            uid: 10288,
            app_data_dir: "/data/user/0/me.fakerqu.test.storageredirect".to_string(),
            redirect_target:
                "/storage/emulated/0/Android/data/me.fakerqu.test.storageredirect/sdcard"
                    .to_string(),
            mount_root: Some("/storage/emulated/0/Download/SrtMonitorLocked".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: vec![
                "Download/SrtMonitorLocked".to_string(),
                "!Download/SrtMonitorLocked/Writable".to_string(),
            ],
            path_mappings: Vec::new(),
            is_mapping_mode_only: false,
        })
        .expect("policy");

        let locked = policy.backend_decision(
            "/storage/emulated/0/Download/SrtMonitorLocked/.pending-1783286547515-srt_monitor_27_media-read-only-denied.bin",
            OperationKind::Write,
        );
        let excluded = policy.backend_decision(
            "/storage/emulated/0/Download/SrtMonitorLocked/Writable/.pending-1783286547516-srt_monitor_27_media-read-only-excluded.bin",
            OperationKind::Write,
        );

        assert!(matches!(locked.kind, BackendKind::Real));
        assert!(locked.is_read_only);
        assert!(matches!(excluded.kind, BackendKind::Real));
        assert!(!excluded.is_read_only);
    }

    #[test]
    fn fuse_read_only_deny_line_records_qq_write_failure() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "com.tencent.mobileqq".to_string(),
            uid: 10288,
            app_data_dir: "/data/user/0/com.tencent.mobileqq".to_string(),
            redirect_target: "/storage/emulated/0/Android/data/com.tencent.mobileqq/sdcard"
                .to_string(),
            mount_root: Some("/storage/emulated/0/Download".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: vec!["Download".to_string()],
            path_mappings: Vec::new(),
            is_mapping_mode_only: false,
        })
        .expect("policy");
        let backend = policy
            .backend_for_relative("QQ/save.jpg", OperationKind::Write)
            .expect("backend");

        let line = policy
            .monitor_read_only_deny_line("create", &backend, None, libc::EROFS)
            .expect("monitor line");

        assert!(line.contains(
            "|com.tencent.mobileqq|com.tencent.mobileqq|CREATE|/storage/emulated/0/Download/QQ/save.jpg|ret=-1|errno=30|"
        ));
        assert!(line.contains("|identify_method=fuse_redirect|"));
        assert!(line.contains("|op=create|source=fuse_redirect|"));
        assert!(line.contains("|deny_reason=read_only_rule"));
    }

    #[test]
    fn shared_public_backend_mode_keeps_media_rw_group_access() {
        assert_eq!(adjust_metadata_mode(0o600, true, false), 0o660);
        assert_eq!(adjust_metadata_mode(0o644, true, false), 0o660);
        assert_eq!(adjust_metadata_mode(0o700, true, true), 0o2770);
        assert_eq!(adjust_metadata_mode(0o600, false, false), 0o600);
    }

    #[test]
    fn shared_public_backend_detection_excludes_android_private_dirs() {
        assert!(!is_android_app_private_relative_path("Download/1DMP/a.zip"));
        assert!(is_android_app_private_relative_path(
            "Android/data/com.example/cache/a.bin"
        ));
        assert!(is_android_app_private_relative_path(
            "Android/media/com.example/a.bin"
        ));
    }

    #[test]
    fn mapped_public_target_backend_is_marked_shared_public() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "idm.internet.download.manager.plus".to_string(),
            uid: 10367,
            app_data_dir: "/data/user/0/idm.internet.download.manager.plus".to_string(),
            redirect_target:
                "/storage/emulated/0/Android/data/idm.internet.download.manager.plus/sdcard"
                    .to_string(),
            mount_root: Some("/storage/emulated/0/Download".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![PathMapping::new(
                "Download/1DMP".to_string(),
                "Download/第三方下载/1DMP".to_string(),
            )],
            is_mapping_mode_only: true,
        })
        .expect("policy");

        let backend = policy
            .backend_for_relative("1DMP/storage.redirect.x.zip", OperationKind::Write)
            .expect("backend");

        assert!(backend.is_shared_public_backend);
        assert_eq!(
            backend.path.to_string_lossy(),
            "/data/media/0/Download/第三方下载/1DMP/storage.redirect.x.zip"
        );
    }

    #[test]
    fn real_backend_override_routes_allowed_paths_through_anchor() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "me.fakerqu.test.storageredirect".to_string(),
            uid: 10288,
            app_data_dir: "/data/user/0/me.fakerqu.test.storageredirect".to_string(),
            redirect_target:
                "/storage/emulated/0/Android/data/me.fakerqu.test.storageredirect/sdcard"
                    .to_string(),
            mount_root: Some("/storage/emulated/0/DCIM/SrtFuseQQ".to_string()),
            real_root_override: Some(
                "/data/adb/modules/storage.redirect.x/tmp/real_storage/0".to_string(),
            ),
            is_file_monitor_enabled: false,
            allowed_real_paths: vec!["DCIM/SrtFuseQQ/SrtAllowed*".to_string()],
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: Vec::new(),
            is_mapping_mode_only: false,
        })
        .expect("policy");

        let allowed_backend = policy
            .backend_for_relative("SrtAllowedAlpha/srt_ci_probe.txt", OperationKind::Write)
            .expect("allowed backend");
        assert_eq!(
            allowed_backend.path.to_string_lossy(),
            "/data/adb/modules/storage.redirect.x/tmp/real_storage/0/DCIM/SrtFuseQQ/SrtAllowedAlpha/srt_ci_probe.txt"
        );
        assert!(allowed_backend.is_shared_public_backend);

        let miss_backend = policy
            .backend_for_relative("SrtOther/srt_ci_probe.txt", OperationKind::Write)
            .expect("miss backend");
        assert_eq!(
            miss_backend.path.to_string_lossy(),
            "/data/media/0/Android/data/me.fakerqu.test.storageredirect/sdcard/DCIM/SrtFuseQQ/SrtOther/srt_ci_probe.txt"
        );
    }

    #[test]
    fn path_mappings_skip_android_private_targets() {
        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "com.example".to_string(),
            uid: 10123,
            app_data_dir: "/data/user/0/com.example".to_string(),
            redirect_target: "/storage/emulated/0/Android/data/com.example/sdcard".to_string(),
            mount_root: Some("/storage/emulated/0/Download".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: true,
            allowed_real_paths: Vec::new(),
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: vec![
                PathMapping::new(
                    "Download/App".to_string(),
                    "Android/data/com.example/files".to_string(),
                ),
                PathMapping::new(
                    "Download/Obb".to_string(),
                    "Android/obb/com.example".to_string(),
                ),
                PathMapping::new("Download/Public".to_string(), "Pictures/Public".to_string()),
            ],
            is_mapping_mode_only: true,
        })
        .expect("policy");

        assert_eq!(
            policy.path_mappings,
            vec![PathMapping::new(
                "/storage/emulated/0/Download/Public".to_string(),
                "/storage/emulated/0/Pictures/Public".to_string(),
            )]
        );
    }

    #[test]
    fn wildcard_rule_keeps_potential_descendants_on_real_side() {
        assert!(rule_may_match_path_or_descendant(
            "/storage/emulated/0/Download/*/foo",
            "/storage/emulated/0/Download"
        ));
        assert!(rule_may_match_path_or_descendant(
            "/storage/emulated/0/Download/*/foo",
            "/storage/emulated/0/Download/bar"
        ));
        assert!(!rule_may_match_path_or_descendant(
            "/storage/emulated/0/Download/*/foo",
            "/storage/emulated/0/Pictures"
        ));
    }

    #[test]
    fn scoped_fuse_wildcard_miss_under_mount_root_redirects() {
        let allowed = vec!["DCIM/SrtFuseQQ/SrtAllowed*".to_string()];
        let roots = scoped_mount_roots_for_hybrid_rules(10288, &allowed, &[], &[], &[], &[], false);
        assert_eq!(
            roots,
            vec!["/storage/emulated/0/DCIM/SrtFuseQQ".to_string()]
        );

        let policy = RedirectPolicy::new(FuseRedirectConfig {
            package_name: "me.fakerqu.test.storageredirect".to_string(),
            uid: 10288,
            app_data_dir: "/data/user/0/me.fakerqu.test.storageredirect".to_string(),
            redirect_target:
                "/storage/emulated/0/Android/data/me.fakerqu.test.storageredirect/sdcard"
                    .to_string(),
            mount_root: Some("/storage/emulated/0/DCIM/SrtFuseQQ".to_string()),
            real_root_override: None,
            is_file_monitor_enabled: false,
            allowed_real_paths: allowed,
            excluded_real_paths: Vec::new(),
            sandboxed_paths: Vec::new(),
            read_only_paths: Vec::new(),
            path_mappings: Vec::new(),
            is_mapping_mode_only: false,
        })
        .expect("policy");

        let allowed_decision = policy.backend_decision(
            "/storage/emulated/0/DCIM/SrtFuseQQ/SrtAllowedAlpha/srt_ci_probe.txt",
            OperationKind::Write,
        );
        assert!(matches!(allowed_decision.kind, BackendKind::Real));

        let miss_decision = policy.backend_decision(
            "/storage/emulated/0/DCIM/SrtFuseQQ/SrtOther/srt_ci_probe.txt",
            OperationKind::Write,
        );
        assert!(matches!(miss_decision.kind, BackendKind::Redirect));
    }
}
