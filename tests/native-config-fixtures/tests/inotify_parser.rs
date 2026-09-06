use native_config_fixtures::platform::inotify::for_each_event;

#[test]
fn parser_dispatches_complete_event() {
    let header = [
        7i32.to_ne_bytes(),
        3u32.to_ne_bytes(),
        0u32.to_ne_bytes(),
        0u32.to_ne_bytes(),
    ]
    .concat();
    let mut seen = Vec::new();
    for_each_event(&header, |event| seen.push((event.wd, event.mask)));
    assert_eq!(seen, vec![(7, 3)]);
}

#[test]
fn parser_ignores_truncated_payload() {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(&7i32.to_ne_bytes());
    bytes[4..8].copy_from_slice(&3u32.to_ne_bytes());
    bytes[12..16].copy_from_slice(&4u32.to_ne_bytes());
    let mut count = 0;
    for_each_event(&bytes, |_| count += 1);
    assert_eq!(count, 0);
}
