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
    let package = resolve_identity_package(&cmdline)?;
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

/// pid 表与系统请求者归因使用的身份串。
///
/// 解析顺序刻意收窄，避免把「命令行参数里恰好出现的包名」当成进程身份：
/// 1. argv0 是包名形态且能在 uid 表命中 —— 直接采用（绝大多数 Java 进程）；
/// 2. argv0 是包名形态但表里查不到 —— 按 `<package>.<suffix>` 截到真实父包。
///    HyperOS 上这类形态很常见（`com.miui.securitycenter.remote`），不截断就会
///    留下一个"查无此包"的名字：用它反查 uid、读应用配置全部落空，监视记录里也会
///    显示一个系统里并不存在的应用；
/// 3. argv0 不是包名形态（`sh`、`mtio` 这类 native/helper）—— 才去扫完整命令行里的
///    `@<package>-<version>` 形态（MT 管理器的 `mtio daemon @bin.mt.plus-3162069`）；
/// 4. 都解析不出就回退 argv0 的 basename：pid 表必须能给每个进程一个身份，这里的
///    诚实做法是进程名，而不是编造包名。
fn resolve_identity_package(cmdline: &str) -> Option<String> {
    let first = cmdline.split('\0').next()?.trim();
    let mut value = first.split(':').next()?.trim();
    if let Some(basename) = value.rsplit('/').next() {
        value = basename;
    }
    if is_package_shape(value) {
        if crate::redirect::policy::get_uid_for_package(value) >= 0 {
            return Some(value.to_string());
        }
        if let Some(parent) = truncate_to_known_package(value) {
            return Some(parent);
        }
        return Some(value.to_string());
    }
    if let Some(package) = parse_package_from_cmdline(cmdline) {
        return Some(package);
    }
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
    {
        return None;
    }
    Some(value.to_string())
}

/// 是否具备包名形态（含点、不以点开头/结尾、无连续点）。
fn is_package_shape(value: &str) -> bool {
    value.contains('.') && !value.starts_with('.') && !value.ends_with('.') && !value.contains("..")
}

/// 读取单个进程的「真实源头包名」，读不到返回 None。
///
/// 与 [`read_proc_identity`] 的区别：后者服务 pid 表（要求能给出 uid 与任意身份），
/// 本函数只回答「这个进程属于哪个安装包」，解析不出就返回 None。daemon 在把带
/// `caller_pid` 的监视记录落盘前用它对 root / 特权 helper 做反查——这些进程的
/// `/proc` 在 MediaProvider 域被 SELinux 拒绝，只有 root 侧读得到。
// quality-allow(lint-suppression): 引用方 log_daemon 仅由 daemon 二进制编译。
#[allow(dead_code)]
pub fn resolve_package_from_pid(pid: i32) -> Option<String> {
    if pid <= 0 {
        return None;
    }
    let cmdline = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).ok()?;
    parse_package_from_cmdline(&cmdline)
}

/// 从 cmdline 原始内容解析安装包名，解析不出返回 None。
///
/// 覆盖真机上出现的三类形态：
/// - Java 进程：argv0 本身就是包名（`com.tencent.mm`）；
/// - 分叉 helper：`<helper> daemon @<package>-<version>`（MT 管理器的
///   `mtio daemon @bin.mt.plus-3162069`）；
/// - 子进程名：`<package>:<suffix>`，以及 HyperOS 上大量存在的
///   `<package>.<suffix>`（`com.miui.securitycenter.remote`）。
///
/// 纪律：优先返回能在 uid 表命中的候选（那才是真实安装包）；其次是 `@<pkg>-<ver>`
/// 这种显式标了归属的候选。两者都没有就返回 None——宁可让上游降级成 basename 或
/// 未知，也不能把进程名当成包名写进监视记录。纯函数，不读 /proc，供 hook 侧
/// （只能走裸系统调用读文件）与 daemon 侧共用。
pub fn parse_package_from_cmdline(cmdline: &str) -> Option<String> {
    let mut helper_hit: Option<String> = None;
    for token in cmdline.split('\0') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        let from_helper = trimmed.starts_with('@');
        let candidate = normalize_package_token(trimmed);
        if candidate.is_empty() {
            continue;
        }
        if crate::redirect::policy::get_uid_for_package(&candidate) >= 0 {
            return Some(candidate);
        }
        if let Some(parent) = truncate_to_known_package(&candidate) {
            return Some(parent);
        }
        if from_helper && helper_hit.is_none() && candidate.matches('.').count() >= 2 {
            helper_hit = Some(candidate);
        }
    }
    helper_hit
}

/// 把单个 cmdline token 归一化成包名形态候选：去路径、去 `:` 后缀、去 `@` 前缀、
/// 去 `-<纯数字版本>` 后缀。不像包名时返回空串。
fn normalize_package_token(token: &str) -> String {
    let mut value = token.trim_start_matches('@');
    if let Some(basename) = value.rsplit('/').next() {
        value = basename;
    }
    value = value.split(':').next().unwrap_or("");
    if let Some(index) = value.rfind('-') {
        let suffix = &value[index + 1..];
        if index > 0 && !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            value = &value[..index];
        }
    }
    if value.is_empty()
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-')
    {
        return String::new();
    }
    if !value.contains('.')
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("..")
    {
        return String::new();
    }
    value.to_string()
}

/// 逐段截断到能在 uid 表命中的父包（`com.miui.securitycenter.remote` →
/// `com.miui.securitycenter`）。至少保留两段，避免把 `com.foo` 截成 `com`。
fn truncate_to_known_package(candidate: &str) -> Option<String> {
    let mut remaining = candidate;
    while let Some(index) = remaining.rfind('.') {
        remaining = &remaining[..index];
        if remaining.matches('.').count() < 1 {
            break;
        }
        if crate::redirect::policy::get_uid_for_package(remaining) >= 0 {
            return Some(remaining.to_string());
        }
    }
    None
}
