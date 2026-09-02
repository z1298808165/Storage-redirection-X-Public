//! 验证多用户路径改写只作用于前缀，不会改写路径中间的同名字面量。

use native_config_fixtures::platform::paths;

#[test]
fn rewrites_storage_prefix_for_secondary_user() {
    assert_eq!(
        paths::resolve_user_path("/storage/emulated/0/DCIM/a.jpg", 10),
        "/storage/emulated/10/DCIM/a.jpg"
    );
    // 不带尾部内容的根路径也要改写。
    assert_eq!(
        paths::resolve_user_path("/storage/emulated/0", 10),
        "/storage/emulated/10"
    );
    assert_eq!(
        paths::resolve_user_path("/storage/emulated/0/", 11),
        "/storage/emulated/11/"
    );
}

#[test]
fn rewrites_data_user_prefix_for_secondary_user() {
    assert_eq!(
        paths::resolve_user_path("/data/user/0/com.example/files", 10),
        "/data/user/10/com.example/files"
    );
    assert_eq!(
        paths::resolve_user_path("/data/user/0", 10),
        "/data/user/10"
    );
}

#[test]
fn rewrites_legacy_data_data_alias_for_secondary_user() {
    assert_eq!(
        paths::resolve_user_path("/data/data/com.example/files", 0),
        "/data/user/0/com.example/files"
    );
    assert_eq!(
        paths::resolve_user_path("/data/data/com.example/files", 10),
        "/data/user/10/com.example/files"
    );
}

#[test]
fn does_not_rewrite_literal_inside_path() {
    // 关键回归点：备份类目录里可能出现存储路径字面量。此前用整串 replace 会把
    // 中间那一段也改写，导致解析出的路径与真实文件不符。
    assert_eq!(
        paths::resolve_user_path(
            "/storage/emulated/0/Download/storage/emulated/0/note.txt",
            10
        ),
        "/storage/emulated/10/Download/storage/emulated/0/note.txt"
    );
    // 路径不以受支持的前缀开头时完全不改写。
    assert_eq!(
        paths::resolve_user_path("/data/local/tmp/storage/emulated/0/x", 10),
        "/data/local/tmp/storage/emulated/0/x"
    );
}

#[test]
fn does_not_rewrite_partial_segment_match() {
    // /storage/emulated/09 不是 /storage/emulated/0 的子路径，必须原样保留。
    assert_eq!(
        paths::resolve_user_path("/storage/emulated/09/DCIM", 10),
        "/storage/emulated/09/DCIM"
    );
    assert_eq!(
        paths::resolve_user_path("/data/user/0abc/files", 10),
        "/data/user/0abc/files"
    );
}

#[test]
fn primary_user_path_is_unchanged() {
    let path = "/storage/emulated/0/DCIM/a.jpg";
    assert_eq!(paths::resolve_user_path(path, 0), path);
}
