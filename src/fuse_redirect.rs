use crate::domain::{PathMapping, sort_path_mappings_shortest_request_first};
use crate::platform::{fs, module_paths, paths};
use fuser::{
    AccessFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, InitFlags, KernelConfig, LockOwner, MountOption, OpenAccMode, OpenFlags, RenameFlags,
    ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen,
    ReplyStatfs, ReplyWrite, Request, SessionACL, TimeOrNow, WriteFlags,
};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_millis(250);
const ROOT_INO: u64 = 1;
const MAX_READ_SIZE: usize = 256 * 1024;
const MEDIA_RW_UID: u32 = 1023;
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
        if let Some(root) = scoped_mount_root_for_wildcard_prefix(&prefix, &storage_root) {
            roots.push(root);
        }
    }
    compact_scoped_mount_roots(roots, &storage_root)
}

fn scoped_mount_root_for_wildcard_prefix(prefix: &str, storage_root: &str) -> Option<String> {
    if prefix.is_empty() || !paths::is_child(prefix, storage_root) {
        return Some(storage_root.to_string());
    }
    if let Some(root) = public_collection_mount_root(prefix, storage_root) {
        return Some(root);
    }
    Some(prefix.to_string())
}

fn public_collection_mount_root(prefix: &str, storage_root: &str) -> Option<String> {
    public_collection_name(prefix, storage_root).map(|first| paths::join(storage_root, first))
}

fn public_collection_name<'a>(prefix: &'a str, storage_root: &str) -> Option<&'a str> {
    let rel = paths::relative_child_path(prefix, storage_root)?;
    let first = rel.split('/').find(|part| !part.is_empty())?;
    match first {
        "Alarms" | "Audiobooks" | "DCIM" | "Documents" | "Download" | "Movies" | "Music"
        | "Notifications" | "Pictures" | "Podcasts" | "Recordings" | "Ringtones" => Some(first),
        _ => None,
    }
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
    let scoped_allowed_rules = allowed_real_paths.iter().map(String::as_str);
    let mut roots = scoped_mount_roots_for_wildcard_rules(
        uid,
        scoped_allowed_rules
            .chain(excluded_real_paths.iter().map(String::as_str))
            .chain(sandboxed_paths.iter().map(String::as_str))
            .chain(read_only_paths.iter().map(String::as_str)),
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

    for allowed_path in allowed_real_paths {
        let allowed_root =
            resolve_concrete_scoped_rule_parent(allowed_path, user_id, &storage_root);
        if !allowed_root.is_empty() {
            roots.push(allowed_root);
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
                (!paths::contains_wildcards(request_path)
                    && paths::is_child(request_path, read_only_root))
                    || (!paths::contains_wildcards(final_path)
                        && paths::is_child(final_path, read_only_root))
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
    perf: FusePerfStats,
}

struct FusePerfStats {
    package_name: String,
    calls: AtomicU64,
    lookup_calls: AtomicU64,
    metadata_calls: AtomicU64,
    open_calls: AtomicU64,
    read_calls: AtomicU64,
    write_calls: AtomicU64,
    mutation_calls: AtomicU64,
    sampled_calls: AtomicU64,
    sampled_ns: AtomicU64,
    slow_samples: AtomicU64,
}

struct FusePerfSample<'a> {
    stats: Option<&'a FusePerfStats>,
    started: Option<Instant>,
    snapshot: bool,
}

impl FusePerfStats {
    fn new(package_name: String) -> Self {
        Self {
            package_name,
            calls: AtomicU64::new(0),
            lookup_calls: AtomicU64::new(0),
            metadata_calls: AtomicU64::new(0),
            open_calls: AtomicU64::new(0),
            read_calls: AtomicU64::new(0),
            write_calls: AtomicU64::new(0),
            mutation_calls: AtomicU64::new(0),
            sampled_calls: AtomicU64::new(0),
            sampled_ns: AtomicU64::new(0),
            slow_samples: AtomicU64::new(0),
        }
    }

    fn observe<'a>(&'a self, counter: &AtomicU64) -> FusePerfSample<'a> {
        if !crate::logging::is_debug_logging_enabled() {
            return FusePerfSample {
                stats: None,
                started: None,
                snapshot: false,
            };
        }
        counter.fetch_add(1, Ordering::Relaxed);
        let calls = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        FusePerfSample {
            stats: Some(self),
            started: calls.is_multiple_of(256).then(Instant::now),
            snapshot: calls.is_multiple_of(4096),
        }
    }

    fn log_snapshot(&self) {
        let sampled = self.sampled_calls.load(Ordering::Relaxed);
        let sampled_ns = self.sampled_ns.load(Ordering::Relaxed);
        log::debug!(
            "perf_snapshot component=fuse pkg={} calls={} lookup={} metadata={} open={} read={} write={} mutation={} samples={} avg_sample_us={} slow_samples={}",
            self.package_name,
            self.calls.load(Ordering::Relaxed),
            self.lookup_calls.load(Ordering::Relaxed),
            self.metadata_calls.load(Ordering::Relaxed),
            self.open_calls.load(Ordering::Relaxed),
            self.read_calls.load(Ordering::Relaxed),
            self.write_calls.load(Ordering::Relaxed),
            self.mutation_calls.load(Ordering::Relaxed),
            sampled,
            sampled_ns.checked_div(sampled.max(1)).unwrap_or(0) / 1000,
            self.slow_samples.load(Ordering::Relaxed),
        );
    }
}

impl Drop for FusePerfSample<'_> {
    fn drop(&mut self) {
        let Some(stats) = self.stats else {
            return;
        };
        if let Some(started) = self.started {
            let elapsed_ns = started.elapsed().as_nanos().min(u64::MAX as u128) as u64;
            stats.sampled_calls.fetch_add(1, Ordering::Relaxed);
            stats.sampled_ns.fetch_add(elapsed_ns, Ordering::Relaxed);
            if elapsed_ns >= 5_000_000 {
                stats.slow_samples.fetch_add(1, Ordering::Relaxed);
            }
        }
        if self.snapshot {
            stats.log_snapshot();
        }
    }
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
            state: Mutex::new(FuseState {
                next_ino: ROOT_INO + 1,
                next_fh: 1,
                inodes,
                paths_by_inode,
                lookup_counts: HashMap::new(),
                dir_entry_refs: HashMap::new(),
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
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, rel);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.visible_attr_for_backend(ino, &backend) {
            Ok(attr) => reply.entry(&TTL, &attr, Generation(0)),
            Err(errno) => {
                let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
                MAPPED_DIR_MODE as u32,
                backend.is_shared_public_backend,
                true,
            );
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
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
            let entries: Arc<[DirEntry]> =
                build_dir_entries(&mut state, &self.policy, ino, &rel).into();
            add_dir_entry_refs(&mut state, &entries);
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
        let _perf = self.perf.observe(&self.perf.read_calls);
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
                let entries: Arc<[DirEntry]> =
                    build_dir_entries(&mut state, &self.policy, ino, &rel).into();
                add_dir_entry_refs(&mut state, &entries);
                state.dirs.insert(fh.into(), Arc::clone(&entries));
                entries
            }
        };

        for (index, entry) in entries.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry.ino, (index + 1) as u64, entry.kind, &entry.name) {
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
        let _perf = self.perf.observe(&self.perf.read_calls);
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
        let _perf = self.perf.observe(&self.perf.write_calls);
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

    fn flush(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _lock_owner: LockOwner,
        reply: ReplyEmpty,
    ) {
        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
        let ino = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, &rel);
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        let attr = match self.attr_for_backend(ino, &backend) {
            Ok(attr) => attr,
            Err(errno) => {
                let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
                Self::remove_lookup_locked(&mut state, ino, 1);
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
        let ino = {
            let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
                let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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
                let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
                remap_inode_path(&mut state, &old_rel, &new_rel);
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
            let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
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

        let rule_prefixes =
            build_rule_prefixes(&config, &path_mappings, user_id, &storage_root, &mount_root);

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
        } else if self.matches_any(&self.read_only_excluded_paths, storage_path)
            || self.matches_any(&self.allowed_real_paths, storage_path)
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
    lookup_counts: HashMap<u64, u64>,
    dir_entry_refs: HashMap<u64, u64>,
    files: HashMap<u64, OpenFile>,
    dirs: HashMap<u64, Arc<[DirEntry]>>,
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
    config: &FuseRedirectConfig,
    path_mappings: &[PathMapping],
    user_id: i32,
    storage_root: &str,
    mount_root: &str,
) -> Vec<RulePrefix> {
    let mut prefixes = Vec::new();
    for rule in config
        .allowed_real_paths
        .iter()
        .chain(config.excluded_real_paths.iter())
        .chain(config.sandboxed_paths.iter())
        .chain(config.read_only_paths.iter())
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
    let effective_uid = if is_shared_public_backend {
        MEDIA_RW_UID
    } else {
        owner_uid as u32
    };
    if let Ok(c_path) = cstring_path(path) {
        // SAFETY: c_path is NUL-terminated and valid for the duration of chown.
        let _ = unsafe { libc::chown(c_path.as_ptr(), effective_uid, MEDIA_RW_GID) };
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

fn add_dir_entry_refs(state: &mut FuseState, entries: &[DirEntry]) {
    for entry in entries {
        if entry.ino.0 == ROOT_INO {
            continue;
        }
        let count = state.dir_entry_refs.entry(entry.ino.0).or_default();
        *count = count.saturating_add(1);
    }
}

fn remove_dir_entry_refs(state: &mut FuseState, entries: &[DirEntry]) {
    for entry in entries {
        if entry.ino.0 == ROOT_INO {
            continue;
        }
        if let Some(count) = state.dir_entry_refs.get_mut(&entry.ino.0) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.dir_entry_refs.remove(&entry.ino.0);
            }
        }
        remove_unreferenced_inode(state, entry.ino.0);
    }
}

fn remove_unreferenced_inode(state: &mut FuseState, ino: u64) {
    if ino == ROOT_INO
        || state.lookup_counts.contains_key(&ino)
        || state.dir_entry_refs.contains_key(&ino)
    {
        return;
    }
    if let Some(rel) = state.paths_by_inode.remove(&ino) {
        state.inodes.remove(&rel);
    }
}

fn remove_inode_path(state: &mut FuseState, rel: &str) {
    if let Some(ino) = state.inodes.remove(rel) {
        state.paths_by_inode.remove(&ino);
        state.lookup_counts.remove(&ino);
        state.dir_entry_refs.remove(&ino);
    }
}

fn remap_inode_path(state: &mut FuseState, old_rel: &str, new_rel: &str) {
    if old_rel == new_rel {
        return;
    }
    remove_inode_path(state, new_rel);
    if let Some(ino) = state.inodes.remove(old_rel) {
        state.inodes.insert(new_rel.to_string(), ino);
        state.paths_by_inode.insert(ino, new_rel.to_string());
    }
}

fn rename_noreplace(old_path: &Path, new_path: &Path) -> Result<(), Errno> {
    let old_path = cstring_path(old_path)?;
    let new_path = cstring_path(new_path)?;
    // SAFETY: Both pointers reference live, NUL-terminated C strings for the syscall duration.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            old_path.as_ptr(),
            libc::AT_FDCWD,
            new_path.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(errno_from_code(last_errno()))
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
        Some("data" | "media" | "obb") => true,
        _ => false,
    }
}
