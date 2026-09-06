use fuser::{FileAttr, FileType, INodeNo};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(super) fn file_type_from_std(file_type: std::fs::FileType) -> FileType {
    if file_type.is_dir() {
        FileType::Directory
    } else if file_type.is_symlink() {
        FileType::Symlink
    } else {
        FileType::RegularFile
    }
}

pub(super) fn file_type_from_mode(mode: u32) -> FileType {
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

pub(super) fn file_attr_from_metadata(ino: INodeNo, metadata: std::fs::Metadata) -> FileAttr {
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

pub(super) fn synthetic_dir_attr(ino: INodeNo, uid: u32, gid: u32) -> FileAttr {
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
