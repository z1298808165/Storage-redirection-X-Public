#[allow(dead_code)] // quality-allow(lint-suppression): cdylib 目标不编译 daemon 调用方，但测试和 daemon 目标需要 wd 字段。
pub struct Event<'a> {
    pub wd: i32,
    pub mask: u32,
    pub name: &'a [u8],
}

/// 遍历一次 inotify read 缓冲区，统一处理事件头完整性、长度溢出和尾部截断。
pub fn for_each_event(mut buffer: &[u8], mut callback: impl FnMut(&Event<'_>)) {
    while let Some((header, payload)) = buffer.split_first_chunk::<16>() {
        let wd = i32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
        let mask = u32::from_ne_bytes([header[4], header[5], header[6], header[7]]);
        let length = u32::from_ne_bytes([header[12], header[13], header[14], header[15]]) as usize;
        let Some(name) = payload.get(..length) else {
            break;
        };
        callback(&Event { wd, mask, name });
        buffer = &payload[length..];
    }
}
