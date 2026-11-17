use native_config_fixtures::platform::packages::{
    MAX_REPLY, decode_reply, parse_app_inventory, parse_uids, parse_users,
};

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
