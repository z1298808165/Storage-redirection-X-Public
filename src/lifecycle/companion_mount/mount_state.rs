use super::FuseMountState;
use super::sys::{errno_text, last_errno};
use crate::lifecycle::companion_request::CompanionMountRequest;
use crate::platform::{fs, module_paths};
use libc::{O_CLOEXEC, O_CREAT, O_TRUNC, O_WRONLY, chmod, open};
use std::ffi::CString;

pub(super) fn write_mount_state(
    request: &CompanionMountRequest,
    targets: &[String],
    fuse_children: &[FuseMountState],
) -> bool {
    if request.pid <= 0 || request.package_name.is_empty() {
        return false;
    }
    if std::fs::create_dir_all(module_paths::MOUNT_STATE_DIR).is_err() {
        log::warn!(
            "mount state mkdir failed dir={}",
            module_paths::MOUNT_STATE_DIR
        );
        return false;
    }

    let state_path = state_file_path(request);
    let Ok(c_path) = CString::new(state_path.clone()) else {
        return false;
    };
    let fd = unsafe {
        open(
            c_path.as_ptr(),
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        let errno = last_errno();
        log::warn!(
            "mount state open failed path={} errno={} {}",
            state_path,
            errno,
            errno_text(errno)
        );
        return false;
    }

    let mut content = String::new();
    content.push_str(&format!("version={}\n", request.config_version));
    content.push_str(&format!("package={}\n", request.package_name));
    content.push_str(&format!("uid={}\n", request.uid));
    for state in fuse_children {
        content.push_str(&format!("fuse_child={}\n", state.child));
    }
    let mut all_targets = targets.to_vec();
    all_targets.extend(fuse_children.iter().map(|state| state.target.clone()));
    for target in normalize_targets(&all_targets) {
        content.push_str("target=");
        content.push_str(&target);
        content.push('\n');
    }

    let ok = fs::write_all(fd, content.as_bytes());
    unsafe {
        libc::fsync(fd);
        libc::close(fd);
        let _ = chmod(c_path.as_ptr(), 0o600);
    }
    if ok {
        log::info!(
            "mount state saved pid={} targets={} path={}",
            request.pid,
            targets.len(),
            state_path
        );
    }
    ok
}

fn state_file_path(request: &CompanionMountRequest) -> String {
    let safe_package = sanitize_name(&request.package_name);
    format!(
        "{}/{}_{}.state",
        module_paths::MOUNT_STATE_DIR,
        safe_package,
        request.pid
    )
}

fn normalize_targets(targets: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = targets
        .iter()
        .filter(|target| is_safe_mount_target(target))
        .cloned()
        .collect();
    normalized.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| b.cmp(a)));
    normalized.dedup();
    normalized
}

fn is_safe_mount_target(target: &str) -> bool {
    if target.is_empty() || target.contains('\0') || target.contains("/../") {
        return false;
    }
    target.starts_with("/storage/")
        || target.starts_with("/mnt/")
        || target.starts_with(module_paths::REAL_STORAGE_TMP_PREFIX)
}

fn sanitize_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
