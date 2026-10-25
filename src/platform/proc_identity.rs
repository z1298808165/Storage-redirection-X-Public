// /proc 进程身份读取：cmdline 身份 + status 实际 uid。
//
// 独立成模块是因为两个 root 进程上下文需要同一语义：daemon 的系统 pid 表落盘
// （全量扫描）与自有 FUSE 引擎的系统请求者即时归因（单 pid 查询）。daemon.rs
// 原有实现语义保持不变，仅物理搬迁共享。

/// 读取单个进程的 (uid, 包名)。内核线程与无法提取可读名的进程返回 None。
///
/// argv0 带路径的本地二进制（如 adb shell 派生的 /system/bin/sh）取其
/// basename 作为身份：native 系统二进制没有包名形态，basename 是能给出的
/// 最诚实身份；Java 进程的 argv0 本就是包名，不受影响。status 的 Uid 行是
/// 身份可信的最终裁决（cmdline 可被进程自改，uid 不可）。
pub fn read_proc_identity(pid: i32) -> Option<(i32, String)> {
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    let first = cmdline.split('\0').next()?.trim().to_string();
    let mut package = first.split(':').next()?.trim().to_string();
    if let Some(basename) = package.rsplit('/').next() {
        package = basename.to_string();
    }
    if package.is_empty()
        || !package
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
    {
        return None;
    }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("Uid:") else {
            continue;
        };
        let uid = rest
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(-1);
        if uid < 0 {
            return None;
        }
        return Some((uid, package));
    }
    None
}
