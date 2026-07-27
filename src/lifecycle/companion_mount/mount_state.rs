use super::sys::{errno_text, last_errno};
use super::{CompanionMountForkPlan, FuseMountState};
use crate::lifecycle::companion_request::CompanionMountRequest;
use crate::platform::{fs, module_paths};
use libc::{O_CLOEXEC, O_CREAT, O_TRUNC, O_WRONLY, chmod, open};
use std::ffi::CString;

pub(super) fn write_mount_state(
    request: &CompanionMountRequest,
    plan: &CompanionMountForkPlan,
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

    // 路径在 fork 之前已经算好，这里直接复用，避免子进程重复拼接字符串。
    let state_path = plan.state_path.as_str();
    let temp_path = plan.temp_state_path.as_str();
    let Ok(c_temp_path) = CString::new(temp_path) else {
        return false;
    };
    let fd = unsafe {
        open(
            c_temp_path.as_ptr(),
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        let errno = last_errno();
        log::warn!(
            "mount state open failed path={} errno={} {}",
            temp_path,
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
    for target in module_paths::normalize_mount_targets(&all_targets) {
        content.push_str("target=");
        content.push_str(&target);
        content.push('\n');
    }

    let mut ok = fs::write_all(fd, content.as_bytes());
    unsafe {
        if libc::fsync(fd) != 0 {
            ok = false;
        }
        libc::close(fd);
        let _ = chmod(c_temp_path.as_ptr(), 0o600);
    }
    // 先写临时文件再原子改名：写入中途失败时保留上一份有效状态，避免留下被截断的状态文件。
    if ok && std::fs::rename(temp_path, state_path).is_err() {
        ok = false;
        log::warn!(
            "mount state rename failed temp={} path={}",
            temp_path,
            state_path
        );
    }
    if ok {
        log::info!(
            "mount state saved pid={} targets={} path={}",
            request.pid,
            targets.len(),
            state_path
        );
    } else {
        let _ = std::fs::remove_file(temp_path);
    }
    ok
}

pub(super) fn state_file_path(request: &CompanionMountRequest) -> String {
    let safe_package = module_paths::sanitize_name(&request.package_name);
    format!(
        "{}/{}_{}.state",
        module_paths::MOUNT_STATE_DIR,
        safe_package,
        request.pid
    )
}
