use libc::c_int;
use std::ffi::CString;

pub(super) use crate::platform::errno::{last as last_errno, text as errno_text};

pub(super) fn c_str(value: &str) -> Option<CString> {
    CString::new(value).ok()
}

pub(super) fn decode_wait_status(status: c_int) -> String {
    let signal = status & 0x7f;
    if signal == 0 {
        let exit_code = (status >> 8) & 0xff;
        return format!("exit={}", exit_code);
    }
    if signal == 0x7f {
        let stop_signal = (status >> 8) & 0xff;
        return format!("stop sig={}", stop_signal);
    }
    let is_core_dump = (status & 0x80) != 0;
    format!("sig={} core={}", signal, is_core_dump)
}
