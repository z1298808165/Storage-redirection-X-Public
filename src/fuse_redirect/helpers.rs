use fuser::{OpenAccMode, OpenFlags};
use std::path::Path;

pub(super) fn open_flags_write(flags: i32) -> bool {
    let accmode = OpenFlags(flags).acc_mode();
    accmode == OpenAccMode::O_WRONLY || accmode == OpenAccMode::O_RDWR || flags & libc::O_TRUNC != 0
}

pub(super) fn fuse_open_operation_name(flags: i32) -> &'static str {
    if open_flags_write(flags) {
        "open:write"
    } else {
        "open:read"
    }
}

pub(super) fn fuse_setattr_operation_name(
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

pub(super) fn elapsed_ns(started: Option<std::time::Instant>) -> u64 {
    started
        .map(|value| value.elapsed().as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

pub(super) fn paths_eq(left: &Path, right: &Path) -> bool {
    left == right
}
