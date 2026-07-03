use super::SystemWriterContext;
use crate::config::ResolvedUserProfile;
use crate::domain::PathMapping;
use crate::redirect::PathRouter;

pub(super) struct RouteConfigSnapshot {
    pub(super) allowed_real_paths: Vec<String>,
    pub(super) excluded_real_paths: Vec<String>,
    pub(super) sandboxed_paths: Vec<String>,
    pub(super) read_only_paths: Vec<String>,
    pub(super) path_mappings: Vec<PathMapping>,
    pub(super) is_mapping_mode_only: bool,
}

impl RouteConfigSnapshot {
    pub(super) fn from_resolved_profile(profile: Option<&ResolvedUserProfile>) -> Self {
        Self {
            allowed_real_paths: profile
                .map(|profile| profile.allowed_real_paths.clone())
                .unwrap_or_default(),
            excluded_real_paths: profile
                .map(|profile| profile.excluded_real_paths.clone())
                .unwrap_or_default(),
            sandboxed_paths: profile
                .map(|profile| profile.sandboxed_paths.clone())
                .unwrap_or_default(),
            read_only_paths: profile
                .map(|profile| profile.read_only_paths.clone())
                .unwrap_or_default(),
            path_mappings: profile
                .map(|profile| profile.path_mappings.clone())
                .unwrap_or_default(),
            is_mapping_mode_only: profile
                .map(|profile| profile.is_mapping_mode_only)
                .unwrap_or(false),
        }
    }

    pub(super) fn log_config_summary(&self, package_name: &str) {
        log::info!(
            "config sum pkg={} allow={} excl={} sandbox={} ro={} map={} map_only={}",
            package_name,
            self.allowed_real_paths.len(),
            self.excluded_real_paths.len(),
            self.sandboxed_paths.len(),
            self.read_only_paths.len(),
            self.path_mappings.len(),
            self.is_mapping_mode_only
        );
    }

    pub(super) fn log_config_details(&self) {
        if !self.allowed_real_paths.is_empty() {
            log_allowed_real_paths(&self.allowed_real_paths);
        }
        if !self.excluded_real_paths.is_empty() {
            log_excluded_real_paths(&self.excluded_real_paths);
        }
        if !self.path_mappings.is_empty() {
            log_path_mappings(&self.path_mappings);
        }
        if !self.read_only_paths.is_empty() {
            log_read_only_paths(&self.read_only_paths);
        }
    }

    pub(super) fn apply_writer_override(
        &mut self,
        writer_context: &mut SystemWriterContext,
        is_system_writer_hook_redirect: bool,
    ) {
        if !writer_context.is_system_writer || !writer_context.has_merged_writer_mappings {
            return;
        }

        self.is_mapping_mode_only = true;
        if is_system_writer_hook_redirect {
            self.path_mappings.clear();
            log::info!("writer per-caller hook map, skip global mount");
            return;
        }

        self.path_mappings = std::mem::take(&mut writer_context.merged_writer_mappings);
        log::info!("writer merged map count={}", self.path_mappings.len());
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn configure_router(
        package_name: &str,
        app_uid: i32,
        redirect_base: &str,
        is_system_writer_hook_redirect: bool,
        allowed_real_paths: &[String],
        excluded_real_paths: &[String],
        sandboxed_paths: &[String],
        read_only_paths: &[String],
        path_mappings: &[PathMapping],
        is_mapping_mode_only: bool,
    ) {
        if is_system_writer_hook_redirect {
            PathRouter::instance().configure(
                package_name,
                app_uid,
                redirect_base,
                &[],
                &[],
                &[],
                &[],
                &[],
                false,
            );
            return;
        }

        PathRouter::instance().configure(
            package_name,
            app_uid,
            redirect_base,
            allowed_real_paths,
            excluded_real_paths,
            sandboxed_paths,
            read_only_paths,
            path_mappings,
            is_mapping_mode_only,
        );
    }
}

fn log_allowed_real_paths(paths: &[String]) {
    for path in paths {
        log::info!("cfg allow={}", path);
    }
}

fn log_excluded_real_paths(paths: &[String]) {
    for path in paths {
        log::info!("cfg excl={}", path);
    }
}

fn log_path_mappings(mappings: &[PathMapping]) {
    for mapping in mappings {
        log::info!("cfg map {} -> {}", mapping.request_path, mapping.final_path);
    }
}

fn log_read_only_paths(paths: &[String]) {
    for path in paths {
        log::info!("cfg readonly={}", path);
    }
}
