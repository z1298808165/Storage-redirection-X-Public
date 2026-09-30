use super::{CompanionMountForkPlan, FuseMountState};
use crate::lifecycle::companion_request::CompanionMountRequest;
use crate::platform::module_paths;

pub(super) fn write_mount_state(
    request: &CompanionMountRequest,
    plan: &CompanionMountForkPlan,
    targets: &[String],
    fuse_children: &[FuseMountState],
) -> bool {
    crate::fuse_session::write_mount_state(
        request.pid,
        request.uid,
        &request.package_name,
        request.config_version,
        plan.state_path.as_str(),
        plan.temp_state_path.as_str(),
        targets,
        fuse_children,
    )
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
