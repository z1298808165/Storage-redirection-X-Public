use native_config_fixtures::domain::{PathMapping, expand_namespace_path_mappings};
use native_config_fixtures::platform::paths;

fn mapping(request: &str, target: &str) -> PathMapping {
    PathMapping::new(request.into(), target.into())
}

const STORAGE_ROOT: &str = "/storage/emulated/0";

/// 与 `fuse_redirect::config::scoped_mapping_mount_contribution` 及
/// `resolve_scoped_path_mappings` 的判定保持一致：只有**目标侧**落在共享存储
/// 内的映射才被 scoped FUSE 接纳——两侧都在存储内时由请求侧承接，请求侧在
/// 存储外而目标侧在存储内时由目标侧承接。目标侧在存储外时 scoped FUSE 的
/// 后端无法表达该映射（策略层 `resolve_storage_rule` 同样拒绝），必须继续
/// 丢弃；否则带通配的请求侧会借通配根推导扩大 FUSE 接管范围。
fn scoped_mapping_is_admitted(rule: &PathMapping, storage_root: &str) -> bool {
    let request = rule.request_path.as_str();
    let target = rule.final_path.as_str();
    if paths::is_application_private_root(request) || paths::eq_ignore_case(request, target) {
        return false;
    }
    paths::is_same_or_child(target, storage_root)
}

/// 真机回归复现（nu.gpu.nagram，v1.2.60-ci.40，用户反馈"TG 里看不到
/// Android/data/nu.gpu.nagram/files/ 里的文件"，ci.37 正常、ci.38 起复现）。
///
/// 配置：
/// ```json
/// "path_mappings": {
///   "/data/user/0/nu.gpu.nagram/cache": "Android/data/nu.gpu.nagram/cache",
///   "Android/data/nu.gpu.nagram/files/Telegram/Telegram Files": "Download/Nagram"
/// }
/// ```
///
/// 现象：daemon 日志 `scoped roots diag ... mappings=2 wildcard_roots=0`，
/// 但只有 `Download/Nagram` 那条建立了 bind（`map primary result` 存在）；
/// `cache` 那条既无挂载记录也无失败告警。应用侧表现为
/// `Android/data/nu.gpu.nagram/cache/*.jpg` 全部 `open failed: ENOENT`。
///
/// 根因：scoped 挂载根推导要求映射的**请求侧**与**目标侧**都落在共享存储根
/// 之下，否则整条规则被静默丢弃。请求侧是应用私有数据目录
/// （`/data/user/0/<pkg>/cache`）时不满足，于是：
///   - 规则表里没有这条映射，目标侧目录也从未被创建；
///   - namespace bind 侧以不存在的目标作 bind 源，静默失败；
///   - 应用读目标侧路径时无规则可循，落到真实存储，而那里没有该目录，
///     最终 ENOENT——正是「TG 自己扫盘，扫不到就当没下载过」。
///
/// 修复：这类映射由**目标侧**贡献 scoped FUSE 挂载根（目标侧才是应用访问的
/// 路径），使其进入 FUSE 视图并按映射规则路由。
#[test]
fn private_data_request_mapping_survives_scoped_root_filter() {
    let rules = [
        mapping(
            "/data/user/0/nu.gpu.nagram/cache",
            "/storage/emulated/0/Android/data/nu.gpu.nagram/cache",
        ),
        mapping(
            "/storage/emulated/0/Android/data/nu.gpu.nagram/files/Telegram/Telegram Files",
            "/storage/emulated/0/Download/Nagram",
        ),
    ];

    let admitted: Vec<&PathMapping> = rules
        .iter()
        .filter(|rule| scoped_mapping_is_admitted(rule, STORAGE_ROOT))
        .collect();

    assert!(
        admitted.len() == rules.len(),
        "private-data request mapping must not be dropped by the scoped-root filter; \
         admitted={admitted:?}"
    );
}

/// 请求侧在存储外时，挂载根必须由**目标侧**贡献——目标侧才是应用访问的路径。
#[test]
fn private_data_request_mapping_contributes_target_side_as_mount_root() {
    let request = "/data/user/0/nu.gpu.nagram/cache";
    let target = "/storage/emulated/0/Android/data/nu.gpu.nagram/cache";

    assert!(
        !paths::is_same_or_child(request, STORAGE_ROOT),
        "request side is outside shared storage by construction"
    );
    assert!(paths::is_same_or_child(target, STORAGE_ROOT));
    assert!(paths::is_child(target, STORAGE_ROOT));
}

/// 请求侧与目标侧都在共享存储之外的映射仍不能被 scoped FUSE 承接
/// （FUSE 视图覆盖不到那里），必须继续被丢弃。
#[test]
fn mapping_outside_storage_on_both_sides_stays_rejected() {
    let rule = mapping(
        "/data/user/0/nu.gpu.nagram/anything",
        "/data/user/0/nu.gpu.nagram/other",
    );
    assert!(!scoped_mapping_is_admitted(&rule, STORAGE_ROOT));
}

/// 请求侧在存储内、目标侧在存储外的映射同样不能被 scoped FUSE 承接：
/// 后端以共享存储为根，无法表达存储外目标，策略层也会拒绝。若放行，
/// 带通配的请求侧会经通配根推导在公共目录上扩大 FUSE 接管范围，而策略表
/// 里没有对应规则。这类映射仍由挂载层 namespace bind 承接，行为不变。
#[test]
fn storage_request_with_outside_target_stays_rejected() {
    let rule = mapping(
        "/storage/emulated/0/Download/Nagram",
        "/data/user/0/nu.gpu.nagram/other",
    );
    assert!(paths::is_same_or_child(&rule.request_path, STORAGE_ROOT));
    assert!(!paths::is_same_or_child(&rule.final_path, STORAGE_ROOT));
    assert!(!scoped_mapping_is_admitted(&rule, STORAGE_ROOT));
}

/// 请求侧在存储内的常规映射不受影响。
#[test]
fn scoped_root_filter_keeps_storage_request_mapping() {
    let rules = [mapping(
        "/storage/emulated/0/Android/data/nu.gpu.nagram/files/Telegram/Telegram Files",
        "/storage/emulated/0/Download/Nagram",
    )];
    let admitted: Vec<&PathMapping> = rules
        .iter()
        .filter(|rule| scoped_mapping_is_admitted(rule, STORAGE_ROOT))
        .collect();
    assert_eq!(admitted.len(), 1);
}

/// 应用私有根目录本身仍然禁止作为映射请求侧（改动不得放宽这条既有边界）。
#[test]
fn application_private_root_is_still_rejected() {
    let rule = mapping(
        "/data/user/0/nu.gpu.nagram",
        "/storage/emulated/0/Android/data/nu.gpu.nagram",
    );
    assert!(!scoped_mapping_is_admitted(&rule, STORAGE_ROOT));
}

/// `/data/user/0/<pkg>/<sub>` 是应用私有目录下的**子路径**，不是私有根，
/// 必须允许作为映射请求侧（私有根本身才禁止）。
#[test]
fn private_data_subdirectory_is_not_application_private_root() {
    assert!(paths::is_application_private_root(
        "/data/user/0/nu.gpu.nagram"
    ));
    assert!(!paths::is_application_private_root(
        "/data/user/0/nu.gpu.nagram/cache"
    ));
}

/// 领域层不得因为链式解析而丢掉私有数据请求侧的映射。
#[test]
fn private_data_request_mapping_is_not_dropped_by_domain_expansion() {
    let rules = vec![
        mapping(
            "/data/user/0/nu.gpu.nagram/cache",
            "/storage/emulated/0/Android/data/nu.gpu.nagram/cache",
        ),
        mapping(
            "/storage/emulated/0/Android/data/nu.gpu.nagram/files/Telegram/Telegram Files",
            "/storage/emulated/0/Download/Nagram",
        ),
    ];
    let expanded = expand_namespace_path_mappings(&rules);
    assert!(
        expanded.contains(&rules[0]) && expanded.contains(&rules[1]),
        "expanded={expanded:?}"
    );
}
