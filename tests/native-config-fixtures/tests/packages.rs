use native_config_fixtures::platform::packages::{
    MAX_REPLY, decode_reply, encode_base64, parse_app_inventory, parse_uids, parse_users,
};

#[test]
fn inventory_base64_payload_keeps_percent_fields_and_tabs_verbatim() {
    // WebUI 的文本通道会把 `%XX` 当 URL 转义解码一次；清单靠 percent 编码保真，
    // 因此发给它的回复必须先编码成只含字母数字与 `-`/`_` 的形式（issue 20）。
    let payload = "0\t1000\t1\tandroid\tAndroid%20%E7%B3%BB%E7%BB%9F\t%2Fsystem%2Fframework%2Fframework-res.apk\n";
    let encoded = encode_base64(payload.as_bytes());
    assert!(!encoded.contains('%'));
    assert!(!encoded.contains('\t'));
    assert!(!encoded.contains('+'));
    assert!(!encoded.contains('/'));
    assert_eq!(
        encoded,
        "MAkxMDAwCTEJYW5kcm9pZAlBbmRyb2lkJTIwJUU3JUIzJUJCJUU3JUJCJTlGCSUyRnN5c3RlbSUyRmZyYW1ld29yayUyRmZyYW1ld29yay1yZXMuYXBrCg=="
    );
    // 不足三字节的尾部必须补 `=`，否则 WebView 的 atob 会抛 InvalidCharacterError。
    assert_eq!(encode_base64(b"ab"), "YWI=");
    assert_eq!(encode_base64(b"a"), "YQ==");
    assert_eq!(encode_base64(b""), "");
    assert_eq!(encode_base64(&[0xfb, 0xff]), "-_8=");
}

#[test]
fn app_inventory_keeps_user_metadata_and_decodes_labels_and_paths() {
    let apps = parse_app_inventory(
        "10\t1010123\t1\tcom.example.system\tSystem%20%E2%9C%93\t%2Fsystem%2Fapp%2Fexample%2Fbase.apk\n10\t1010456\t0\tcom.example.user\tUser%09App\t%2Fdata%2Fapp%2Fbase.apk\n",
        10,
    )
    .unwrap();
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[0].user_id, 10);
    assert_eq!(apps[0].uid, 1_010_123);
    assert!(apps[0].is_system);
    assert_eq!(apps[0].label, "System ✓");
    assert_eq!(apps[0].apk_path, "/system/app/example/base.apk");
    assert!(!apps[1].is_system);
    assert_eq!(apps[1].label, "User\tApp");
}

#[test]
fn app_inventory_reads_versioned_seven_column_format() {
    // v2：首行协议版本 + 末列 icon。旧版无首行按 6 列解析，icon 缺省 0。
    let v2 = concat!(
        "#srx-inventory\t2\n",
        "10\t1010123\t1\tcom.example.system\tSys\t%2Fsystem%2Fapp.apk\t2130903040\n",
    );
    let apps = parse_app_inventory(v2, 10).unwrap();
    assert_eq!(apps.len(), 1);
    assert_eq!(apps[0].icon, 2_130_903_040);

    let legacy = "10\t1010456\t0\tcom.example.user\tUser\t%2Fdata%2Fapp.apk\n";
    let apps = parse_app_inventory(legacy, 10).unwrap();
    assert_eq!(apps[0].icon, 0);

    // 未知版本必须报错，而不是按 6 列蒙混过关。
    let unknown = "#srx-inventory\t9\n";
    assert!(parse_app_inventory(unknown, 10).is_err());
}

#[test]
fn app_inventory_keeps_negative_icon_resource_ids() {
    // 资源包 ID 高位为 1 时 `ApplicationInfo.icon` 作为有符号 int 是负数，
    // 例如 B 站（资源包 ID 0x90）的启动器图标 `0x900E0000` 即 `-1878130688`。
    // 这是**有效**资源 ID：归零会把它换成系统默认图标，属功能回退。
    // 因此负数必须保真，且不得因单个负值使整份清单被拒（真机 566 行清单曾整体作废）。
    let v2 = concat!(
        "#srx-inventory\t2\n",
        "0\t10393\t0\tcom.padi.pilipili\tPILIPILI\t%2Fdata%2Fapp%2Fbili%2Fbase.apk\t-1878130688\n",
        "0\t10400\t1\tcom.example.system\tSys\t%2Fsystem%2Fapp.apk\t2130903040\n",
    );
    let apps = parse_app_inventory(v2, 0).unwrap();
    assert_eq!(apps.len(), 2);
    assert_eq!(apps[0].icon, -1_878_130_688);
    assert_eq!(apps[0].label, "PILIPILI");
    assert_eq!(apps[0].apk_path, "/data/app/bili/base.apk");
    assert_eq!(apps[1].icon, 2_130_903_040);

    // 边界：i32::MIN 是可表示的最小资源 ID；再小一就必须拒绝（溢出而非静默截断）。
    let min_row =
        "#srx-inventory\t2\n0\t10393\t0\tcom.example.min\tMin\t%2Fdata%2Fapp.apk\t-2147483648\n";
    assert_eq!(
        parse_app_inventory(min_row, 0).unwrap()[0].icon,
        i32::MIN,
        "i32::MIN 应保真"
    );
    let underflow =
        "#srx-inventory\t2\n0\t10393\t0\tcom.example.min\tMin\t%2Fdata%2Fapp.apk\t-2147483649\n";
    assert!(
        parse_app_inventory(underflow, 0).is_err(),
        "超出 i32 范围的资源 ID 必须拒绝"
    );
}

#[test]
fn app_inventory_still_rejects_malformed_icon_fields() {
    // 放宽符号位不等于放弃校验：icon 的形态由我方生成，畸形即协议违约，必须暴露。
    for icon in ["", "-", "--1", "1-", "abc", "1.5", " 1", "0x90", "+1"] {
        let row = format!(
            "#srx-inventory\t2\n10\t1010123\t0\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\t{icon}\n"
        );
        assert!(
            parse_app_inventory(&row, 10).is_err(),
            "malformed icon accepted: {icon:?}"
        );
    }
}

#[test]
fn app_inventory_rejects_wrong_user_duplicate_packages_and_bad_fields() {
    for invalid in [
        (
            "11\t1010123\t1\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        (
            "10\t1010123\t1\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\n10\t1010124\t0\tcom.example.app\tAgain\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        (
            "10\t-1\t0\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        (
            "10\t10123\t0\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        (
            "10\t+10123\t0\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        (
            "+10\t10123\t0\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        (
            "10\t1010123\t2\tcom.example.app\tExample\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        ("10\t1010123\t0\tbad/name\tExample\t%2Fdata%2Fapp.apk\n", 10),
        (
            "10\t1010123\t0\tcom.example.app\tBad%2\t%2Fdata%2Fapp.apk\n",
            10,
        ),
        (
            "10\t1010123\t0\tcom.example.app\tExample\trelative.apk\n",
            10,
        ),
        ("10\t1010123\t0\tcom.example.app\tExample\t%FF\n", 10),
    ] {
        assert!(
            parse_app_inventory(invalid.0, invalid.1).is_err(),
            "{invalid:?}"
        );
    }
    assert!(parse_app_inventory("10\t1\t0\tcom.example.app\tExample\t\n", -1).is_err());
}

#[test]
fn user_inventory_preserves_ids_and_rejects_empty_duplicate_or_negative_ids() {
    assert_eq!(parse_users("0\n10\n999\n").unwrap(), [0, 10, 999]);
    for invalid in ["", "0\n0\n", "-1\n", "+1\n", "user\n", "0\t10\n"] {
        assert!(parse_users(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn shared_user_reply_keeps_group_label_and_rejects_multiple_or_malformed_values() {
    use native_config_fixtures::platform::packages::parse_shared_user_id;
    assert_eq!(parse_shared_user_id("").unwrap(), None);
    assert_eq!(
        parse_shared_user_id("android.uid.system\n").unwrap(),
        Some("android.uid.system".into())
    );
    for invalid in [
        "android.uid.system\nandroid.uid.other\n",
        "literal",
        "shared-user",
        "android..uid",
        "android.uid\0",
        "android.uid\n\n",
        "../escape",
        "android.uid\t",
    ] {
        assert!(parse_shared_user_id(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn package_inventory_preserves_shared_uids_and_users() {
    let rows = parse_uids("android:1000\ncom.example.one:10123\ncom.example.two:10123\ncom.example.one:1010123\ncom.example.one:10123\n").unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[&10123].len(), 2);
    assert!(rows[&10123].contains("com.example.one"));
    assert!(rows[&10123].contains("com.example.two"));
    assert_eq!(rows[&1010123].len(), 1);
    assert!(parse_uids("").unwrap().is_empty());
}

#[test]
fn malformed_inventory_fails_instead_of_accepting_a_partial_snapshot() {
    for row in [
        "bad",
        "bad:-1",
        "bad:+1000",
        "bad:4294977296",
        "bad:not-a-uid",
        "bad:1:2",
        ":1000",
        "../escape:1000",
        "com..app:1000",
        "bad\tname:1000",
        "bad\0name:1000",
        "\n",
    ] {
        assert!(
            parse_uids(&format!("com.valid.app:10123\n{row}")).is_err(),
            "{row:?}"
        );
    }
}

#[test]
fn reply_framing_rejects_truncation_encoding_size_and_service_errors() {
    assert_eq!(decode_reply(3u32.to_be_bytes(), b"OK\n").unwrap(), "");
    let body = b"OK\ncom.example.app:10123\n";
    assert_eq!(
        decode_reply((body.len() as u32).to_be_bytes(), body).unwrap(),
        "com.example.app:10123\n"
    );
    for bad in [b"ERR\nunavailable".as_slice(), b"OK\xff", b"NO\n", b"OK"] {
        assert!(decode_reply((bad.len() as u32).to_be_bytes(), bad).is_err());
    }
    assert!(decode_reply((body.len() as u32 + 1).to_be_bytes(), body).is_err());
    assert!(decode_reply((body.len() as u32 - 1).to_be_bytes(), body).is_err());
    assert!(decode_reply(((MAX_REPLY + 1) as u32).to_be_bytes(), b"").is_err());
    assert!(decode_reply(u32::MAX.to_be_bytes(), b"").is_err());
}
