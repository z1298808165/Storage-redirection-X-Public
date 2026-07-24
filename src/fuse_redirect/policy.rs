use crate::domain::{PathMapping, sort_path_mappings_shortest_request_first};
use crate::platform::{fs, module_paths, paths};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use super::FuseRedirectConfig;

const FILE_MONITOR_LOG_TAG: &str = "FileMonitorOp";
const READ_ONLY_DENY_EXTRA: &str = "deny_reason=read_only_rule";
const DUPLICATE_MONITOR_CREATE_WINDOW_MS: i64 = 1500;
const MAX_RECENT_MONITOR_CREATES: usize = 256;

static RECENT_MONITOR_CREATES: Lazy<Mutex<HashMap<String, i64>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub(super) struct BackendPath {
    pub(super) rel: String,
    pub(super) path: PathBuf,
    pub(super) is_read_only: bool,
    pub(super) is_shared_public_backend: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BackendKind {
    Real,
    Redirect,
}

#[derive(Clone, Copy)]
pub(super) enum OperationKind {
    Read,
    Write,
}

struct BackendDecision {
    kind: BackendKind,
    is_read_only: bool,
}

#[derive(Clone)]
pub(super) struct RulePrefix {
    pub(super) rel: String,
    pub(super) full_prefix: bool,
}

#[derive(Clone)]
pub(super) struct RedirectPolicy {
    pub(super) package_name: String,
    pub(super) uid: i32,
    pub(super) user_id: i32,
    pub(super) storage_root: String,
    pub(super) mount_rel: String,
    pub(super) real_root: PathBuf,
    pub(super) redirect_root: PathBuf,
    pub(super) rule_prefixes: Vec<RulePrefix>,
    pub(super) allowed_real_paths: Vec<String>,
    pub(super) excluded_real_paths: Vec<String>,
    pub(super) sandboxed_paths: Vec<String>,
    pub(super) read_only_paths: Vec<String>,
    pub(super) read_only_excluded_paths: Vec<String>,
    pub(super) path_mappings: Vec<PathMapping>,
    pub(super) is_mapping_mode_only: bool,
    pub(super) is_file_monitor_enabled: bool,
}

impl RedirectPolicy {
    pub(super) fn new(config: FuseRedirectConfig) -> Option<Self> {
        let user_id = config.user_id();
        let storage_root = paths::storage_user_root_for_user(user_id);
        let mount_root = super::config::fuse_mount_point(&config, user_id);
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

        let normalized_read_only_paths =
            super::normalize_rule_list(config.read_only_paths, user_id);
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
            allowed_real_paths: super::normalize_rule_list(config.allowed_real_paths, user_id),
            excluded_real_paths: super::normalize_rule_list(config.excluded_real_paths, user_id),
            sandboxed_paths: super::normalize_rule_list(config.sandboxed_paths, user_id),
            read_only_paths,
            read_only_excluded_paths,
            path_mappings,
            is_mapping_mode_only: config.is_mapping_mode_only,
            is_file_monitor_enabled: config.is_file_monitor_enabled,
        })
    }

    pub(super) fn backend_for_relative(
        &self,
        rel: &str,
        operation: OperationKind,
    ) -> Option<BackendPath> {
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

    pub(super) fn resolve_mapping(&self, storage_path: &str) -> Option<String> {
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

    pub(super) fn matches_any(&self, rules: &[String], storage_path: &str) -> bool {
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

    pub(super) fn is_virtual_dir(&self, rel: &str) -> bool {
        let storage_path = self.storage_path_for_rel(rel);
        self.rule_prefixes.iter().any(|prefix| {
            let prefix_storage_path = self.storage_path_for_rel(&prefix.rel);
            paths::eq_ignore_case(&prefix.rel, rel)
                || paths::is_child(&prefix_storage_path, &storage_path)
                || (!prefix.full_prefix
                    && rule_has_path_prefix(&storage_path, &prefix_storage_path))
        })
    }

    pub(super) fn real_backend_for_rel(&self, rel: &str) -> PathBuf {
        let rel = self.full_storage_rel(rel);
        self.real_backend_for_storage_rel(&rel)
    }

    pub(super) fn redirect_backend_for_rel(&self, rel: &str) -> PathBuf {
        let rel = self.full_storage_rel(rel);
        self.redirect_backend_for_storage_rel(&rel)
    }

    pub(super) fn real_backend_for_storage_rel(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.real_root.clone()
        } else {
            self.real_root.join(rel)
        }
    }

    pub(super) fn redirect_backend_for_storage_rel(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            self.redirect_root.clone()
        } else {
            self.redirect_root.join(rel)
        }
    }

    pub(super) fn backend_path_for_storage(
        &self,
        storage_path: &str,
        kind: BackendKind,
    ) -> Option<PathBuf> {
        let rel = paths::relative_child_path(storage_path, &self.storage_root)?;
        Some(match kind {
            BackendKind::Real => self.real_backend_for_storage_rel(rel),
            BackendKind::Redirect => self.redirect_backend_for_storage_rel(rel),
        })
    }

    pub(super) fn storage_path_for_rel(&self, rel: &str) -> String {
        let rel = self.full_storage_rel(rel);
        if rel.is_empty() {
            self.storage_root.clone()
        } else {
            paths::join(&self.storage_root, &rel)
        }
    }

    pub(super) fn emit_monitor_create(&self, backend: &BackendPath) {
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

    pub(super) fn emit_monitor_read_only_deny(&self, operation_name: &str, backend: &BackendPath) {
        self.emit_monitor_read_only_deny_with_from(operation_name, backend, None, libc::EROFS);
    }

    pub(super) fn emit_monitor_read_only_deny_with_errno(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        error_no: i32,
    ) {
        self.emit_monitor_read_only_deny_with_from(operation_name, backend, None, error_no);
    }

    pub(super) fn emit_monitor_read_only_deny_with_from(
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

    pub(super) fn monitor_read_only_deny_line(
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

    pub(super) fn full_storage_rel(&self, rel: &str) -> String {
        let rel = rel.trim_matches('/');
        if self.mount_rel.is_empty() {
            rel.to_string()
        } else if rel.is_empty() {
            self.mount_rel.clone()
        } else {
            paths::join(&self.mount_rel, rel)
        }
    }

    pub(super) fn is_shared_public_backend_path(&self, path: &Path) -> bool {
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

pub(super) fn real_backend_root_for_config(config: &FuseRedirectConfig, user_id: i32) -> PathBuf {
    if let Some(root) = config.real_root_override.as_deref() {
        let normalized = paths::normalize(root);
        if paths::eq_ignore_case(&normalized, &real_storage_anchor_for_user(user_id)) {
            return PathBuf::from(normalized);
        }
        log::warn!("fuse real root override ignored: {}", root);
    }
    PathBuf::from(paths::data_media_user_root_for_user(user_id))
}

pub(super) fn real_storage_anchor_for_user(user_id: i32) -> String {
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
    // SAFETY: now 是栈上的合法可写指针，libc::time 只写入一个 time_t 值。
    unsafe { libc::time(&mut now as *mut _) };

    // SAFETY: libc::tm 是 C ABI 结构体，全零初始化是合法的起始状态，后续由 localtime_r 填充。
    let mut tm_value: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: now 和 tm_value 均为有效的本地变量，指针在调用期间保持有效。
    let tm_ptr = unsafe { libc::localtime_r(&now as *const _, &mut tm_value as *mut _) };
    if tm_ptr.is_null() {
        return String::new();
    }

    let mut buffer = [0u8; 32];
    let format = b"%Y-%m-%d %H:%M:%S\0";
    // SAFETY: buffer 是有效的可写缓冲区，format 是以 NUL 结尾的合法格式字符串，tm_value 已由 localtime_r 填充，三个指针在调用期间均保持有效。
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

pub(super) fn build_rule_prefixes(
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

pub(super) fn visible_prefix_child(parent_rel: &str, prefix: &RulePrefix) -> Option<String> {
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

pub(super) fn sanitize_relative(rel: &str) -> Option<String> {
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

pub(super) fn monitor_event_kind_for_operation(operation_name: &str) -> &'static str {
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

pub(super) fn fix_mapped_dir_metadata(path: &str, owner_uid: i32) {
    if let Ok(c_path) = CString::new(path) {
        // SAFETY: c_path 以 NUL 结尾，并在 chown/chmod 调用期间保持有效。
        let _ = unsafe { libc::chown(c_path.as_ptr(), owner_uid as u32, super::MEDIA_RW_GID) };
        let _ = unsafe { libc::chmod(c_path.as_ptr(), super::MAPPED_DIR_MODE) };
    }
}

fn is_android_app_private_relative_path(relative: &str) -> bool {
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    matches!(parts.next(), Some("data" | "media" | "obb"))
}
