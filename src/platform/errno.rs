//! 统一的 errno 读取与错误文本转换。
//!
//! 之前各模块自带 `last_errno` / `errno_text` 副本，且都调用 `libc::strerror`。
//! `strerror` 返回进程内共享的静态缓冲区，POSIX 不保证线程安全；守护进程与 hook
//! 回调都是多线程的，并发告警可能打印出彼此错乱的错误文本，反而干扰诊断。
//! 这里统一改用 `strerror_r`，由调用方提供栈上缓冲区，各线程互不影响。

use std::ffi::CStr;

// Android bionic 的错误描述远短于此；留足余量后仍可完全放在栈上，
// 避免在错误路径上引入堆分配。
const ERROR_TEXT_BUFFER_LEN: usize = 256;

/// 读取当前线程的 errno。
pub fn last() -> i32 {
    // SAFETY: __errno 返回当前线程 errno 的有效指针，仅解引用读取一个 int。
    unsafe { *libc::__errno() }
}

/// 把 errno 转换为可读文本。
pub fn text(code: i32) -> String {
    let mut buffer = [0 as libc::c_char; ERROR_TEXT_BUFFER_LEN];
    // SAFETY: buffer 是本函数栈上的有效可写数组，长度按实际容量传入；
    // bionic 的 strerror_r 是 XSI 版本，返回 0 表示已写入 NUL 结尾字符串，
    // 失败时缓冲区内容不可信，改用数字兜底。
    let ret = unsafe { libc::strerror_r(code, buffer.as_mut_ptr(), buffer.len()) };
    if ret != 0 {
        return format!("errno {}", code);
    }
    // SAFETY: strerror_r 返回 0 时已在 buffer 内写入 NUL 结尾字符串。
    let text = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    text.to_string_lossy().to_string()
}
