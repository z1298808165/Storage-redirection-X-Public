//! 验证配置解析失败时保留上一次生效的配置，而不是静默回落默认值。

use native_config_fixtures::{
    GlobalConfigOutcome, parse_global_config_from, parse_monitor_filters_from,
    parse_storage_backend_mode,
};

/// 上一次生效的开关：监控开启、fuse fix 关闭、历史字段开启、详细日志开启。
/// 故意与默认值全部相反，这样"保留"和"回落默认"的结果不会混淆。
const PREVIOUS: (bool, bool, bool, bool) = (true, false, true, true);

#[test]
fn truncated_global_config_keeps_previous_switches() {
    // 管理端覆盖写入的瞬间读到半个 JSON 时，不得静默关掉文件监控。
    let outcome = parse_global_config_from(PREVIOUS, r#"{"file_monitor_enabled": tr"#);
    assert_eq!(
        outcome,
        GlobalConfigOutcome {
            is_ok: false,
            is_file_monitor_enabled: true,
            is_fuse_fix_enabled: false,
            is_fuse_daemon_redirect_enabled: true,
            is_verbose_logging_enabled: true,
        }
    );
}

#[test]
fn valid_global_config_overwrites_previous_switches() {
    // 解析成功时必须完整落值，不能因为保留逻辑而漏改。
    let outcome = parse_global_config_from(
        PREVIOUS,
        r#"{"file_monitor_enabled": false, "fuse_fix_enabled": true,
            "fuse_daemon_redirect_enabled": false, "verbose_logging_enabled": false}"#,
    );
    assert_eq!(
        outcome,
        GlobalConfigOutcome {
            is_ok: true,
            is_file_monitor_enabled: false,
            is_fuse_fix_enabled: true,
            is_fuse_daemon_redirect_enabled: false,
            is_verbose_logging_enabled: false,
        }
    );
}

#[test]
fn missing_keys_fall_back_to_defaults_not_previous_values() {
    // 解析成功但键缺失表示用户确实没有配置该项，应使用各自默认值，
    // 而不是沿用上一次的值——否则开关一旦打开就再也回不去。
    let outcome = parse_global_config_from(PREVIOUS, "{}");
    assert_eq!(
        outcome,
        GlobalConfigOutcome {
            is_ok: true,
            is_file_monitor_enabled: false,
            is_fuse_fix_enabled: true,
            is_fuse_daemon_redirect_enabled: false,
            is_verbose_logging_enabled: false,
        }
    );
}

#[test]
fn non_bool_values_fall_back_to_key_defaults() {
    // 类型错误的值按该键的默认值处理，保持与原实现一致。
    let outcome = parse_global_config_from(
        PREVIOUS,
        r#"{"file_monitor_enabled": "yes", "fuse_fix_enabled": 0}"#,
    );
    assert_eq!(
        outcome,
        GlobalConfigOutcome {
            is_ok: true,
            is_file_monitor_enabled: false,
            is_fuse_fix_enabled: true,
            is_fuse_daemon_redirect_enabled: false,
            is_verbose_logging_enabled: false,
        }
    );
}

#[test]
fn truncated_monitor_filters_keep_previous_excluded_paths() {
    // 清空排除列表会让本应被排除的目录重新产生监控记录，损坏时必须保留旧规则。
    let (is_ok, excluded) =
        parse_monitor_filters_from(&["/storage/emulated/*/Download"], r#"{"excluded_paths": ["#);
    assert!(!is_ok, "损坏的过滤配置应报告解析失败");
    assert_eq!(excluded, vec!["/storage/emulated/*/Download".to_string()]);
}

#[test]
fn valid_monitor_filters_replace_previous_excluded_paths() {
    // 排除规则按相对路径配置，解析后展开为跨用户的 /storage/emulated/*/ 形式。
    let (is_ok, excluded) = parse_monitor_filters_from(
        &["/storage/emulated/*/Download"],
        r#"{"excluded_paths": ["DCIM"]}"#,
    );
    assert!(is_ok);
    assert_eq!(excluded, vec!["/storage/emulated/*/DCIM".to_string()]);
}

#[test]
fn storage_backend_mode_always_uses_auto_and_ignores_legacy_values() {
    assert_eq!(parse_storage_backend_mode("{}", false), "auto");
    assert_eq!(
        parse_storage_backend_mode(r#"{"fuse_daemon_redirect_enabled":true}"#, false),
        "auto"
    );
    assert_eq!(
        parse_storage_backend_mode(r#"{"storage_backend_mode":"fuse"}"#, false),
        "auto"
    );
    assert_eq!(
        parse_storage_backend_mode(r#"{"storage_backend_mode":"namespace"}"#, true),
        "auto"
    );
}
