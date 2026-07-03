use crate::platform::{self, module_paths, paths};
use crate::redirect::policy;
use once_cell::sync::Lazy;
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::sync::Mutex;

const HINT_VERSION: &str = "3";
const RECENT_PRIVATE_OWNER_HINT_WINDOW_MS: i64 = 30_000;
const RECENT_PRIVATE_CALLER_HINT_WINDOW_MS: i64 = 300_000;
const RECENT_PRIVATE_TOKEN_HINT_WINDOW_MS: i64 = 300_000;
const RECENT_PATH_CALLER_HINT_VERSION: &str = "2";
const RECENT_PATH_CALLER_HINT_WINDOW_MS: i64 = 30_000;
const MAX_RECENT_PRIVATE_OWNER_HINTS: usize = 8;
const MAX_RECENT_PATH_CALLER_HINTS: usize = 16;
const ANDROID_APP_UID_START: i32 = 10_000;
const MAX_PROC_PACKAGE_CANDIDATES: usize = 512;

#[derive(Clone, Debug)]
struct PrivateOwnerHint {
    user_id: i32,
    updated_ms: i64,
    owner_package: String,
    package_name: String,
    caller_uid: i32,
    tokens: Vec<String>,
    source: &'static str,
    confidence: &'static str,
}

#[derive(Clone, Debug)]
struct PathCallerHint {
    user_id: i32,
    updated_ms: i64,
    package_name: String,
    path: String,
    source: &'static str,
    confidence: &'static str,
    op_filter: &'static str,
}

static RECENT_PRIVATE_OWNER_HINT: Lazy<Mutex<VecDeque<PrivateOwnerHint>>> =
    Lazy::new(|| Mutex::new(VecDeque::new()));
static RECENT_PATH_CALLER_HINT: Lazy<Mutex<VecDeque<PathCallerHint>>> =
    Lazy::new(|| Mutex::new(VecDeque::new()));

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RecentPrivateOwnerIdentity {
    pub(crate) package_name: String,
    pub(crate) source: &'static str,
    pub(crate) confidence: &'static str,
}

pub(crate) fn remember_private_path_owner_hint(
    normalized_path: &str,
    package_name: &str,
    user_id: i32,
) {
    remember_private_path_hint(
        normalized_path,
        package_name,
        package_name,
        user_id,
        "recent_private_owner",
        "medium",
        -1,
    );
}

pub(crate) fn remember_private_path_caller_hint(
    normalized_path: &str,
    owner_package: &str,
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
) {
    if caller_uid < ANDROID_APP_UID_START || platform::user_id_from_uid(caller_uid) != user_id {
        return;
    }

    remember_private_path_hint(
        normalized_path,
        owner_package,
        caller_package,
        user_id,
        "recent_private_caller",
        "medium",
        caller_uid,
    );
}

pub(crate) fn remember_private_path_caller_hint_in_memory(
    normalized_path: &str,
    owner_package: &str,
    caller_package: &str,
    caller_uid: i32,
    user_id: i32,
) {
    if caller_uid < ANDROID_APP_UID_START || platform::user_id_from_uid(caller_uid) != user_id {
        return;
    }

    remember_private_path_hint_inner(
        normalized_path,
        owner_package,
        caller_package,
        user_id,
        "recent_private_caller",
        "medium",
        caller_uid,
        false,
    );
}

pub(crate) fn remember_private_path_caller_uid_hint_in_memory(
    normalized_path: &str,
    owner_package: &str,
    caller_uid: i32,
    user_id: i32,
) {
    if caller_uid < ANDROID_APP_UID_START || platform::user_id_from_uid(caller_uid) != user_id {
        return;
    }

    remember_private_path_hint_inner(
        normalized_path,
        owner_package,
        "",
        user_id,
        "recent_private_caller",
        "medium",
        caller_uid,
        false,
    );
}

fn remember_private_path_hint(
    normalized_path: &str,
    affinity_owner_package: &str,
    package_name: &str,
    user_id: i32,
    source: &'static str,
    confidence: &'static str,
    caller_uid: i32,
) {
    remember_private_path_hint_inner(
        normalized_path,
        affinity_owner_package,
        package_name,
        user_id,
        source,
        confidence,
        caller_uid,
        true,
    );
}

fn remember_private_path_hint_inner(
    normalized_path: &str,
    affinity_owner_package: &str,
    package_name: &str,
    user_id: i32,
    source: &'static str,
    confidence: &'static str,
    caller_uid: i32,
    persist: bool,
) {
    let has_package_name = is_valid_package_name(package_name);
    let has_caller_uid = source == "recent_private_caller"
        && caller_uid >= ANDROID_APP_UID_START
        && platform::user_id_from_uid(caller_uid) == user_id;
    if user_id < 0
        || normalized_path.is_empty()
        || !is_valid_package_name(affinity_owner_package)
        || (!has_package_name && !has_caller_uid)
    {
        return;
    }

    let affinity_text = private_owner_affinity_text(normalized_path, affinity_owner_package);
    let tokens = extract_affinity_tokens(&affinity_text);
    if tokens.is_empty() {
        return;
    }

    let hint = PrivateOwnerHint {
        user_id,
        updated_ms: paths::monotonic_ms(),
        owner_package: affinity_owner_package.to_string(),
        package_name: package_name.to_string(),
        caller_uid,
        tokens,
        source,
        confidence,
    };

    let hints_to_write = if let Ok(mut hints) = RECENT_PRIVATE_OWNER_HINT.lock() {
        remember_hint_locked(&mut hints, hint.clone());
        persist.then(|| hints.iter().cloned().collect::<Vec<_>>())
    } else if persist {
        Some(vec![hint.clone()])
    } else {
        None
    };
    if let Some(hints_to_write) = hints_to_write {
        write_hint_file(&hints_to_write);
    }
}

pub(crate) fn infer_recent_private_owner_identity(
    normalized_path: &str,
    user_id: i32,
) -> Option<RecentPrivateOwnerIdentity> {
    if user_id < 0 || !is_public_download_path(normalized_path) {
        return None;
    }

    let path_tokens = extract_affinity_tokens(normalized_path);
    if path_tokens.is_empty() {
        return None;
    }

    let mut hints = RECENT_PRIVATE_OWNER_HINT
        .lock()
        .ok()
        .map(|slot| slot.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    hints.extend(read_hint_file());
    infer_from_hints(hints, user_id, &path_tokens)
}

pub(crate) fn remember_public_path_caller_hint(
    normalized_path: &str,
    package_name: &str,
    caller_uid: i32,
    source: &'static str,
    confidence: &'static str,
) {
    if caller_uid < ANDROID_APP_UID_START
        || !is_valid_package_name(package_name)
        || normalize_path_hint_source(source).is_none()
        || normalize_hint_confidence(confidence).is_none()
    {
        return;
    }
    let user_id = paths::extract_user_id_from_storage_path(normalized_path);
    if user_id < 0 || platform::user_id_from_uid(caller_uid) != user_id {
        return;
    }
    if !is_public_storage_hint_path(normalized_path, user_id) {
        return;
    }

    let hint = PathCallerHint {
        user_id,
        updated_ms: paths::monotonic_ms(),
        package_name: package_name.to_string(),
        path: normalized_path.to_string(),
        source,
        confidence,
        op_filter: "provider_open",
    };

    let hints_to_write = if let Ok(mut hints) = RECENT_PATH_CALLER_HINT.lock() {
        remember_path_hint_locked(&mut hints, hint.clone());
        hints.iter().cloned().collect::<Vec<_>>()
    } else {
        vec![hint.clone()]
    };
    write_path_hint_file(&hints_to_write);
}

pub(crate) fn remember_saf_path_caller_hint(
    normalized_path: &str,
    package_name: &str,
    caller_uid: i32,
    source: &'static str,
    confidence: &'static str,
    op_filter: &str,
) {
    if caller_uid < ANDROID_APP_UID_START
        || !is_valid_package_name(package_name)
        || normalize_path_hint_source(source) != Some("saf_provider")
        || normalize_hint_confidence(confidence).is_none()
    {
        return;
    }
    let user_id = paths::extract_user_id_from_storage_path(normalized_path);
    if user_id < 0 || platform::user_id_from_uid(caller_uid) != user_id {
        return;
    }
    if !is_public_storage_hint_path(normalized_path, user_id) {
        return;
    }
    let Some(op_filter) = normalize_path_hint_op_filter(op_filter) else {
        return;
    };

    let hint = PathCallerHint {
        user_id,
        updated_ms: paths::monotonic_ms(),
        package_name: package_name.to_string(),
        path: normalized_path.to_string(),
        source,
        confidence,
        op_filter,
    };

    let hints_to_write = if let Ok(mut hints) = RECENT_PATH_CALLER_HINT.lock() {
        remember_path_hint_locked(&mut hints, hint.clone());
        hints.iter().cloned().collect::<Vec<_>>()
    } else {
        vec![hint.clone()]
    };
    write_path_hint_file(&hints_to_write);
}

pub(crate) fn infer_recent_path_caller_identity(
    normalized_path: &str,
    user_id: i32,
) -> Option<RecentPrivateOwnerIdentity> {
    if user_id < 0 || !is_public_storage_hint_path(normalized_path, user_id) {
        return None;
    }

    let mut hints = RECENT_PATH_CALLER_HINT
        .lock()
        .ok()
        .map(|slot| slot.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    hints.extend(read_path_hint_file());
    infer_from_path_hints(hints, user_id, normalized_path)
}

pub(crate) fn infer_recent_saf_caller_identity(
    normalized_path: &str,
    user_id: i32,
) -> Option<RecentPrivateOwnerIdentity> {
    if user_id < 0 || !is_public_storage_hint_path(normalized_path, user_id) {
        return None;
    }

    let mut hints = RECENT_PATH_CALLER_HINT
        .lock()
        .ok()
        .map(|slot| slot.iter().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    hints.extend(read_path_hint_file());
    infer_from_path_hints(
        hints
            .into_iter()
            .filter(|hint| hint.source == "saf_provider")
            .collect(),
        user_id,
        normalized_path,
    )
}

pub(crate) fn infer_public_path_token_identity(
    normalized_path: &str,
    user_id: i32,
) -> Option<RecentPrivateOwnerIdentity> {
    if user_id < 0 || !is_public_download_path(normalized_path) {
        return None;
    }

    let path_tokens = extract_affinity_tokens(normalized_path);
    if path_tokens.is_empty() {
        return None;
    }

    let mut packages = policy::get_all_package_names();
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_all_package_names();
    }
    best_public_path_token_package(packages, &path_tokens, user_id)
        .or_else(|| infer_running_package_by_public_path_tokens(&path_tokens, user_id))
        .map(|package_name| RecentPrivateOwnerIdentity {
            package_name,
            source: "public_path_token",
            confidence: "medium",
        })
}

fn remember_hint_locked(hints: &mut VecDeque<PrivateOwnerHint>, hint: PrivateOwnerHint) {
    hints.retain(|existing| {
        !(existing.user_id == hint.user_id
            && existing.owner_package == hint.owner_package
            && existing.package_name == hint.package_name
            && existing.caller_uid == hint.caller_uid
            && existing.source == hint.source
            && existing.tokens == hint.tokens)
    });
    hints.push_back(hint);
    let now_ms = paths::monotonic_ms();
    hints.retain(|existing| {
        (0..=private_hint_window_ms(existing)).contains(&now_ms.saturating_sub(existing.updated_ms))
    });
    while hints.len() > MAX_RECENT_PRIVATE_OWNER_HINTS {
        hints.pop_front();
    }
}

fn remember_path_hint_locked(hints: &mut VecDeque<PathCallerHint>, hint: PathCallerHint) {
    hints.retain(|existing| {
        !(existing.user_id == hint.user_id
            && existing.package_name == hint.package_name
            && existing.source == hint.source
            && existing.path == hint.path
            && existing.op_filter == hint.op_filter)
    });
    hints.push_back(hint);
    let now_ms = paths::monotonic_ms();
    hints.retain(|existing| {
        (0..=RECENT_PATH_CALLER_HINT_WINDOW_MS)
            .contains(&now_ms.saturating_sub(existing.updated_ms))
    });
    while hints.len() > MAX_RECENT_PATH_CALLER_HINTS {
        hints.pop_front();
    }
}

fn infer_from_hints(
    hints: Vec<PrivateOwnerHint>,
    user_id: i32,
    path_tokens: &[String],
) -> Option<RecentPrivateOwnerIdentity> {
    hints
        .into_iter()
        .filter(|hint| hint_matches(hint, user_id, path_tokens))
        .max_by(|left, right| {
            hint_rank(left)
                .cmp(&hint_rank(right))
                .then_with(|| left.updated_ms.cmp(&right.updated_ms))
        })
        .and_then(|hint| {
            let package_name = if hint.source == "recent_private_owner" {
                infer_package_by_private_path_tokens(&hint, path_tokens)
                    .or_else(|| resolve_hint_package(&hint))?
            } else {
                resolve_hint_package(&hint)?
            };
            let (source, confidence) =
                if hint.source == "recent_private_owner" && package_name != hint.package_name {
                    ("recent_private_token", "medium")
                } else {
                    (hint.source, hint.confidence)
                };
            Some(RecentPrivateOwnerIdentity {
                package_name,
                source,
                confidence,
            })
        })
}

fn infer_from_path_hints(
    hints: Vec<PathCallerHint>,
    user_id: i32,
    normalized_path: &str,
) -> Option<RecentPrivateOwnerIdentity> {
    hints
        .into_iter()
        .filter(|hint| path_hint_matches(hint, user_id, normalized_path))
        .max_by(|left, right| {
            path_hint_rank(left)
                .cmp(&path_hint_rank(right))
                .then_with(|| left.updated_ms.cmp(&right.updated_ms))
        })
        .map(|hint| RecentPrivateOwnerIdentity {
            package_name: hint.package_name,
            source: hint.source,
            confidence: hint.confidence,
        })
}

fn hint_matches(hint: &PrivateOwnerHint, user_id: i32, path_tokens: &[String]) -> bool {
    if hint.user_id != user_id {
        return false;
    }
    if !has_token_overlap(&hint.tokens, path_tokens) {
        return false;
    }
    let token_package = infer_package_by_private_path_tokens(hint, path_tokens);
    let age_ms = paths::monotonic_ms().saturating_sub(hint.updated_ms);
    let max_age_ms = if token_package.is_some() {
        RECENT_PRIVATE_TOKEN_HINT_WINDOW_MS
    } else {
        private_hint_window_ms(hint)
    };
    if !(0..=max_age_ms).contains(&age_ms) {
        return false;
    }
    resolve_hint_package(hint).or(token_package).is_some()
}

fn private_hint_window_ms(hint: &PrivateOwnerHint) -> i64 {
    if hint.source == "recent_private_caller" {
        RECENT_PRIVATE_CALLER_HINT_WINDOW_MS
    } else {
        RECENT_PRIVATE_OWNER_HINT_WINDOW_MS
    }
}

fn resolve_hint_package(hint: &PrivateOwnerHint) -> Option<String> {
    if is_valid_package_name(&hint.package_name) {
        return Some(hint.package_name.clone());
    }
    if hint.source != "recent_private_caller" || hint.caller_uid < ANDROID_APP_UID_START {
        return None;
    }

    let mut packages = policy::get_packages_for_uid(hint.caller_uid);
    if packages.is_empty() {
        policy::refresh_shared_uid_cache();
        packages = policy::get_packages_for_uid(hint.caller_uid);
    }
    packages.sort();
    packages.dedup();
    packages.into_iter().find(|package| {
        is_valid_package_name(package)
            && package != &hint.owner_package
            && !policy::is_system_writer_package(package)
            && !policy::is_media_intermediate_package(package)
    })
}

fn infer_package_by_private_path_tokens(
    hint: &PrivateOwnerHint,
    path_tokens: &[String],
) -> Option<String> {
    if hint.source != "recent_private_owner" {
        return None;
    }
    policy::refresh_shared_uid_cache();
    let packages = policy::get_all_package_names();
    best_private_path_token_package(packages, hint, path_tokens)
        .or_else(|| infer_running_package_by_private_path_tokens(hint, path_tokens))
}

fn infer_running_package_by_private_path_tokens(
    hint: &PrivateOwnerHint,
    path_tokens: &[String],
) -> Option<String> {
    let mut packages = Vec::new();
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        if packages.len() >= MAX_PROC_PACKAGE_CANDIDATES {
            break;
        }
        let file_name = entry.file_name();
        let Some(pid_text) = file_name.to_str() else {
            continue;
        };
        if !pid_text.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let cmdline_path = entry.path().join("cmdline");
        let Some(package) = read_proc_cmdline_package(&cmdline_path) else {
            continue;
        };
        packages.push(package);
    }
    best_private_path_token_package(packages, hint, path_tokens)
}

fn read_proc_cmdline_package(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read(path).ok()?;
    let first = content.split(|byte| *byte == 0).next()?;
    let text = std::str::from_utf8(first).ok()?;
    normalize_process_package_text(text)
}

fn normalize_process_package_text(text: &str) -> Option<String> {
    let mut value = text.trim();
    if value.is_empty() || value.starts_with('/') {
        return None;
    }
    if let Some((package, _suffix)) = value.split_once(':') {
        value = package;
    }
    if is_valid_package_name(value) {
        Some(value.to_string())
    } else {
        None
    }
}

fn best_private_path_token_package(
    packages: Vec<String>,
    hint: &PrivateOwnerHint,
    path_tokens: &[String],
) -> Option<String> {
    packages
        .into_iter()
        .filter(|package| is_valid_private_token_package(package, hint))
        .filter_map(|package| {
            let score = private_path_token_package_score(&package, &hint.tokens, path_tokens);
            (score > 0).then_some((score, package))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)))
        .map(|(_, package)| package)
}

fn is_valid_private_token_package(package: &str, hint: &PrivateOwnerHint) -> bool {
    is_valid_package_name(package)
        && package != hint.owner_package
        && package != hint.package_name
        && !policy::is_system_writer_package(package)
        && !policy::is_media_intermediate_package(package)
}

fn private_path_token_package_score(
    package_name: &str,
    hint_tokens: &[String],
    path_tokens: &[String],
) -> i32 {
    hint_tokens
        .iter()
        .filter(|token| {
            path_tokens.contains(token)
                && token.len() >= 5
                && package_name
                    .to_ascii_lowercase()
                    .split(['.', '_', '-'])
                    .any(|part| part == token.as_str())
        })
        .map(|token| token.len() as i32)
        .sum()
}

fn infer_running_package_by_public_path_tokens(
    path_tokens: &[String],
    user_id: i32,
) -> Option<String> {
    let mut packages = Vec::new();
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        if packages.len() >= MAX_PROC_PACKAGE_CANDIDATES {
            break;
        }
        let file_name = entry.file_name();
        let Some(pid_text) = file_name.to_str() else {
            continue;
        };
        if !pid_text.chars().all(|ch| ch.is_ascii_digit()) {
            continue;
        }
        let cmdline_path = entry.path().join("cmdline");
        let Some(package) = read_proc_cmdline_package(&cmdline_path) else {
            continue;
        };
        if running_package_matches_user(entry.path().join("status").as_path(), user_id) {
            packages.push(package);
        }
    }
    best_public_path_token_package(packages, path_tokens, user_id)
}

fn running_package_matches_user(path: &std::path::Path, user_id: i32) -> bool {
    let Some(uid) = read_proc_status_uid(path) else {
        return false;
    };
    uid >= ANDROID_APP_UID_START && platform::user_id_from_uid(uid) == user_id
}

fn read_proc_status_uid(path: &std::path::Path) -> Option<i32> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let value = line.strip_prefix("Uid:")?;
        value
            .split_whitespace()
            .next()
            .and_then(|uid| uid.parse::<i32>().ok())
    })
}

fn best_public_path_token_package(
    packages: Vec<String>,
    path_tokens: &[String],
    user_id: i32,
) -> Option<String> {
    packages
        .into_iter()
        .filter(|package| is_valid_public_path_token_package(package, user_id))
        .filter_map(|package| {
            let score = public_path_token_package_score(&package, path_tokens);
            (score > 0).then_some((score, package))
        })
        .max_by(|left, right| left.0.cmp(&right.0).then_with(|| right.1.cmp(&left.1)))
        .map(|(_, package)| package)
}

fn is_valid_public_path_token_package(package: &str, user_id: i32) -> bool {
    if !is_valid_package_name(package)
        || policy::is_system_writer_package(package)
        || policy::is_media_intermediate_package(package)
    {
        return false;
    }
    let uid = policy::get_uid_for_package(package);
    uid < 0 || platform::user_id_from_uid(uid) == user_id
}

fn public_path_token_package_score(package_name: &str, path_tokens: &[String]) -> i32 {
    path_tokens
        .iter()
        .filter(|token| {
            token.len() >= 5
                && package_name
                    .to_ascii_lowercase()
                    .split(['.', '_', '-'])
                    .any(|part| part == token.as_str())
        })
        .map(|token| token.len() as i32)
        .sum()
}

fn path_hint_matches(hint: &PathCallerHint, user_id: i32, normalized_path: &str) -> bool {
    if hint.user_id != user_id || !is_valid_package_name(&hint.package_name) {
        return false;
    }
    let age_ms = paths::monotonic_ms().saturating_sub(hint.updated_ms);
    if !(0..=RECENT_PATH_CALLER_HINT_WINDOW_MS).contains(&age_ms) {
        return false;
    }
    if hint.path == normalized_path {
        return true;
    }

    if media_store_pending_display_path(normalized_path)
        .as_deref()
        .is_some_and(|display_path| hint.path == display_path)
    {
        return true;
    }

    if hint.source != "saf_provider" {
        return false;
    }

    saf_hint_path_matches(&hint.path, normalized_path)
}

fn saf_hint_path_matches(hint_path: &str, normalized_path: &str) -> bool {
    if hint_path.is_empty() || normalized_path.is_empty() {
        return false;
    }
    if paths::is_child(normalized_path, hint_path) {
        return true;
    }
    if let Some(display_path) = media_store_pending_display_path(normalized_path) {
        if paths::is_child(&display_path, hint_path) {
            return true;
        }
    }
    let Some(hint_name) = path_file_name(hint_path) else {
        return false;
    };
    let Some(path_name) = path_file_name(normalized_path) else {
        return false;
    };
    !hint_name.is_empty() && hint_name.eq_ignore_ascii_case(path_name)
}

fn path_file_name(path: &str) -> Option<&str> {
    path.trim_end_matches('/').rsplit('/').next()
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

fn hint_rank(hint: &PrivateOwnerHint) -> i32 {
    let source_rank = match hint.source {
        "recent_private_caller" => 300,
        "recent_private_token" => 250,
        "recent_private_owner" => 200,
        _ => 0,
    };
    let confidence_rank = match hint.confidence {
        "high" => 30,
        "medium" => 20,
        "fallback" => 10,
        _ => 0,
    };
    source_rank + confidence_rank
}

fn path_hint_rank(hint: &PathCallerHint) -> i32 {
    let source_rank = match hint.source {
        "saf_provider" => 400,
        "provider_open" => 300,
        "query_access" => 200,
        _ => 0,
    };
    let confidence_rank = match hint.confidence {
        "high" => 30,
        "medium" => 20,
        "fallback" => 10,
        _ => 0,
    };
    source_rank + confidence_rank
}

fn private_owner_affinity_text(normalized_path: &str, package_name: &str) -> String {
    let marker = format!("/{package_name}");
    normalized_path
        .find(&marker)
        .map(|index| normalized_path[index + marker.len()..].to_string())
        .filter(|suffix| !suffix.is_empty())
        .unwrap_or_else(|| normalized_path.to_string())
}

fn is_public_download_path(path: &str) -> bool {
    let user_id = paths::extract_user_id_from_storage_path(path);
    if user_id < 0 || !paths::extract_android_private_path_owner(path).is_empty() {
        return false;
    }
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &storage_root) else {
        return false;
    };
    paths::matches("Download", relative, true)
}

fn is_public_storage_hint_path(path: &str, user_id: i32) -> bool {
    if path.is_empty()
        || path.contains('|')
        || path.contains('\n')
        || path.contains('\r')
        || !paths::extract_android_private_path_owner(path).is_empty()
    {
        return false;
    }
    let storage_root = paths::storage_user_root_for_user(user_id);
    paths::relative_child_path(path, &storage_root).is_some()
}

fn has_token_overlap(left: &[String], right: &[String]) -> bool {
    let right: HashSet<&str> = right.iter().map(String::as_str).collect();
    left.iter().any(|token| right.contains(token.as_str()))
}

fn extract_affinity_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else {
            flush_token(&mut current, &mut tokens);
        }
    }
    flush_token(&mut current, &mut tokens);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn flush_token(current: &mut String, tokens: &mut Vec<String>) {
    if current.len() >= 4
        && current.len() <= 48
        && !current.chars().all(|ch| ch.is_ascii_digit())
        && !is_generic_token(current)
    {
        tokens.push(current.clone());
    }
    current.clear();
}

fn is_generic_token(token: &str) -> bool {
    matches!(
        token,
        "android"
            | "backup"
            | "cache"
            | "config"
            | "data"
            | "download"
            | "emulated"
            | "export"
            | "file"
            | "files"
            | "json"
            | "media"
            | "module"
            | "obb"
            | "setting"
            | "settings"
            | "storage"
            | "temp"
            | "tmp"
    )
}

fn write_hint_file(hints: &[PrivateOwnerHint]) {
    let path = std::path::Path::new(module_paths::RECENT_SOURCE_HINT_FILE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::File::create(path) else {
        return;
    };
    for hint in hints {
        let _ = writeln!(
            file,
            "{}|{}|{}|{}|{}|{}|{}|{}",
            HINT_VERSION,
            hint.user_id,
            hint.updated_ms,
            hint.owner_package,
            hint.package_name,
            hint.tokens.join(","),
            hint.source,
            hint.confidence
        );
    }
    chmod_hint_file(path);
}

fn write_path_hint_file(hints: &[PathCallerHint]) {
    let path = std::path::Path::new(module_paths::RECENT_PATH_CALLER_HINT_FILE);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::File::create(path) else {
        return;
    };
    for hint in hints {
        let _ = writeln!(
            file,
            "{}|{}|{}|{}|{}|{}|{}|{}",
            RECENT_PATH_CALLER_HINT_VERSION,
            hint.user_id,
            hint.updated_ms,
            hint.package_name,
            hint.source,
            hint.confidence,
            hint.op_filter,
            hint.path
        );
    }
    chmod_hint_file(path);
}

fn read_hint_file() -> Vec<PrivateOwnerHint> {
    std::fs::read_to_string(module_paths::RECENT_SOURCE_HINT_FILE)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter_map(|line| parse_hint_line(line.trim()))
                .collect()
        })
        .unwrap_or_default()
}

fn read_path_hint_file() -> Vec<PathCallerHint> {
    std::fs::read_to_string(module_paths::RECENT_PATH_CALLER_HINT_FILE)
        .ok()
        .map(|content| {
            content
                .lines()
                .filter_map(|line| parse_path_hint_line(line.trim()))
                .collect()
        })
        .unwrap_or_default()
}

fn parse_hint_line(line: &str) -> Option<PrivateOwnerHint> {
    let parts: Vec<&str> = line.split('|').collect();
    let (
        user_id_part,
        updated_ms_part,
        owner_package_part,
        package_name_part,
        tokens_part,
        source,
        confidence,
    ) = match parts.as_slice() {
        ["1", user_id, updated_ms, package_name, tokens] => (
            *user_id,
            *updated_ms,
            *package_name,
            *package_name,
            *tokens,
            "recent_private_owner",
            "medium",
        ),
        [
            "2",
            user_id,
            updated_ms,
            package_name,
            tokens,
            source,
            confidence,
        ] => (
            *user_id,
            *updated_ms,
            *package_name,
            *package_name,
            *tokens,
            normalize_hint_source(source)?,
            normalize_hint_confidence(confidence)?,
        ),
        [
            "3",
            user_id,
            updated_ms,
            owner_package,
            package_name,
            tokens,
            source,
            confidence,
        ] => (
            *user_id,
            *updated_ms,
            *owner_package,
            *package_name,
            *tokens,
            normalize_hint_source(source)?,
            normalize_hint_confidence(confidence)?,
        ),
        _ => return None,
    };
    let user_id = user_id_part.parse().ok()?;
    let updated_ms = updated_ms_part.parse().ok()?;
    let owner_package = owner_package_part.to_string();
    let package_name = package_name_part.to_string();
    if !is_valid_package_name(&owner_package) || !is_valid_package_name(&package_name) {
        return None;
    }
    let tokens = tokens_part
        .split(',')
        .filter(|token| !token.is_empty() && token.chars().all(|ch| ch.is_ascii_alphanumeric()))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    Some(PrivateOwnerHint {
        user_id,
        updated_ms,
        owner_package,
        package_name,
        caller_uid: -1,
        tokens,
        source,
        confidence,
    })
}

fn parse_path_hint_line(line: &str) -> Option<PathCallerHint> {
    let parts: Vec<&str> = line.split('|').collect();
    let (
        user_id_part,
        updated_ms_part,
        package_name_part,
        source_part,
        confidence_part,
        op_filter_part,
        path_part,
    ) = match parts.as_slice() {
        [
            "1",
            user_id,
            updated_ms,
            package_name,
            source,
            confidence,
            path,
        ] => (
            *user_id,
            *updated_ms,
            *package_name,
            *source,
            *confidence,
            "provider_open",
            *path,
        ),
        [
            "2",
            user_id,
            updated_ms,
            package_name,
            source,
            confidence,
            op_filter,
            path,
        ] => (
            *user_id,
            *updated_ms,
            *package_name,
            *source,
            *confidence,
            *op_filter,
            *path,
        ),
        _ => return None,
    };
    let user_id = user_id_part.parse().ok()?;
    let updated_ms = updated_ms_part.parse().ok()?;
    let package_name = package_name_part.to_string();
    let source = normalize_path_hint_source(source_part)?;
    let confidence = normalize_hint_confidence(confidence_part)?;
    let op_filter = normalize_path_hint_op_filter(op_filter_part)?;
    let path = path_part.to_string();
    if !is_valid_package_name(&package_name) || !is_public_storage_hint_path(&path, user_id) {
        return None;
    }
    Some(PathCallerHint {
        user_id,
        updated_ms,
        package_name,
        path,
        source,
        confidence,
        op_filter,
    })
}

fn normalize_hint_source(value: &str) -> Option<&'static str> {
    match value {
        "recent_private_owner" => Some("recent_private_owner"),
        "recent_private_caller" => Some("recent_private_caller"),
        "recent_private_token" => Some("recent_private_token"),
        _ => None,
    }
}

fn normalize_path_hint_source(value: &str) -> Option<&'static str> {
    match value {
        "provider_open" => Some("provider_open"),
        "saf_provider" => Some("saf_provider"),
        "query_access" => Some("query_access"),
        _ => None,
    }
}

fn normalize_path_hint_op_filter(value: &str) -> Option<&'static str> {
    match value {
        "provider_open" => Some("provider_open"),
        "provider_open:create" => Some("provider_open:create"),
        "provider_open:read" => Some("provider_open:read"),
        "provider_open:write" => Some("provider_open:write"),
        _ => None,
    }
}

fn normalize_hint_confidence(value: &str) -> Option<&'static str> {
    match value {
        "high" => Some("high"),
        "medium" => Some("medium"),
        "fallback" => Some("fallback"),
        _ => None,
    }
}

fn is_valid_package_name(value: &str) -> bool {
    !value.is_empty()
        && value.contains('.')
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
}

fn chmod_hint_file(path: &std::path::Path) {
    let Some(path_text) = path.to_str() else {
        return;
    };
    let Ok(c_path) = std::ffi::CString::new(path_text) else {
        return;
    };
    unsafe {
        libc::chmod(c_path.as_ptr(), 0o666);
    }
}

#[cfg(test)]
pub(crate) fn clear_recent_private_owner_hint_for_tests() {
    if let Ok(mut slot) = RECENT_PRIVATE_OWNER_HINT.lock() {
        slot.clear();
    }
    if let Ok(mut slot) = RECENT_PATH_CALLER_HINT.lock() {
        slot.clear();
    }
    let _ = std::fs::remove_file(module_paths::RECENT_SOURCE_HINT_FILE);
    let _ = std::fs::remove_file(module_paths::RECENT_PATH_CALLER_HINT_FILE);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static SOURCE_HINT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_source_hint_test() -> MutexGuard<'static, ()> {
        SOURCE_HINT_TEST_LOCK
            .lock()
            .unwrap_or_else(|err| err.into_inner())
    }

    #[test]
    fn recent_private_owner_hint_matches_public_download_token() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_private_path_owner_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            "com.eg.android.AlipayGphone",
            0,
        );

        assert_eq!(
            infer_recent_private_owner_identity(
                "/storage/emulated/0/Download/搞机备份/模块设置备份/XRadiant_backup_20260618_183000.json",
                0,
            )
            .as_ref()
            .map(|identity| identity.package_name.as_str()),
            Some("com.eg.android.AlipayGphone")
        );
    }

    #[test]
    fn recent_private_owner_token_can_infer_real_app_package() {
        use std::collections::HashMap;

        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();
        let previous_uid = policy::replace_test_uid_cache(HashMap::from([
            ("com.eg.android.AlipayGphone".to_string(), 10274),
            ("com.leo.xposed.xradiant".to_string(), 10164),
        ]));

        remember_private_path_owner_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            "com.eg.android.AlipayGphone",
            0,
        );
        let identity = infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_193856.json",
            0,
        )
        .expect("token hint should infer XRadiant package");

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );

        assert_eq!(identity.package_name, "com.leo.xposed.xradiant");
        assert_eq!(identity.source, "recent_private_token");
        assert_eq!(identity.confidence, "medium");
    }

    #[test]
    fn recent_private_owner_token_survives_saf_picker_flow() {
        use std::collections::HashMap;

        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();
        let previous_uid = policy::replace_test_uid_cache(HashMap::from([
            ("com.eg.android.AlipayGphone".to_string(), 10274),
            ("com.leo.xposed.xradiant".to_string(), 10164),
        ]));

        remember_private_path_owner_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            "com.eg.android.AlipayGphone",
            0,
        );
        if let Ok(mut hints) = RECENT_PRIVATE_OWNER_HINT.lock() {
            for hint in hints.iter_mut() {
                hint.updated_ms = paths::monotonic_ms().saturating_sub(120_000);
            }
        }
        let identity = infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_193856.json",
            0,
        )
        .expect("token owner hint should survive SAF picker latency");

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );

        assert_eq!(identity.package_name, "com.leo.xposed.xradiant");
        assert_eq!(identity.source, "recent_private_token");
    }

    #[test]
    fn recent_private_owner_token_falls_back_to_owner_without_app_match() {
        use std::collections::HashMap;

        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();
        let previous_uid = policy::replace_test_uid_cache(HashMap::from([(
            "com.eg.android.AlipayGphone".to_string(),
            10274,
        )]));

        remember_private_path_owner_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            "com.eg.android.AlipayGphone",
            0,
        );
        let identity = infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_193856.json",
            0,
        )
        .expect("owner hint should still be usable");

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );

        assert_eq!(identity.package_name, "com.eg.android.AlipayGphone");
        assert_eq!(identity.source, "recent_private_owner");
    }

    #[test]
    fn private_owner_hint_v3_round_trip_keeps_owner_package() {
        let line = "3|0|1234|com.eg.android.AlipayGphone|com.eg.android.AlipayGphone|xradiant|recent_private_owner|medium";
        let hint = parse_hint_line(line).expect("v3 hint");

        assert_eq!(hint.owner_package, "com.eg.android.AlipayGphone");
        assert_eq!(hint.package_name, "com.eg.android.AlipayGphone");
        assert_eq!(hint.tokens, vec!["xradiant"]);
    }

    #[test]
    fn process_package_text_accepts_app_process_suffix() {
        assert_eq!(
            normalize_process_package_text("com.leo.xposed.xradiant:remote").as_deref(),
            Some("com.leo.xposed.xradiant")
        );
        assert_eq!(
            normalize_process_package_text("/system/bin/app_process").as_deref(),
            None
        );
    }

    #[test]
    fn private_path_token_candidate_excludes_owner_and_intermediates() {
        let hint = PrivateOwnerHint {
            user_id: 0,
            updated_ms: paths::monotonic_ms(),
            owner_package: "com.eg.android.AlipayGphone".to_string(),
            package_name: "com.eg.android.AlipayGphone".to_string(),
            caller_uid: -1,
            tokens: vec!["xradiant".to_string()],
            source: "recent_private_owner",
            confidence: "medium",
        };

        let package = best_private_path_token_package(
            vec![
                "com.eg.android.AlipayGphone".to_string(),
                "com.android.providers.media.module".to_string(),
                "com.leo.xposed.xradiant".to_string(),
            ],
            &hint,
            &["xradiant".to_string()],
        )
        .expect("xradiant package");

        assert_eq!(package, "com.leo.xposed.xradiant");
    }

    #[test]
    fn public_path_token_can_infer_exporting_app_package() {
        use std::collections::HashMap;

        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();
        let previous_uid = policy::replace_test_uid_cache(HashMap::from([
            ("com.android.providers.downloads".to_string(), 10081),
            ("com.leo.xposed.xradiant".to_string(), 10164),
        ]));

        let identity = infer_public_path_token_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_201321.json",
            0,
        )
        .expect("public path token should infer XRadiant package");

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );

        assert_eq!(identity.package_name, "com.leo.xposed.xradiant");
        assert_eq!(identity.source, "public_path_token");
        assert_eq!(identity.confidence, "medium");
    }

    #[test]
    fn public_path_token_ignores_generic_backup_names() {
        use std::collections::HashMap;

        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();
        let previous_uid = policy::replace_test_uid_cache(HashMap::from([
            ("com.backup.viewer".to_string(), 10164),
            ("com.leo.xposed.xradiant".to_string(), 10165),
        ]));

        assert!(
            infer_public_path_token_identity(
                "/storage/emulated/0/Download/backup_20260629_201321.json",
                0,
            )
            .is_none()
        );

        policy::restore_test_uid_cache(
            previous_uid.0,
            previous_uid.1,
            previous_uid.2,
            previous_uid.3,
        );
    }

    #[test]
    fn recent_private_caller_hint_matches_public_download_token() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_private_path_caller_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
            10164,
            0,
        );

        let identity = infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json",
            0,
        )
        .expect("caller hint should match XRadiant export token");

        assert_eq!(identity.package_name, "com.leo.xposed.xradiant");
        assert_eq!(identity.source, "recent_private_caller");
        assert_eq!(identity.confidence, "medium");
    }

    #[test]
    fn recent_private_caller_hint_survives_long_saf_picker_flow() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_private_path_caller_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db-shm",
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
            10164,
            0,
        );
        if let Ok(mut hints) = RECENT_PRIVATE_OWNER_HINT.lock() {
            for hint in hints.iter_mut() {
                hint.updated_ms = paths::monotonic_ms().saturating_sub(120_000);
            }
        }

        let identity = infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_191435.json",
            0,
        )
        .expect("cross-app sqlite caller hint should survive SAF picker latency");

        assert_eq!(identity.package_name, "com.leo.xposed.xradiant");
        assert_eq!(identity.source, "recent_private_caller");
    }

    #[test]
    fn recent_private_owner_hint_keeps_short_window() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_private_path_owner_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            "com.eg.android.AlipayGphone",
            0,
        );
        if let Ok(mut hints) = RECENT_PRIVATE_OWNER_HINT.lock() {
            for hint in hints.iter_mut() {
                hint.updated_ms = paths::monotonic_ms().saturating_sub(120_000);
            }
        }

        assert!(
            infer_recent_private_owner_identity(
                "/storage/emulated/0/Download/XRadiant_backup_20260629_191435.json",
                0,
            )
            .is_none()
        );
    }

    #[test]
    fn recent_private_owner_hint_requires_token_overlap() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_private_path_owner_hint(
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db",
            "com.eg.android.AlipayGphone",
            0,
        );

        assert!(
            infer_recent_private_owner_identity(
                "/storage/emulated/0/Download/backup_20260618_183000.json",
                0,
            )
            .is_none()
        );
    }

    #[test]
    fn recent_private_caller_hint_wins_over_owner_hint() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        let private_path =
            "/storage/emulated/0/Android/media/com.eg.android.AlipayGphone/XRadiant/XRadiant.db";
        remember_private_path_caller_hint(
            private_path,
            "com.eg.android.AlipayGphone",
            "com.leo.xposed.xradiant",
            10164,
            0,
        );
        remember_private_path_owner_hint(private_path, "com.eg.android.AlipayGphone", 0);

        let identity = infer_recent_private_owner_identity(
            "/storage/emulated/0/Download/XRadiant_backup_20260623_183330.json",
            0,
        )
        .expect("matching private caller hint should survive later owner hint");

        assert_eq!(identity.package_name, "com.leo.xposed.xradiant");
        assert_eq!(identity.source, "recent_private_caller");
    }

    #[test]
    fn recent_public_path_caller_hint_survives_cross_process_file_read() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        let path =
            "/storage/emulated/0/Download/storage-redirect-x-backup-20260624-092038.srxbak.zip";
        remember_public_path_caller_hint(path, "org.srx.manager", 10081, "provider_open", "high");
        if let Ok(mut slot) = RECENT_PATH_CALLER_HINT.lock() {
            slot.clear();
        }

        let identity = infer_recent_path_caller_identity(path, 0).expect("identity");

        assert_eq!(identity.package_name, "org.srx.manager");
        assert_eq!(identity.source, "provider_open");
        assert_eq!(identity.confidence, "high");
    }

    #[test]
    fn recent_public_path_caller_hint_matches_media_store_pending_display_path() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_public_path_caller_hint(
            "/storage/emulated/0/Download/srt_ci_probe.part",
            "me.fakerqu.test.storageredirect",
            10366,
            "provider_open",
            "high",
        );

        let identity = infer_recent_path_caller_identity(
            "/storage/emulated/0/Download/.pending-1783044788-srt_ci_probe.part",
            0,
        )
        .expect("pending display path should match recent provider open hint");

        assert_eq!(identity.package_name, "me.fakerqu.test.storageredirect");
        assert_eq!(identity.source, "provider_open");
        assert_eq!(identity.confidence, "high");
    }

    #[test]
    fn recent_saf_caller_hint_matches_child_created_under_tree() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_saf_path_caller_hint(
            "/storage/emulated/0/Download/第三方下载/1DMP",
            "idm.internet.download.manager.plus",
            10317,
            "saf_provider",
            "high",
            "provider_open:create",
        );

        let identity = infer_recent_saf_caller_identity(
            "/storage/emulated/0/Download/第三方下载/1DMP/.tmppages",
            0,
        )
        .expect("saf path hint");

        assert_eq!(identity.package_name, "idm.internet.download.manager.plus");
        assert_eq!(identity.source, "saf_provider");
        assert_eq!(identity.confidence, "high");
        let generic_identity = infer_recent_path_caller_identity(
            "/storage/emulated/0/Download/第三方下载/1DMP/.tmppages",
            0,
        )
        .expect("generic path hint should also use saf attribution");

        assert_eq!(
            generic_identity.package_name,
            "idm.internet.download.manager.plus"
        );
        assert_eq!(generic_identity.source, "saf_provider");
        assert_eq!(generic_identity.confidence, "high");
    }

    #[test]
    fn recent_saf_caller_hint_matches_pending_display_name() {
        let _guard = lock_source_hint_test();
        clear_recent_private_owner_hint_for_tests();

        remember_saf_path_caller_hint(
            "/storage/emulated/0/Download/XRadiant_backup_20260629_140205.json",
            "com.leo.xposed.xradiant",
            10164,
            "saf_provider",
            "high",
            "provider_open:create",
        );

        let identity = infer_recent_saf_caller_identity(
            "/storage/emulated/0/Download/.pending-1751176925-XRadiant_backup_20260629_140205.json",
            0,
        )
        .expect("saf pending hint");

        assert_eq!(identity.package_name, "com.leo.xposed.xradiant");
        assert_eq!(identity.source, "saf_provider");
        assert_eq!(identity.confidence, "high");
    }
}
