//! 验证 hook 决策入口使用的 `.`/`..` 折叠语义，以及配置校验仍能拒绝越界规则。

use native_config_fixtures::platform::paths;

#[test]
fn collapses_parent_segments_to_kernel_equivalent_path() {
    // 只读保护绕过场景：折叠后必须落在真实目标目录上，规则才能匹配。
    assert_eq!(
        paths::collapse_dot_segments("/storage/emulated/0/Pictures/../ReadOnlyDir/a.jpg"),
        "/storage/emulated/0/ReadOnlyDir/a.jpg"
    );
    // 连续多级回退。
    assert_eq!(
        paths::collapse_dot_segments("/storage/emulated/0/A/B/../../DCIM"),
        "/storage/emulated/0/DCIM"
    );
    // 当前目录段直接丢弃。
    assert_eq!(
        paths::collapse_dot_segments("/storage/emulated/0/./DCIM/./a.jpg"),
        "/storage/emulated/0/DCIM/a.jpg"
    );
    // 重复斜杠与尾斜杠一并规整。
    assert_eq!(
        paths::collapse_dot_segments("/storage//emulated/0/DCIM/"),
        "/storage/emulated/0/DCIM"
    );
}

#[test]
fn absolute_path_drops_parent_segments_at_root() {
    // 与内核一致：根目录处的 `..` 无处可退，直接丢弃而不是越出根。
    assert_eq!(
        paths::collapse_dot_segments("/../../etc/passwd"),
        "/etc/passwd"
    );
    assert_eq!(paths::collapse_dot_segments("/.."), "/");
    assert_eq!(paths::collapse_dot_segments("/"), "/");
}

#[test]
fn relative_path_keeps_leading_parent_segments() {
    // 相对路径的前导 `..` 无法在词法层解析，必须原样保留。
    assert_eq!(paths::collapse_dot_segments("../DCIM"), "../DCIM");
    assert_eq!(paths::collapse_dot_segments("../../A/B"), "../../A/B");
    // 中间的 `..` 仍可抵消。
    assert_eq!(paths::collapse_dot_segments("A/../B"), "B");
    assert_eq!(paths::collapse_dot_segments("../A/../B"), "../B");
}

#[test]
fn paths_without_dot_segments_skip_collapse() {
    // 预筛必须放过不含点段的普通路径，避免热路径上多余扫描。
    assert!(!paths::needs_dot_segment_collapse(
        "/storage/emulated/0/DCIM/a.jpg"
    ));
    // 文件名中的点不构成点段。
    assert!(!paths::needs_dot_segment_collapse(
        "/storage/emulated/0/..a/b"
    ));
    assert!(!paths::needs_dot_segment_collapse(
        "/storage/emulated/0/a../b"
    ));
    // 真正的点段必须被识别。
    assert!(paths::needs_dot_segment_collapse(
        "/storage/emulated/0/A/../B"
    ));
    assert!(paths::needs_dot_segment_collapse("/storage/emulated/0/./B"));
}

#[test]
fn normalize_keeps_parent_segments_for_config_validation() {
    // 关键回归点：配置校验依赖 normalize 之后仍能看到 `..` 才能拒绝越界规则。
    // 若在 normalize 内提前折叠，DCIM/../../etc 会被折叠成合法路径而通过校验。
    let normalized = paths::normalize("DCIM/../../etc");
    assert!(
        paths::has_unsafe_segments(&normalized),
        "normalize 不得折叠 .. 段，否则配置校验的拒绝逻辑失效：{normalized}"
    );

    let normalized_absolute = paths::normalize("/storage/emulated/0/DCIM/../../../etc");
    assert!(
        paths::has_unsafe_segments(&normalized_absolute),
        "normalize 不得折叠 .. 段：{normalized_absolute}"
    );
}

#[test]
fn normalize_still_collapses_slashes_and_trailing_slash() {
    // 折叠函数拆分后，normalize 原有的斜杠规整行为必须保持不变。
    assert_eq!(
        paths::normalize("/storage//emulated/0/DCIM/"),
        "/storage/emulated/0/DCIM"
    );
    assert_eq!(paths::normalize("/"), "/");
}
