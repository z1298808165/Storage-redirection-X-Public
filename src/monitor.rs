mod download_owner;
mod source_hint;
mod stack_owner;
mod thread_hint;

pub(crate) use download_owner::infer_download_owner_package_by_path;
pub(crate) use source_hint::{
    infer_recent_path_caller_identity, remember_private_path_caller_hint,
    remember_private_path_caller_hint_in_memory, remember_private_path_caller_uid_hint_in_memory,
    remember_private_path_owner_hint, remember_saf_path_caller_hint,
};
pub(crate) use stack_owner::infer_caller_package_by_stack;

use crate::config::SettingsHub;
use crate::platform::paths::monotonic_ms;
use crate::platform::{self, paths};
use crate::redirect::{policy, writer, writer::ANDROID_APP_UID_START};
use libc::{time, tm};
use once_cell::sync::Lazy;
use stack_owner::infer_package_from_java_stack;
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};
use thread_hint::infer_component_from_thread_name;

const LOGCAT_OP_TAG: &str = "FileMonitorOp";
const DUPLICATE_EVENT_WINDOW_MS: i64 = 1500;
// 优化：延长 caller package 时间窗口，减少误判
const RECENT_CALLER_PACKAGE_WINDOW_MS: i64 = 2500;
// 优化：延长 query access 时间窗口，提高路径推断准确性
const QUERY_ACCESS_WINDOW_MS: i64 = 8_000;
const MAX_RECENT_EVENTS: usize = 512;
const MAX_QUERY_ACCESS_PATHS: usize = 1024;

#[derive(Copy, Clone)]
pub enum OpKind {
    Open,
    Mkdir,
    Mknod,
    Rename,
    Unlink,
    Rmdir,
    Link,
    Symlink,
    Truncate,
    Chmod,
    Utimens,
}

struct AuditState {
    package_name: String,
    uid: i32,
    shared_uid_packages: String,
    caller_package: String,
    caller_package_updated_ms: i64,
    recent_event_ms: HashMap<String, i64>,
    recent_event_order: VecDeque<String>,
    query_access_paths: HashMap<String, QueryAccessRecord>,
    query_access_order: VecDeque<String>,
}

#[derive(Clone)]
struct QueryAccessRecord {
    package_name: String,
    uid: i32,
    updated_ms: i64,
    source: &'static str,
    confidence: &'static str,
}

struct MonitorStateSnapshot {
    package_name: String,
    uid: i32,
    shared_uid_packages: String,
}

impl AuditState {
    fn new() -> Self {
        Self {
            package_name: String::new(),
            uid: -1,
            shared_uid_packages: String::new(),
            caller_package: String::new(),
            caller_package_updated_ms: -1,
            recent_event_ms: HashMap::new(),
            recent_event_order: VecDeque::new(),
            query_access_paths: HashMap::new(),
            query_access_order: VecDeque::new(),
        }
    }
}

pub struct AuditTrail {
    is_enabled: AtomicBool,
    state: Mutex<AuditState>,
}

impl AuditTrail {
    pub fn instance() -> &'static AuditTrail {
        &AUDIT_TRAIL
    }

    pub fn init(&self, package_name: &str, uid: i32) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.package_name = package_name.to_string();
        state.uid = uid;
        state.caller_package.clear();
        state.caller_package_updated_ms = -1;
        state.recent_event_ms.clear();
        state.recent_event_order.clear();
        state.query_access_paths.clear();
        state.query_access_order.clear();

        if uid >= 0 && policy::is_shared_uid_process(uid) {
            state.shared_uid_packages = policy::get_shared_uid_packages_string(uid);
        } else {
            state.shared_uid_packages.clear();
        }

        if !state.shared_uid_packages.is_empty() {
            log::info!(
                "monitor init pkg={} shared_uid={}",
                state.package_name,
                state.shared_uid_packages
            );
        } else {
            log::info!("monitor init pkg={}", state.package_name);
        }
        true
    }

    pub fn update_caller_package(&self, caller_package: &str) {
        let normalized = extract_caller_package(caller_package);
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        state.caller_package = normalized;
        state.caller_package_updated_ms = monotonic_ms();
    }

    pub fn record_query_access_path(&self, path: &str, caller_uid: i32) {
        self.record_recent_path_caller(path, caller_uid, "", "query_access", "medium");
    }

    pub fn record_provider_open_path(&self, path: &str, caller_uid: i32, caller_package: &str) {
        self.record_recent_path_caller(path, caller_uid, caller_package, "provider_open", "high");
    }

    pub fn record_saf_provider_path(
        &self,
        path: &str,
        caller_uid: i32,
        caller_package: &str,
        op_filter: &str,
    ) -> bool {
        if !self.is_enabled() || !SettingsHub::instance().is_file_monitor_enabled() {
            return false;
        }
        let Some((normalized, package_name)) = self.record_recent_path_caller(
            path,
            caller_uid,
            caller_package,
            "provider_open",
            "high",
        ) else {
            return false;
        };
        let op_filter = normalize_provider_open_op_filter(op_filter);
        if SettingsHub::instance().should_filter_monitor_record(&normalized, op_filter) {
            return false;
        }
        remember_saf_path_caller_hint(
            &normalized,
            &package_name,
            caller_uid,
            "saf_provider",
            "high",
            op_filter,
        );
        let line = format!(
            "{}|{}|{}|OPEN|{}|ret=0|errno=0|identify_method=saf_provider|identify_reliability=high|op=provider_open|op_filter={}|source=saf_provider|caller_uid={}",
            build_timestamp_locked(),
            self.package_name(),
            package_name,
            normalized,
            op_filter,
            caller_uid
        );
        append_line_locked(&line);
        true
    }

    fn record_recent_path_caller(
        &self,
        path: &str,
        caller_uid: i32,
        caller_package: &str,
        source: &'static str,
        confidence: &'static str,
    ) -> Option<(String, String)> {
        if !self.is_enabled() || caller_uid < 10_000 {
            return None;
        }
        let normalized = normalize_storage_path_locked(path);
        if normalized.is_empty() || is_filtered_media_provider_path(&normalized) {
            return None;
        }
        if SettingsHub::instance().should_filter_monitor_record(&normalized, source) {
            return None;
        }
        let package_name = resolve_recent_path_caller_package(caller_uid, caller_package)?;
        let normalized_for_hint = normalized.clone();
        let package_for_hint = package_name.clone();
        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        remember_query_access_locked(
            &mut state,
            normalized,
            package_name,
            caller_uid,
            source,
            confidence,
        );
        let result = (normalized_for_hint.clone(), package_for_hint.clone());
        drop(state);
        source_hint::remember_public_path_caller_hint(
            &normalized_for_hint,
            &package_for_hint,
            caller_uid,
            source,
            confidence,
        );
        Some(result)
    }

    pub fn get_log_fd(&self) -> i32 {
        -1
    }

    pub fn is_enabled(&self) -> bool {
        self.is_enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, is_enabled: bool) {
        self.is_enabled.store(is_enabled, Ordering::Relaxed);
    }

    fn package_name(&self) -> String {
        self.state
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .package_name
            .clone()
    }

    pub fn record_operation_result(
        &self,
        kind: OpKind,
        caller_package: &str,
        path: &str,
        result: i32,
        error_no: i32,
        extra: &str,
    ) {
        if !self.is_enabled() {
            return;
        }

        // EEXIST 属于幂等检查，不记录
        if result == -1 && error_no == libc::EEXIST {
            return;
        }

        let normalized = normalize_storage_path_locked(path);
        if normalized.is_empty() {
            return;
        }
        // MediaProvider 合成目录跳过审计
        if is_filtered_media_provider_path(&normalized) {
            return;
        }

        let operation_name = operation_name_from_extra(kind, extra);
        let filter_operation = if is_read_only_rule_deny_extra(extra) {
            ""
        } else {
            &operation_name
        };
        if SettingsHub::instance().should_filter_monitor_record(&normalized, filter_operation) {
            return;
        }

        let state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        if should_filter_shared_uid_probe_event_locked(&state, &normalized, result, error_no) {
            return;
        }
        drop(state);

        let mut state = self.state.lock().unwrap_or_else(|err| err.into_inner());
        let source_identity = resolve_caller_info_for_path_locked(
            &state,
            caller_package,
            &normalized,
            is_mkdir_like_extra(extra),
            should_use_recent_private_owner_hint(kind, &normalized, &operation_name),
        );
        let event_key = build_event_key_locked(
            &state,
            &source_identity,
            kind,
            &operation_name,
            &normalized,
            result,
            error_no,
        );
        if should_skip_duplicate_event_locked(&mut state, &event_key) {
            return;
        }
        let state_snapshot = MonitorStateSnapshot {
            package_name: state.package_name.clone(),
            uid: state.uid,
            shared_uid_packages: state.shared_uid_packages.clone(),
        };
        drop(state);
        let extra_to_write = build_monitor_extra_with_backend(
            &state_snapshot,
            &source_identity,
            kind,
            &operation_name,
            &normalized,
            result,
            extra,
        );

        // 预分配缓冲区以减少字符串重分配
        let timestamp = build_timestamp_locked();
        let caller_pkg = if source_identity.package_name.is_empty() {
            "-"
        } else {
            source_identity.package_name.as_str()
        };

        let mut line = String::with_capacity(
            timestamp.len()
                + state_snapshot.package_name.len()
                + caller_pkg.len()
                + op_type_to_text(kind).len()
                + normalized.len()
                + extra_to_write.len()
                + 100,
        );

        line.push_str(&timestamp);
        line.push('|');
        line.push_str(&state_snapshot.package_name);
        line.push('|');
        line.push_str(caller_pkg);
        line.push('|');
        line.push_str(op_type_to_text(kind));
        line.push('|');
        line.push_str(&normalized);
        line.push_str("|ret=");
        line.push_str(&result.to_string());
        line.push_str("|errno=");
        line.push_str(&error_no.to_string());

        append_source_identity_meta(&mut line, &source_identity);
        if !extra_to_write.is_empty() {
            line.push('|');
            line.push_str(&extra_to_write);
        }
        append_line_locked(&line);
    }
}

static AUDIT_TRAIL: Lazy<AuditTrail> = Lazy::new(|| AuditTrail {
    is_enabled: AtomicBool::new(true),
    state: Mutex::new(AuditState::new()),
});

// 将 /data/media 别名统一转为 /storage/emulated 前缀
fn normalize_storage_path_locked(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }

    let normalized = paths::normalize(path);
    if paths::starts_with(&normalized, "/storage/emulated/") {
        return normalized;
    }

    if paths::starts_with(&normalized, "/data/media/") {
        return paths::data_media_to_storage_path(&normalized);
    }

    String::new()
}

fn is_filtered_media_provider_path(path: &str) -> bool {
    paths::is_filtered_media_provider_path(path)
}

fn should_filter_shared_uid_probe_event_locked(
    state: &AuditState,
    path: &str,
    result: i32,
    error_no: i32,
) -> bool {
    if result >= 0
        || state.shared_uid_packages.is_empty()
        || !policy::is_shared_uid_process(state.uid)
    {
        return false;
    }
    if error_no != libc::EACCES && error_no != libc::ENOTCONN {
        return false;
    }

    is_storage_root_or_android_probe_path(path)
}

fn is_storage_root_or_android_probe_path(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }

    if path == "/storage/emulated" {
        return true;
    }

    const PREFIX: &str = "/storage/emulated/";
    let Some(rest) = path.strip_prefix(PREFIX) else {
        return false;
    };
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let Some(user_id) = parts.next() else {
        return false;
    };
    if !user_id.chars().all(|ch| ch.is_ascii_digit()) {
        return false;
    }

    matches!(
        (parts.next(), parts.next()),
        (None, None) | (Some("Android"), None) | (Some(".xlDownload"), None)
    )
}

// 本地时间 YYYY-MM-DD HH:MM:SS
fn build_timestamp_locked() -> String {
    let mut now: libc::time_t = 0;
    unsafe { time(&mut now as *mut _) };

    let mut tm_value: tm = unsafe { std::mem::zeroed() };
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

fn append_line_locked(line: &str) {
    if line.is_empty() {
        return;
    }
    log::info!(target: LOGCAT_OP_TAG, "{}", line);
}

fn append_source_identity_meta(line: &mut String, identity: &SourceIdentity) {
    line.push('|');
    line.push_str("identify_method=");
    line.push_str(identity.source);
    line.push('|');
    line.push_str("identify_reliability=");
    line.push_str(identity.confidence);
}

fn build_monitor_extra_with_backend<'a>(
    state: &MonitorStateSnapshot,
    identity: &SourceIdentity,
    kind: OpKind,
    operation_name: &str,
    normalized_path: &str,
    result: i32,
    extra: &'a str,
) -> Cow<'a, str> {
    let Some(meta) = infer_monitor_redirect_backend(
        state,
        identity,
        kind,
        operation_name,
        normalized_path,
        result,
        extra,
    ) else {
        return Cow::Borrowed(extra);
    };

    let mut enriched = String::with_capacity(
        extra.len() + meta.backend_path.len() + meta.source.len() + normalized_path.len() + 32,
    );
    enriched.push_str(extra);
    append_monitor_extra_kv(&mut enriched, "backend", &meta.backend_path);
    if !extra_contains_key(extra, "from") {
        append_monitor_extra_kv(&mut enriched, "from", normalized_path);
    }
    if !extra_contains_key(extra, "source") {
        append_monitor_extra_kv(&mut enriched, "source", meta.source);
    }
    Cow::Owned(enriched)
}

struct MonitorRedirectBackend {
    backend_path: String,
    source: &'static str,
}

fn infer_monitor_redirect_backend(
    state: &MonitorStateSnapshot,
    identity: &SourceIdentity,
    kind: OpKind,
    operation_name: &str,
    normalized_path: &str,
    result: i32,
    extra: &str,
) -> Option<MonitorRedirectBackend> {
    if result < 0
        || normalized_path.is_empty()
        || extra_contains_key(extra, "backend")
        || is_read_only_rule_deny_extra(extra)
        || !is_redirect_backend_candidate(kind, operation_name)
    {
        return None;
    }

    let caller_package = identity.package_name.as_str();
    if caller_package.is_empty() || caller_package == "-" {
        return None;
    }
    let caller_uid = resolve_monitor_identity_uid(state, caller_package);
    if caller_uid < ANDROID_APP_UID_START {
        return None;
    }

    let user_id = paths::extract_user_id_from_storage_path(normalized_path);
    if user_id < 0 || platform::user_id_from_uid(caller_uid) != user_id {
        return None;
    }

    let config = SettingsHub::instance();
    let enablement = config.get_user_redirect_enablement(caller_package, caller_uid, user_id);
    if !enablement.is_enabled() {
        return None;
    }

    let mappings = writer::get_caller_mappings(caller_package, caller_uid);
    let mapped_path = writer::map_path_by_caller_mappings(normalized_path, &mappings);
    if !mapped_path.is_empty() && mapped_path != normalized_path {
        return monitor_backend_from_storage_path(mapped_path, "path_mapping");
    }

    if writer::is_path_excluded_by_caller_real_paths(normalized_path, caller_package, caller_uid) {
        return None;
    }

    if enablement.is_mapping_mode_only {
        if writer::is_path_sandboxed_by_caller_paths(normalized_path, caller_package, caller_uid) {
            return monitor_fallback_backend(
                normalized_path,
                caller_package,
                caller_uid,
                user_id,
                "sandbox_path",
            );
        }
        return None;
    }

    if writer::is_path_allowed_by_caller_real_paths(normalized_path, caller_package, caller_uid)
        || writer::is_path_read_only_excluded_by_caller_paths(
            normalized_path,
            caller_package,
            caller_uid,
        )
    {
        return None;
    }

    monitor_fallback_backend(
        normalized_path,
        caller_package,
        caller_uid,
        user_id,
        "redirect_root",
    )
}

fn monitor_fallback_backend(
    normalized_path: &str,
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
    source: &'static str,
) -> Option<MonitorRedirectBackend> {
    let redirect_target =
        writer::resolve_system_writer_redirect_target(caller_package, caller_uid, user_id, false);
    let fallback = writer::map_path_by_caller_fallback(normalized_path, &redirect_target, user_id);
    monitor_backend_from_storage_path(fallback, source)
}

fn monitor_backend_from_storage_path(
    storage_path: String,
    source: &'static str,
) -> Option<MonitorRedirectBackend> {
    if storage_path.is_empty() {
        return None;
    }
    let backend_path = writer::storage_to_data_media_path(&storage_path);
    if backend_path.is_empty() {
        return None;
    }
    Some(MonitorRedirectBackend {
        backend_path,
        source,
    })
}

fn resolve_monitor_identity_uid(state: &MonitorStateSnapshot, package_name: &str) -> i32 {
    if package_name.is_empty() {
        return -1;
    }
    let mut uid = policy::get_uid_for_package(package_name);
    if uid < ANDROID_APP_UID_START {
        uid = policy::get_fresh_uid_for_package(package_name);
    }
    if uid >= ANDROID_APP_UID_START {
        return uid;
    }
    if state.uid >= ANDROID_APP_UID_START
        && (state.package_name == package_name
            || state
                .shared_uid_packages
                .split(',')
                .any(|package| package.trim() == package_name))
    {
        return state.uid;
    }
    -1
}

fn is_redirect_backend_candidate(kind: OpKind, operation_name: &str) -> bool {
    matches!(kind, OpKind::Mkdir | OpKind::Mknod)
        || matches!(kind, OpKind::Open) && is_open_write_or_create_operation(operation_name)
}

fn append_monitor_extra_kv(extra: &mut String, key: &str, value: &str) {
    if value.is_empty() {
        return;
    }
    if !extra.is_empty() {
        extra.push('|');
    }
    extra.push_str(key);
    extra.push('=');
    extra.push_str(value);
}

fn extra_contains_key(extra: &str, key: &str) -> bool {
    let prefix = format!("{key}=");
    extra.split('|').any(|part| part.starts_with(&prefix))
}

fn is_read_only_rule_deny_extra(extra: &str) -> bool {
    extra
        .split('|')
        .any(|part| part.trim() == "deny_reason=read_only_rule")
}

pub(super) struct SourceIdentity {
    package_name: String,
    source: &'static str,
    confidence: &'static str,
}

impl SourceIdentity {
    pub(super) fn new(
        package_name: String,
        source: &'static str,
        confidence: &'static str,
    ) -> Self {
        Self {
            package_name,
            source,
            confidence,
        }
    }

    fn unknown() -> Self {
        Self::new(String::new(), "unknown", "none")
    }
}

// 结合直接信号、shared_uid 线索与近期调用方回退推断调用方包名
fn resolve_caller_info_locked(
    state: &AuditState,
    caller_package: &str,
    normalized_path: &str,
    is_mkdir_like: bool,
) -> SourceIdentity {
    let resolved_caller = extract_caller_package(caller_package);
    let is_intermediate_caller = is_intermediate_caller_package(&resolved_caller);

    if !resolved_caller.is_empty() && !is_intermediate_caller {
        return SourceIdentity::new(resolved_caller, "caller", "high");
    }

    // 线程名识别：shared_uid 进程内区分 MTP
    if !state.shared_uid_packages.is_empty()
        && policy::is_shared_uid_process(state.uid)
        && let Some(resolution) = infer_component_from_thread_name()
    {
        return resolution;
    }

    // Java 栈帧回溯：shared_uid 进程内按调用栈区分具体包名
    if !state.shared_uid_packages.is_empty()
        && policy::is_shared_uid_process(state.uid)
        && let Some(resolution) = infer_package_from_java_stack(&state.shared_uid_packages)
    {
        return resolution;
    }

    if resolved_caller.is_empty()
        && should_use_recent_media_provider_caller(state, normalized_path, is_mkdir_like)
    {
        let caller_age_ms = monotonic_ms() - state.caller_package_updated_ms;
        if (0..=RECENT_CALLER_PACKAGE_WINDOW_MS).contains(&caller_age_ms) {
            return SourceIdentity::new(state.caller_package.clone(), "recent_caller", "medium");
        }
    }

    if !resolved_caller.is_empty() {
        if is_intermediate_caller {
            if !state.shared_uid_packages.is_empty() {
                return SourceIdentity::new(
                    state.shared_uid_packages.clone(),
                    "shared_uid",
                    "fallback",
                );
            }
            return SourceIdentity::unknown();
        }
        return SourceIdentity::new(resolved_caller, "caller", "high");
    }

    if !state.shared_uid_packages.is_empty() {
        return SourceIdentity::new(state.shared_uid_packages.clone(), "shared_uid", "fallback");
    }

    SourceIdentity::unknown()
}

fn should_use_recent_media_provider_caller(
    state: &AuditState,
    normalized_path: &str,
    is_mkdir_like: bool,
) -> bool {
    policy::is_media_provider_package(&state.package_name)
        && !state.caller_package.is_empty()
        && state.caller_package_updated_ms >= 0
        && !is_intermediate_caller_package(&state.caller_package)
        && !(is_mkdir_like && is_public_storage_path(normalized_path))
}

fn is_public_storage_path(path: &str) -> bool {
    if path.is_empty() || !paths::extract_android_private_path_owner(path).is_empty() {
        return false;
    }

    let user_id = paths::extract_user_id_from_storage_path(path);
    if user_id < 0 {
        return false;
    }
    let storage_root = paths::storage_user_root_for_user(user_id);
    if path == storage_root {
        return true;
    }
    paths::is_child(path, &storage_root)
}

fn resolve_caller_info_for_path_locked(
    state: &AuditState,
    caller_package: &str,
    normalized_path: &str,
    is_mkdir_like: bool,
    can_use_recent_private_owner_hint: bool,
) -> SourceIdentity {
    let private_owner = paths::extract_android_private_path_owner(normalized_path);
    if !private_owner.is_empty() && !is_intermediate_caller_package(&private_owner) {
        let user_id = paths::extract_user_id_from_storage_path(normalized_path);
        source_hint::remember_private_path_owner_hint(normalized_path, &private_owner, user_id);
        return SourceIdentity::new(private_owner, "path_owner", "high");
    }

    let identity =
        resolve_caller_info_locked(state, caller_package, normalized_path, is_mkdir_like);

    if should_prefer_direct_identity(&identity) {
        return identity;
    }

    let user_id = paths::extract_user_id_from_storage_path(normalized_path);
    if let Some(path_identity) =
        source_hint::infer_recent_path_caller_identity(normalized_path, user_id)
    {
        return SourceIdentity::new(
            path_identity.package_name,
            path_identity.source,
            path_identity.confidence,
        );
    }

    if let Some(saf_identity) =
        source_hint::infer_recent_saf_caller_identity(normalized_path, user_id)
    {
        return SourceIdentity::new(
            saf_identity.package_name,
            saf_identity.source,
            saf_identity.confidence,
        );
    }

    if let Some(query_identity) = resolve_query_access_identity_locked(state, normalized_path) {
        return query_identity;
    }

    if can_use_recent_private_owner_hint
        && let Some(identity) =
            source_hint::infer_recent_private_owner_identity(normalized_path, user_id)
    {
        return SourceIdentity::new(identity.package_name, identity.source, identity.confidence);
    }

    if can_use_recent_private_owner_hint
        && let Some(identity) =
            source_hint::infer_public_path_token_identity(normalized_path, user_id)
    {
        return SourceIdentity::new(identity.package_name, identity.source, identity.confidence);
    }

    if user_id < 0 {
        return identity;
    }

    let inferred =
        SettingsHub::instance().resolve_monitor_package_by_path_for_user(user_id, normalized_path);
    if inferred.is_empty() {
        return identity;
    }

    SourceIdentity::new(inferred, "path_config", "medium")
}

fn remember_query_access_locked(
    state: &mut AuditState,
    normalized_path: String,
    package_name: String,
    uid: i32,
    source: &'static str,
    confidence: &'static str,
) {
    let now_ms = monotonic_ms();
    if state.query_access_paths.contains_key(&normalized_path) {
        state
            .query_access_order
            .retain(|path| path != &normalized_path);
    }
    state.query_access_order.push_back(normalized_path.clone());
    state.query_access_paths.insert(
        normalized_path,
        QueryAccessRecord {
            package_name,
            uid,
            updated_ms: now_ms,
            source,
            confidence,
        },
    );
    prune_query_access_locked(state, now_ms);
}

fn resolve_query_access_identity_locked(
    state: &AuditState,
    normalized_path: &str,
) -> Option<SourceIdentity> {
    if normalized_path.is_empty() {
        return None;
    }
    let record = state.query_access_paths.get(normalized_path).or_else(|| {
        media_store_pending_display_path(normalized_path)
            .as_deref()
            .and_then(|display_path| state.query_access_paths.get(display_path))
    })?;
    let age_ms = monotonic_ms() - record.updated_ms;
    if !(0..=QUERY_ACCESS_WINDOW_MS).contains(&age_ms) {
        return None;
    }
    if record.uid < 10_000
        || record.package_name.is_empty()
        || is_intermediate_caller_package(&record.package_name)
    {
        return None;
    }
    Some(SourceIdentity::new(
        record.package_name.clone(),
        record.source,
        record.confidence,
    ))
}

fn resolve_recent_path_caller_package(caller_uid: i32, explicit_package: &str) -> Option<String> {
    if caller_uid < 10_000 {
        return None;
    }
    let explicit = extract_caller_package(explicit_package);
    if !explicit.is_empty() && !is_intermediate_caller_package(&explicit) {
        return Some(explicit);
    }
    let mut packages = policy::get_packages_for_uid(caller_uid);
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(caller_uid);
    }
    packages.sort();
    packages.dedup();
    packages
        .into_iter()
        .find(|pkg| !pkg.is_empty() && !is_intermediate_caller_package(pkg))
}

fn normalize_provider_open_op_filter(value: &str) -> &'static str {
    match value {
        "provider_open:create" => "provider_open:create",
        "provider_open:read" => "provider_open:read",
        "provider_open:write" => "provider_open:write",
        "saf_provider:create" => "provider_open:create",
        "saf_provider:read" => "provider_open:read",
        "saf_provider:write" => "provider_open:write",
        _ => "provider_open",
    }
}

fn media_store_pending_display_path(normalized_path: &str) -> Option<String> {
    let slash = normalized_path.rfind('/')?;
    let file_name = &normalized_path[slash + 1..];
    let pending_tail = file_name.strip_prefix(".pending-")?;
    let display_name_start = pending_tail.find('-')? + 1;
    if display_name_start >= pending_tail.len() {
        return None;
    }

    Some(format!(
        "{}/{}",
        normalized_path[..slash].trim_end_matches('/'),
        &pending_tail[display_name_start..]
    ))
}

fn prune_query_access_locked(state: &mut AuditState, now_ms: i64) {
    while let Some(oldest) = state.query_access_order.front().cloned() {
        let should_remove = match state.query_access_paths.get(&oldest) {
            Some(record) => {
                now_ms - record.updated_ms > QUERY_ACCESS_WINDOW_MS
                    || state.query_access_order.len() > MAX_QUERY_ACCESS_PATHS
            }
            None => true,
        };
        if !should_remove {
            break;
        }
        state.query_access_order.pop_front();
        state.query_access_paths.remove(&oldest);
    }
}

fn should_prefer_direct_identity(identity: &SourceIdentity) -> bool {
    !identity.package_name.is_empty()
        && identity.source != "unknown"
        && identity.source != "shared_uid"
        && identity.source != "thread_name"
        && identity.source != "java_stack"
        && identity.source != "recent_caller"
        && !is_intermediate_caller_package(&identity.package_name)
}

fn extract_caller_package(caller_package: &str) -> String {
    if caller_package.is_empty() || caller_package == "-" {
        return String::new();
    }

    if let Some(value) = extract_kv_value(caller_package, "caller=") {
        let normalized = normalize_package_text(&value);
        if !normalized.is_empty() {
            return normalized;
        }
    }

    if caller_package.contains('=') || caller_package.contains('|') {
        return String::new();
    }

    normalize_package_text(caller_package)
}

fn is_intermediate_caller_package(package_name: &str) -> bool {
    policy::is_media_intermediate_package(package_name)
}

// 从 key=value|... 格式中提取指定 key 的值
fn extract_kv_value(source: &str, key: &str) -> Option<String> {
    let begin = source.find(key)? + key.len();
    if begin >= source.len() {
        return None;
    }
    let end = source[begin..]
        .find('|')
        .map(|idx| begin + idx)
        .unwrap_or(source.len());
    if end <= begin {
        return None;
    }
    Some(source[begin..end].to_string())
}

fn normalize_package_text(source: &str) -> String {
    let value = source.trim();
    if value.is_empty() {
        return String::new();
    }

    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
    {
        return String::new();
    }

    if !value.contains('.') {
        return String::new();
    }

    value.to_string()
}

// 时间窗口内抑制相同 event_key 重复事件
fn should_skip_duplicate_event_locked(state: &mut AuditState, event_key: &str) -> bool {
    if event_key.is_empty() {
        return false;
    }

    let now_ms = paths::monotonic_ms();
    if let Some(entry) = state.recent_event_ms.get_mut(event_key) {
        if now_ms - *entry < DUPLICATE_EVENT_WINDOW_MS {
            *entry = now_ms;
            return true;
        }
        *entry = now_ms;
        return false;
    }

    state.recent_event_ms.insert(event_key.to_string(), now_ms);
    state.recent_event_order.push_back(event_key.to_string());
    while state.recent_event_order.len() > MAX_RECENT_EVENTS {
        if let Some(oldest) = state.recent_event_order.pop_front() {
            state.recent_event_ms.remove(&oldest);
        }
    }
    false
}

fn build_event_key_locked(
    state: &AuditState,
    source_identity: &SourceIdentity,
    kind: OpKind,
    operation_name: &str,
    normalized_path: &str,
    result: i32,
    error_no: i32,
) -> String {
    let operation_key = duplicate_event_operation_key(kind, operation_name);
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        state.package_name,
        source_identity.package_name,
        op_type_to_text(kind),
        operation_key,
        normalized_path,
        result,
        error_no
    )
}

fn op_type_to_text(kind: OpKind) -> &'static str {
    match kind {
        OpKind::Open => "OPEN",
        OpKind::Mkdir => "MKDIR",
        OpKind::Mknod => "MKNOD",
        OpKind::Rename => "RENAME",
        OpKind::Unlink => "UNLINK",
        OpKind::Rmdir => "RMDIR",
        OpKind::Link => "LINK",
        OpKind::Symlink => "SYMLINK",
        OpKind::Truncate => "TRUNCATE",
        OpKind::Chmod => "CHMOD",
        OpKind::Utimens => "UTIMENS",
    }
}

fn operation_name_from_extra(kind: OpKind, extra: &str) -> String {
    if let Some(op) = extract_kv_value(extra, "op_filter=") {
        let op = op.trim();
        if !op.is_empty() {
            return op.to_lowercase();
        }
    }
    if let Some(op) = extract_kv_value(extra, "op=") {
        let op = op.trim();
        if !op.is_empty() {
            return op.to_lowercase();
        }
    }
    op_type_to_text(kind).to_lowercase()
}

fn duplicate_event_operation_key(kind: OpKind, operation_name: &str) -> String {
    let op = operation_name.to_lowercase();
    if matches!(kind, OpKind::Open) && is_open_write_or_create_operation(&op) {
        "open:write".to_string()
    } else {
        op
    }
}

fn is_open_write_or_create_operation(op: &str) -> bool {
    (op.starts_with("open") || op.starts_with("provider_open"))
        && (op.contains("create") || op.contains("write"))
}

fn is_mkdir_like_extra(extra: &str) -> bool {
    extra.contains("op=mkdir") || extra.contains("op=mkdirat")
}

fn should_use_recent_private_owner_hint(kind: OpKind, normalized_path: &str, op: &str) -> bool {
    if normalized_path.is_empty() || !(op.contains("create") || op.contains("write")) {
        return false;
    }
    matches!(
        kind,
        OpKind::Open | OpKind::Mkdir | OpKind::Mknod | OpKind::Rename
    )
}
