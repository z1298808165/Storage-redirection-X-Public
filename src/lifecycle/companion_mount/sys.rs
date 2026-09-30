use std::ffi::CString;

pub(super) use crate::platform::errno::{last as last_errno, text as errno_text};

pub(super) fn c_str(value: &str) -> Option<CString> {
    CString::new(value).ok()
}
