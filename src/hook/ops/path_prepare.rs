use super::super::diagnostic;
use super::super::path as path_utils;
use super::super::stats::InterceptHub;
use super::super::util::c_str_to_string;
use libc::{c_char, c_int};

// path_for_decision 一律持有解析后的独立字符串：相对路径需要拼接 dirfd 目标，
// 绝对路径也可能被规范化，都无法借用调用方传入的 C 字符串。用 String 而不是
// Cow<'a, str> 可避免出现不受任何入参约束的生命周期参数——那种签名允许后续改动
// 借用 pathname 指向的缓冲区而不被借用检查器拦截，在 hook 回调里就是释放后使用。
pub enum PreparedPath {
    Ready {
        path_for_decision: String,
        is_relative: bool,
    },
    Bypass,
}

pub unsafe fn prepare_relevant_path(
    hub: &InterceptHub,
    op_name: &str,
    dirfd: c_int,
    pathname: *const c_char,
    log_flags: i32,
    record_fast_bypass: bool,
) -> PreparedPath {
    let path_text = c_str_to_string(pathname);
    if path_text.is_empty() {
        return PreparedPath::Bypass;
    }

    let is_relative = !path_text.starts_with('/');
    let mut path_for_decision = path_text;
    if is_relative {
        diagnostic::log_relative_path_bypass(hub, op_name, dirfd, &path_for_decision, log_flags);
        let resolved = path_utils::resolve_path_for_dirfd(dirfd, &path_for_decision);
        if resolved.is_empty() || !path_utils::is_normalized_relevant_storage_path(hub, &resolved) {
            return PreparedPath::Bypass;
        }
        path_for_decision = resolved;
    } else if !path_utils::is_relevant_storage_path(hub, &path_for_decision) {
        if record_fast_bypass {
            diagnostic::record_fast_bypass(op_name, &path_for_decision);
        }
        return PreparedPath::Bypass;
    }

    PreparedPath::Ready {
        path_for_decision,
        is_relative,
    }
}
