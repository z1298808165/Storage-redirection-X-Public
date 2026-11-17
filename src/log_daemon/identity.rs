use crate::platform::proc_identity::{AttributionBoundary, RequestIdentity, attribution_boundary};

/// 仅对带 `field_encoding=percent` 的记录调用；旧日志中的百分号保持字面值。
fn decode(value: &str) -> std::borrow::Cow<'_, str> {
    if !["%25", "%7C", "%0D", "%0A"]
        .iter()
        .any(|escape| value.contains(escape))
    {
        return std::borrow::Cow::Borrowed(value);
    }
    std::borrow::Cow::Owned(
        value
            .replace("%7C", "|")
            .replace("%0D", "\r")
            .replace("%0A", "\n")
            .replace("%25", "%"),
    )
}

impl RequestIdentity {
    /// 从记录恢复原始任务字段；缺失的核验证据保持未知，不能补造 App 身份。
    fn from_log_fields(uid: i32, pid: i32, extra: &str) -> Self {
        let mut identity = Self::unresolved(uid, pid);
        let mut no_boundary = false;
        let encoded = extra
            .split('|')
            .any(|field| field == "field_encoding=percent");
        for field in extra.split('|') {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            match key {
                "caller_start_time" => identity.start_time = value.parse().unwrap_or(0),
                "caller_context" => {
                    identity.selinux_context = if encoded {
                        decode(value).into_owned()
                    } else {
                        value.to_owned()
                    }
                }
                "native" if !value.is_empty() => {
                    identity.native = if encoded {
                        decode(value).into_owned()
                    } else {
                        value.to_owned()
                    }
                }
                "caller_boundary" => {
                    no_boundary = value.is_empty();
                    identity.boundary = match value {
                        "root" => AttributionBoundary::Root,
                        "shell" => AttributionBoundary::Shell,
                        "privileged" => AttributionBoundary::Privileged,
                        "unverified" => AttributionBoundary::Unverified,
                        _ => identity.boundary,
                    };
                }
                _ => {}
            }
        }
        if no_boundary
            && identity.boundary == AttributionBoundary::Unverified
            && identity.pid > 0
            && identity.start_time > 0
            && !identity.native.is_empty()
            && attribution_boundary(&[uid; 4], &identity.selinux_context)
                == AttributionBoundary::None
        {
            identity.boundary = AttributionBoundary::None;
        }
        identity
    }
}

/// 允许普通进程报告自身请求；跨 UID 代写记录必须来自内核核验的系统代写进程。
pub(super) fn monitor_sender_is_trusted(message: &str, sender: Option<(u32, i32)>) -> bool {
    let Some((uid, pid)) = sender else {
        return false;
    };
    if monitor_record_is_ambiguous(message) {
        return false;
    }
    if uid == 0 {
        return true;
    }
    let caller_uid =
        monitor_field(message, "caller_uid").and_then(|value| value.parse::<u32>().ok());
    if caller_uid == Some(uid) {
        return true;
    }
    crate::platform::proc_identity::read_proc_identity_for_request(pid, uid as i32)
        .is_some_and(|identity| identity.is_system_writer())
}

fn monitor_record_is_ambiguous(message: &str) -> bool {
    if message.contains(['\r', '\n']) {
        return true;
    }
    let mut keys = std::collections::HashSet::new();
    for field in message.split('|').skip(5) {
        if let Some((key, _)) = field.split_once('=')
            && !keys.insert(key)
        {
            return true;
        }
    }
    false
}

fn monitor_field<'a>(message: &'a str, key: &str) -> Option<&'a str> {
    message.split('|').skip(5).find_map(|field| {
        let (name, value) = field.split_once('=')?;
        (name == key).then_some(value)
    })
}

/// 晚到日志只补经过任务凭据校验的身份；旧任务已失效时不查询复用 PID 的新来源。
pub(super) fn enrich_monitor_record_identity(message: &str) -> Option<String> {
    if monitor_field(message, "event_role") != Some("request") {
        return None;
    }
    // 请求发生时的终止边界不能被稍后的凭据变化或进程退出撤销。
    if monitor_field(message, "caller_boundary").is_some_and(|value| !value.is_empty())
        || monitor_field(message, "identify_method") == Some("shared_uid")
    {
        return None;
    }
    let uid = monitor_field(message, "caller_uid")?.parse::<i32>().ok()?;
    let pid = monitor_field(message, "caller_pid")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(-1);
    let Some(identity) = crate::platform::proc_identity::read_proc_identity_for_request(pid, uid)
    else {
        let mut identity =
            crate::platform::proc_identity::RequestIdentity::from_log_fields(uid, pid, message);
        identity.revalidate(None);
        let (label, method, reliability) = identity.monitor_identity();
        return Some(rewrite_monitor_identity(
            message,
            label,
            method,
            reliability,
            Some(&identity),
        ));
    };
    let mut captured =
        crate::platform::proc_identity::RequestIdentity::from_log_fields(uid, pid, message);
    if captured.start_time > 0 && !captured.same_task(&identity) {
        captured.invalidate();
        let (label, method, reliability) = captured.monitor_identity();
        return Some(rewrite_monitor_identity(
            message,
            label,
            method,
            reliability,
            Some(&captured),
        ));
    }
    if identity.is_terminal() {
        let (label, method, reliability) = identity.monitor_identity();
        return Some(rewrite_monitor_identity(
            message,
            label,
            method,
            reliability,
            Some(&identity),
        ));
    }
    let fields: Vec<_> = message.split('|').collect();
    let label = fields.get(2).copied().unwrap_or("");
    if crate::redirect::policy::get_uid_for_package_in_user(label, uid / 100000) == uid {
        return None;
    }
    let cross_thread = monitor_field(message, "caller_evidence") == Some("cross_thread");
    let reliability = if cross_thread { "medium" } else { "high" };
    let (label, method, reliability) = if !identity.package.is_empty() {
        (identity.package.as_str(), "caller_pid", reliability)
    } else if !identity.shared_user_id.is_empty() {
        (identity.shared_user_id.as_str(), "shared_uid", "fallback")
    } else {
        ("-", "unknown", "none")
    };
    Some(rewrite_monitor_identity(
        message,
        label,
        method,
        reliability,
        Some(&identity),
    ))
}

pub(super) fn rewrite_monitor_identity(
    message: &str,
    label: &str,
    method: &str,
    reliability: &str,
    identity: Option<&crate::platform::proc_identity::RequestIdentity>,
) -> String {
    let ambiguous = monitor_record_is_ambiguous(message);
    let mut fields: Vec<_> = message.split('|').map(str::to_owned).collect();
    if fields.len() < 5 {
        return message.to_owned();
    }
    let encoded = monitor_field(message, "field_encoding") == Some("percent");
    let mut extra = fields.split_off(5);
    if !encoded {
        // 旧记录中的百分号是字面值，只做编码升级，不能先解码。
        for field in &mut fields {
            *field = crate::platform::log_fields::encode(field).into_owned();
        }
        for field in &mut extra {
            if let Some((key, value)) = field.split_once('=') {
                *field = format!("{key}={}", crate::platform::log_fields::encode(value));
            } else {
                *field = crate::platform::log_fields::encode(field).into_owned();
            }
        }
        extra.retain(|field| !field.starts_with("field_encoding="));
        extra.push("field_encoding=percent".into());
    }
    fields[2] = crate::platform::log_fields::encode(label).into_owned();
    extra.retain(|field| {
        !field.starts_with("identify_method=") && !field.starts_with("identify_reliability=")
    });
    if ambiguous {
        extra.retain(|field| !field.starts_with("record_integrity="));
        extra.push("record_integrity=ambiguous".into());
    }
    let extra = extra.join("|");
    let extra = identity
        .map(|identity| identity.with_log_fields(&extra))
        .unwrap_or(extra);
    format!(
        "{}|{extra}|identify_method={method}|identify_reliability={reliability}",
        fields.join("|")
    )
    .replace('\r', "\\r")
    .replace('\n', "\\n")
}

fn can_register_request_path_hint(message: &str) -> bool {
    !(monitor_record_is_ambiguous(message)
        || monitor_field(message, "record_integrity") == Some("ambiguous")
        || monitor_field(message, "event_role") != Some("request")
        || monitor_field(message, "ret")
            .and_then(|value| value.parse::<i64>().ok())
            .is_none_or(|ret| ret < 0)
        || monitor_field(message, "identify_reliability") != Some("high")
        || !matches!(
            monitor_field(message, "identify_method"),
            Some(
                "caller"
                    | "caller_pid"
                    | "caller_uid"
                    | "provider_open"
                    | "provider_mkdir"
                    | "saf_provider"
            )
        )
        || monitor_field(message, "caller_boundary").is_some_and(|value| !value.is_empty()))
}

pub(super) fn register_request_path_hint(message: &str) {
    if !can_register_request_path_hint(message) {
        return;
    }
    // 行格式：时间戳|进程包名|目标包名|OP|路径|ret=..|k=v...；额外字段从下标 5 起。
    let fields: Vec<&str> = message.split('|').collect();
    if fields.len() < 6 {
        return;
    }
    let Some(op_filter) = fields
        .iter()
        .skip(5)
        .find_map(|field| field.strip_prefix("op_filter="))
    else {
        return;
    };
    let (source, hint_op_filter) = match op_filter {
        "mkdir" => ("provider_mkdir", "mkdir"),
        "provider_open" | "provider_open:create" | "provider_open:write" => {
            ("provider_open", op_filter)
        }
        // 原生 hook 的写入类 open（映射路径等场景）：请求行本就是台账对这次
        // 写入的记录，映射到 provider_open 写形态登记 hint，让纯映射路径的
        // request/观察记录也走同一套 10 秒窗口去重。
        "open:create" => ("provider_open", "provider_open:create"),
        "open:write" => ("provider_open", "provider_open:write"),
        _ => return,
    };
    let caller_uid = fields
        .iter()
        .skip(5)
        .find_map(|field| field.strip_prefix("caller_uid="))
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(-1);
    if caller_uid < 0
        || crate::redirect::policy::get_uid_for_package_in_user(fields[2], caller_uid / 100000)
            != caller_uid
    {
        return;
    }
    let correlation_id = fields
        .iter()
        .skip(5)
        .find_map(|field| field.strip_prefix("correlation_id="))
        .unwrap_or("");
    let path = if monitor_field(message, "field_encoding") == Some("percent") {
        decode(fields[4])
    } else {
        std::borrow::Cow::Borrowed(fields[4])
    };
    crate::monitor::remember_public_path_caller_hint_with_op_filter(
        &path,
        fields[2],
        caller_uid,
        source,
        "high",
        hint_op_filter,
        correlation_id,
    );
}

#[cfg(test)]
#[path = "../../tests/native-unit/log_identity.rs"]
mod tests;
