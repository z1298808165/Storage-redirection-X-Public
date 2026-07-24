mod config;
mod perf;
mod policy;

pub use config::{
    FuseRedirectConfig, mount_blocking_with_ready, scoped_mount_roots_for_hybrid_rules,
};

use crate::platform::{fs, paths};
use fuser::{
    AccessFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, InitFlags, KernelConfig, LockOwner, OpenAccMode, OpenFlags, RenameFlags, ReplyAttr,
    ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs,
    ReplyWrite, Request, TimeOrNow, WriteFlags,
};
use perf::FusePerfStats;
use policy::{BackendPath, OperationKind, RedirectPolicy};
use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TTL: Duration = Duration::from_millis(250);
const ROOT_INO: u64 = 1;
const MAX_READ_SIZE: usize = 256 * 1024;
const MEDIA_RW_UID: u32 = 1023;
pub(super) const MEDIA_RW_GID: u32 = 1023;
pub(super) const MAPPED_DIR_MODE: libc::mode_t = 0o2773;
const SHARED_PUBLIC_DIR_MODE: u32 = 0o2770;
pub(super) const MAX_SCOPED_FUSE_ROOTS: usize = 4;

struct FuseRedirectFs {
    policy: RedirectPolicy,
    state: Mutex<FuseState>,
    perf: FusePerfStats,
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
                MAPPED_DIR_MODE,
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
        for (index, entry) in entries.iter().enumerate().skip(offset as usize) {
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

fn build_dir_entries(
    state: &mut FuseState,
    policy: &policy::RedirectPolicy,
    ino: INodeNo,
    rel: &str,
) -> Vec<DirEntry> {
    use crate::platform::paths;
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
    policy: &policy::RedirectPolicy,
    parent_rel: &str,
    backend: &Path,
    entries: &mut Vec<DirEntry>,
    seen: &mut HashMap<String, usize>,
) {
    use crate::platform::paths;
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
    policy: &policy::RedirectPolicy,
    parent_rel: &str,
    entries: &mut Vec<DirEntry>,
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

fn cstring_path(path: &Path) -> Result<CString, Errno> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| Errno::EINVAL)
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
        // SAFETY: c_path 以 NUL 结尾，并在 chown 调用期间保持有效。
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
    use std::os::unix::fs::PermissionsExt as _;
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
    // SAFETY: 两个指针均指向有效且以 NUL 结尾的 C 字符串，并在 syscall 调用期间保持有效。
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

fn paths_eq(left: &Path, right: &Path) -> bool {
    left == right
}
