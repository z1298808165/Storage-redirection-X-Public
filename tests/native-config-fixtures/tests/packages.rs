use native_config_fixtures::platform::packages::{MAX_REPLY, decode_reply, parse_uids};

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
