//! 决策期「这个 pid 是谁」的同步反查通道。
//!
//! 记录侧的源头归因早就用同一份证据：内核 FUSE 请求头里的发起任务 id（`caller_pid`），
//! 由 root daemon 读 `/proc` 反查包名。决策侧却只能依赖 `tmp/system_pids` 快照——
//! 那份表由 daemon 周期性扫 `/proc` 生成（静息 10 秒一轮，见 `daemon::SystemPidScanner`），
//! 抓不住「刚启动就写盘」的进程，于是同一份证据在记录里能指出 `com.oplus.sau`、在决策里
//! 却是空，写入被放行到真实存储（2026-11-05 真机 issue #2）。
//!
//! 这里补的就是这条缺口：决策进程发现快照未命中时，向 daemon 发一次带 pid 与 uid 的
//! 询问，daemon 以 root 读 `/proc` 当场反查并回包。**不是按规则反推源头**——反查结果
//! 完全来自内核与 `/proc` 的客观事实，规则只用于判断该身份是否要重定向。
//!
//! 通道复用日志套接字（同一个抽象名字），前缀 `Q\t` 区分询问包，回包走发送方地址，
//! 因此不需要额外端口，也不与既有日志/控制包冲突。

use std::ffi::CString;
use std::mem;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::platform::paths;

/// 询问包前缀：`Q\t<pid>\t<uid>\t<nonce>`。
pub(crate) const QUERY_PREFIX: &str = "Q\t";

/// 回包前缀：`R\t<pid>\t<uid>\t<nonce>\t<包名或空>\t<证据>`。
pub(crate) const REPLY_PREFIX: &str = "R\t";
/// 与 daemon 共享的抽象套接字名（日志通道）。
const DAEMON_SOCKET_NAME: &[u8] = b"storage.redirect.x.logd";
/// 本进程询问用的抽象套接字名前缀。
const QUERY_SOCKET_PREFIX: &[u8] = b"storage.redirect.x.q.";
/// 单次询问的等待上限。
///
/// 取值依据：daemon 侧只做一次 `/proc/<pid>/cmdline` 读，正常在毫秒级；这个上限只是
/// 防止 daemon 卡住时把 FUSE 请求线程一起拖死。超时按「未解析」处理，与快照未命中
/// 同一条回退路径。
const QUERY_TIMEOUT_MS: i32 = 60;
/// 询问通道的客户端状态。daemon 未启动时 `fd` 为 -1，之后每次询问重试连接。
struct QueryClient {
    fd: i32,
    reply_addr: Option<(libc::sockaddr_un, libc::socklen_t)>,
}

// SAFETY: fd 是普通文件描述符，套接字读写都由调用方串行化（见下方 Mutex）。
unsafe impl Send for QueryClient {}

static CLIENT: OnceLock<Mutex<QueryClient>> = OnceLock::new();

fn client() -> &'static Mutex<QueryClient> {
    CLIENT.get_or_init(|| {
        Mutex::new(QueryClient {
            fd: -1,
            reply_addr: None,
        })
    })
}

/// 构造抽象命名空间地址。
fn abstract_addr(name: &[u8]) -> Option<(libc::sockaddr_un, libc::socklen_t)> {
    // SAFETY: sockaddr_un 是允许零初始化的普通 C 结构体。
    let mut addr: libc::sockaddr_un = unsafe { mem::zeroed() };
    addr.sun_family = libc::AF_UNIX as _;
    if name.len() + 1 > addr.sun_path.len() {
        return None;
    }
    // 首字节 0 表示抽象命名空间，名字从 sun_path[1] 开始。
    for (index, byte) in name.iter().enumerate() {
        addr.sun_path[index + 1] = *byte as _;
    }
    Some((
        addr,
        (mem::size_of::<libc::sa_family_t>() + name.len() + 1) as libc::socklen_t,
    ))
}

impl QueryClient {
    /// 懒建立询问套接字。失败返回 false，下次再试。
    fn ensure_ready(&mut self) -> bool {
        if self.fd >= 0 {
            return true;
        }
        // SAFETY: socket 不接收借用指针，成功时返回自有描述符。
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return false;
        }
        // 回包地址按进程唯一：多个进程同时询问时互不覆盖。
        let mut name = QUERY_SOCKET_PREFIX.to_vec();
        name.extend_from_slice(std::process::id().to_string().as_bytes());
        let Some((addr, addr_len)) = abstract_addr(&name) else {
            // SAFETY: fd 由当前代码持有，尚未交给其它任何所有权。
            unsafe { libc::close(fd) };
            return false;
        };
        // SAFETY: addr 已初始化，addr_len 只覆盖已初始化的地址字节。
        if unsafe { libc::bind(fd, &addr as *const _ as *const libc::sockaddr, addr_len) } != 0 {
            // SAFETY: 同上，bind 失败后 fd 仍由当前代码持有。
            unsafe { libc::close(fd) };
            return false;
        }
        self.fd = fd;
        self.reply_addr = Some((addr, addr_len));
        true
    }
}

/// 询问 daemon：`pid` 对应的进程身份是什么。
///
/// 返回 `Some((包名或进程名, uid))`；`None` 表示询问通道不可用或 daemon 当时也反查不到，
/// 调用方按既有回退处理（与快照未命中同义）。
pub(crate) fn query_process_identity(pid: i32, uid: i32) -> Option<(String, i32)> {
    if pid <= 0 {
        return None;
    }
    let mut guard = client().lock().unwrap_or_else(|err| err.into_inner());
    // pid/uid 快照不能辨别同 uid 的 pid 复用，且调用域可能无权读取 /proc starttime。
    // 每次由 daemon 实时核验当前任务身份，避免把旧 shared-UID 进程的包名当成当前身份。
    guard.round_trip(pid, uid)
}

impl QueryClient {
    fn drain_replies(&self) {
        let mut buffer = [0u8; 512];
        loop {
            // SAFETY: buffer 是可写区间，MSG_DONTWAIT 保证不会阻塞查询线程。
            let result = unsafe {
                libc::recv(
                    self.fd,
                    buffer.as_mut_ptr() as *mut _,
                    buffer.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if result <= 0 {
                break;
            }
        }
    }

    fn round_trip(&mut self, pid: i32, uid: i32) -> Option<(String, i32)> {
        if !self.ensure_ready() {
            return None;
        }
        let (daemon_addr, daemon_len) = abstract_addr(DAEMON_SOCKET_NAME)?;
        self.drain_replies();
        static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
        let nonce = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
        let Ok(payload) = CString::new(format!("{QUERY_PREFIX}{pid}\t{uid}\t{nonce}")) else {
            return None;
        };
        let bytes = payload.as_bytes();
        // SAFETY: payload 在调用期间有效，daemon_addr 由抽象命名空间地址构造。
        let sent = unsafe {
            libc::sendto(
                self.fd,
                bytes.as_ptr() as *const _,
                bytes.len(),
                0,
                &daemon_addr as *const _ as *const libc::sockaddr,
                daemon_len,
            )
        };
        if sent < 0 {
            // 发送失败通常是 daemon 尚未绑定套接字：丢掉 fd，下次重建。
            // SAFETY: fd 由本结构体独占持有。
            unsafe { libc::close(self.fd) };
            self.fd = -1;
            self.reply_addr = None;
            return None;
        }
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(QUERY_TIMEOUT_MS as u64);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                self.drain_replies();
                return None;
            }
            let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
            let mut poll_fd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll_fd 指向一个已初始化的 pollfd，超时受单次查询绝对截止点限制。
            let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
            if ready < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            if ready <= 0 || (poll_fd.revents & libc::POLLIN) == 0 {
                self.drain_replies();
                return None;
            }
            let mut buffer = [0u8; 512];
            // SAFETY: buffer 有 buffer.len() 字节可写，fd 由本结构体持有。
            let size =
                unsafe { libc::recv(self.fd, buffer.as_mut_ptr() as *mut _, buffer.len(), 0) };
            if size <= 0 {
                return None;
            }
            let Ok(text) = std::str::from_utf8(&buffer[..size as usize]) else {
                continue;
            };
            if let Some(reply) = parse_reply(text, pid, uid, nonce) {
                return Some(reply);
            }
            // 迟到的旧 nonce 回复只被丢弃；继续在原始 60ms 截止点内等待当前应答。
        }
    }
}

/// 解析回包：`R\t<pid>\t<uid>\t<nonce>\t<身份>\t<证据>`。
fn parse_reply(text: &str, pid: i32, uid: i32, nonce: u64) -> Option<(String, i32)> {
    let body = text.strip_prefix(REPLY_PREFIX)?;
    let mut fields = body.split('\t');
    let reply_pid = fields.next()?.trim().parse::<i32>().ok()?;
    let reply_uid = fields.next()?.trim().parse::<i32>().ok()?;
    let reply_nonce = fields.next()?.trim().parse::<u64>().ok()?;
    let identity = fields.next().unwrap_or("").trim();
    if reply_pid != pid || reply_uid != uid || reply_nonce != nonce || identity.is_empty() {
        return None;
    }
    Some((identity.to_string(), reply_uid))
}

/// 「有人问过身份」的需求信号（eventfd 写端）。
///
/// 只有 daemon 会设置它（hook 进程恒为 -1，写入即 no-op）。用途是把 daemon 侧
/// `tmp/system_pids` 的维护从「固定周期空转」改成「真有人问才刷」：一次询问就意味着
/// 表里缺了某个 pid，此刻刷新才有意义；没人问的时候刷新出来的内容也没人用。
static DEMAND_EVENTFD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

/// 由 daemon 在启动名册扫描线程前注入需求信号 fd。
pub(crate) fn set_demand_eventfd(fd: i32) {
    DEMAND_EVENTFD.store(fd, std::sync::atomic::Ordering::Release);
}

/// 通知名册扫描线程「刚才有人问过身份」。
///
/// 只是把一次刷新提前，绝不阻塞询问路径：eventfd 以非阻塞方式创建，写失败仅意味着
/// 这一次刷新退回固定周期。
fn note_identity_demand() {
    static LAST_DEMAND_MS: AtomicI64 = AtomicI64::new(i64::MIN / 2);
    const DEMAND_THROTTLE_MS: i64 = 10_000;
    let now_ms = paths::monotonic_ms();
    let last_ms = LAST_DEMAND_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last_ms) < DEMAND_THROTTLE_MS
        || LAST_DEMAND_MS
            .compare_exchange(last_ms, now_ms, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    let fd = DEMAND_EVENTFD.load(Ordering::Acquire);
    if fd < 0 {
        return;
    }
    let value: u64 = 1;
    // SAFETY: fd 由 daemon 以非阻塞 eventfd 创建，写 8 字节计数是 eventfd 契约。
    unsafe {
        libc::write(
            fd,
            &value as *const u64 as *const libc::c_void,
            std::mem::size_of::<u64>(),
        );
    }
}

/// daemon 侧：处理一条询问包，返回要发回调用方的文本。
///
/// 只在 daemon（root）里执行，能读到任意进程的 `/proc`，这正是决策进程自己做不到
/// 的部分（MediaProvider 域读 platform 应用的 `/proc/<pid>/cmdline` 被 SELinux 拒绝）。
pub(crate) fn answer_query(packet: &str) -> Option<String> {
    let body = packet.strip_prefix(QUERY_PREFIX)?;
    let mut fields = body.split('\t');
    let pid = fields.next()?.trim().parse::<i32>().ok()?;
    let requested_uid = fields.next().and_then(|v| v.trim().parse::<i32>().ok());
    let nonce = fields.next()?.trim().parse::<u64>().ok()?;
    if pid <= 0 {
        return None;
    }
    let ((name, uid), evidence) = resolve_identity(pid, requested_uid)?;
    // 需求信号有 10 秒节流，实时身份验证不会放大全量 /proc 重建。
    note_identity_demand();
    Some(format!(
        "{REPLY_PREFIX}{pid}\t{uid}\t{nonce}\t{name}\t{evidence}"
    ))
}

/// 反查结果：`((包名或进程名, uid), 证据名)`。
///
/// uid 必须与询问方的期望一致，否则视为身份不符、按未解析处理——错配的 uid 说明
/// pid 已经复用，此时任何答案都不可信。
fn resolve_identity(pid: i32, requested_uid: Option<i32>) -> Option<((String, i32), &'static str)> {
    let start_before = crate::platform::process_start_time_ticks(pid)?;
    let (proc_uid, name) = crate::platform::proc_identity::read_proc_identity(pid)?;
    if name.is_empty() || crate::platform::process_start_time_ticks(pid) != Some(start_before) {
        return None;
    }
    if let Some(expected) = requested_uid
        && expected >= 0
        && expected != proc_uid
    {
        return None;
    }
    // cmdline 不是包名形态（native helper、`sh`、`android.process.media` 之类）时按两条
    // 客观事实收敛，都不成立就保留 native 名——猜错比留空更糟：
    // 1. 内核进程组：应用派生的子进程继承 `uid_<uid>/pid_<发起应用>` 组，组首就是发起
    //    应用。这条对多包 uid 同样成立（issue #2 的 sau 属 uid 1000，53 个包）。
    // 2. PackageManager：该 uid 只对应一个安装包。
    let result = if !name.contains('.') {
        if let Some(package) =
            crate::platform::proc_identity::resolve_launching_app_by_process_group(pid, proc_uid)
        {
            ((package, proc_uid), "process_group")
        } else if let Some(package) = unique_package_for_uid(proc_uid) {
            ((package, proc_uid), "uid_unique_package")
        } else {
            ((name, proc_uid), "proc_reply")
        }
    } else {
        ((name, proc_uid), "proc_reply")
    };
    (crate::platform::process_start_time_ticks(pid) == Some(start_before)).then_some(result)
}

/// PackageManager 为 `uid` 返回的唯一包名。
///
/// 多包 uid 返回 `None`：这条判定的用途是「把 native 进程名换算成包名」，一旦 uid 下
/// 有多个包，任何挑选都是猜，宁可保留原始进程名让记录如实显示未知。
fn unique_package_for_uid(uid: i32) -> Option<String> {
    if uid < 0 {
        return None;
    }
    let packages = crate::platform::packages::packages_for_uid(uid).ok()?;
    if packages.len() != 1 {
        return None;
    }
    packages.into_iter().next()
}
