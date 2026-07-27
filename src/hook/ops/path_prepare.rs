use super::super::diagnostic;
use super::super::path as path_utils;
use super::super::stats::InterceptHub;
use libc::{c_char, c_int};
use std::ffi::CStr;

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
    if pathname.is_null() {
        return PreparedPath::Bypass;
    }
    // 绝大多数被拦截的 syscall 访问的是 /system、/apex、/data/user 等会立刻 bypass 的
    // 路径。to_string_lossy 对合法 UTF-8 返回借用形态，这样只有确认相关后才需要分配，
    // 而非每次 syscall 都先建一个 String 再判断是否要丢弃。
    // SAFETY: 调用方已保证 pathname 在本次拦截期间是有效的 NUL 结尾 C 字符串。
    let path_text = unsafe { CStr::from_ptr(pathname) }.to_string_lossy();
    if path_text.is_empty() {
        return PreparedPath::Bypass;
    }

    let is_relative = !path_text.starts_with('/');
    if is_relative {
        diagnostic::log_relative_path_bypass(hub, op_name, dirfd, &path_text, log_flags);
        let resolved = path_utils::resolve_path_for_dirfd(dirfd, &path_text);
        if resolved.is_empty() || !path_utils::is_normalized_relevant_storage_path(hub, &resolved) {
            return PreparedPath::Bypass;
        }
        return PreparedPath::Ready {
            path_for_decision: resolved,
            is_relative,
        };
    }

    if !path_utils::is_relevant_storage_path(hub, &path_text) {
        if record_fast_bypass {
            diagnostic::record_fast_bypass(op_name, &path_text);
        }
        return PreparedPath::Bypass;
    }

    PreparedPath::Ready {
        path_for_decision: path_text.into_owned(),
        is_relative,
    }
}
