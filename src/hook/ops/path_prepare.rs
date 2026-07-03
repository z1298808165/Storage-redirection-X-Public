use super::super::diagnostic;
use super::super::path as path_utils;
use super::super::stats::InterceptHub;
use super::super::util::c_str_to_string;
use libc::{c_char, c_int};
use std::borrow::Cow;

pub enum PreparedPath<'a> {
    Ready {
        path_for_decision: Cow<'a, str>,
        is_relative: bool,
    },
    Bypass,
}

pub unsafe fn prepare_relevant_path<'a>(
    hub: &InterceptHub,
    op_name: &str,
    dirfd: c_int,
    pathname: *const c_char,
    log_flags: i32,
    record_fast_bypass: bool,
) -> PreparedPath<'a> {
    let path_text = c_str_to_string(pathname);
    if path_text.is_empty() {
        return PreparedPath::Bypass;
    }

    let is_relative = !path_text.starts_with('/');
    let mut path_for_decision: Cow<'a, str> = Cow::Owned(path_text.clone());
    if is_relative {
        diagnostic::log_relative_path_bypass(hub, op_name, dirfd, &path_text, log_flags);
        let resolved = path_utils::resolve_path_for_dirfd(dirfd, &path_text);
        if resolved.is_empty() || !path_utils::is_relevant_storage_path(hub, &resolved) {
            return PreparedPath::Bypass;
        }
        path_for_decision = Cow::Owned(resolved);
    }

    if !path_utils::is_relevant_storage_path(hub, path_for_decision.as_ref()) {
        if record_fast_bypass {
            diagnostic::record_fast_bypass(op_name, path_for_decision.as_ref());
        }
        return PreparedPath::Bypass;
    }

    PreparedPath::Ready {
        path_for_decision,
        is_relative,
    }
}
