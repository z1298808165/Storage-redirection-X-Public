//! 通过 root daemon 核验原请求任务，供代写决策与监视共享身份语义。
//!
//! 日志套接字上的 Q/R 消息携带 UID、PID 与请求序号；回包保留执行者边界、任务启动
//! 时间、SELinux 域、具体包名及 PM 共享组标签。终止边界作为成功结果传递，查询失败
//! 不会降级为命令行参数解析。回复的 root 发送者凭据由内核校验。

use crate::platform::proc_identity::RequestIdentity;
use std::ffi::CString;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::platform::paths;

/// 询问包前缀：`Q\t<pid>\t<uid>\t<nonce>`。
pub(crate) const QUERY_PREFIX: &str = "Q\t";

/// 回包前缀：`R\t<pid>\t<uid>\t<nonce>\t<JSON 身份快照>`。
pub(crate) const REPLY_PREFIX: &str = "R\t";
/// 与 daemon 共享的抽象套接字名（日志通道）。
const DAEMON_SOCKET_NAME: &[u8] = b"storage.redirect.x.logd";
/// 单次询问的等待上限。
///
/// daemon 核验任务并按需查询 PM；等待受绝对截止点约束，这个上限用于
/// 防止 daemon 卡住时把 FUSE 请求线程一起拖死。超时按「未解析」处理，与快照未命中
/// 同一条回退路径。
const QUERY_TIMEOUT_MS: i32 = 60;
#[derive(Default)]
struct QueryClient {
    fd: Option<OwnedFd>,
    owner_pid: u32,
}

static CLIENT: OnceLock<Mutex<QueryClient>> = OnceLock::new();

fn client() -> &'static Mutex<QueryClient> {
    CLIENT.get_or_init(Mutex::default)
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
    fn socket(&mut self) -> Option<i32> {
        let owner = std::process::id();
        if self.owner_pid == owner
            && let Some(fd) = &self.fd
        {
            return Some(fd.as_raw_fd());
        }
        // SAFETY: socket 不接收借用指针，成功时返回自有描述符。
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return None;
        }
        // SAFETY: socket 已返回有效的新描述符，由此客户端接管所有权。
        let socket = unsafe { OwnedFd::from_raw_fd(fd) };
        let passcred: libc::c_int = 1;
        // SAFETY: 选项值与长度匹配；仅接受内核标记为 root 的 daemon 回包。
        if unsafe {
            libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PASSCRED,
                &passcred as *const _ as *const _,
                mem::size_of_val(&passcred) as _,
            )
        } != 0
        {
            return None;
        }
        // 启用 SO_PASSCRED 的未命名数据报套接字在发送时由内核自动绑定。
        // 并发请求通过各自的内核地址接收回复。
        self.owner_pid = owner;
        self.fd = Some(socket);
        Some(fd)
    }
}

/// 询问 daemon：`pid` 对应的进程身份是什么。
///
/// 返回经过任务凭据核验的身份快照；终止边界保留为成功结果，不能当作查询失败回退。
/// `None` 表示通道不可用或任务已失效，调用方保留原请求信息，不补造 App 来源。
pub(crate) fn query_process_identity(pid: i32, uid: i32) -> Option<RequestIdentity> {
    if pid <= 0 {
        return None;
    }
    let mut guard = match client().try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => {
            return QueryClient::default().round_trip(pid, uid);
        }
    };
    // pid/uid 快照不能辨别同 uid 的 pid 复用，且调用域可能无权读取 /proc starttime。
    // 每次由 daemon 实时核验当前任务身份，避免把旧 shared-UID 进程的包名当成当前身份。
    guard.round_trip(pid, uid)
}

impl QueryClient {
    fn drain_replies(fd: i32, deadline: std::time::Instant) {
        let mut buffer = [0u8; 2048];
        while std::time::Instant::now() < deadline {
            // SAFETY: buffer 是可写区间，MSG_DONTWAIT 保证不会阻塞查询线程。
            let result = unsafe {
                libc::recv(
                    fd,
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

    fn round_trip(&mut self, pid: i32, uid: i32) -> Option<RequestIdentity> {
        let fd = self.socket()?;
        let (daemon_addr, daemon_len) = abstract_addr(DAEMON_SOCKET_NAME)?;
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(QUERY_TIMEOUT_MS as u64);
        // 清理迟到回包也计入查询预算，持续输入不能延长 hook 线程的等待。
        Self::drain_replies(fd, deadline);
        if std::time::Instant::now() >= deadline {
            return None;
        }
        static NEXT_NONCE: AtomicU64 = AtomicU64::new(1);
        let nonce = NEXT_NONCE.fetch_add(1, Ordering::Relaxed);
        let Ok(payload) = CString::new(format!("{QUERY_PREFIX}{pid}\t{uid}\t{nonce}")) else {
            return None;
        };
        let bytes = payload.as_bytes();
        // SAFETY: payload 在调用期间有效，daemon_addr 由抽象命名空间地址构造。
        let sent = unsafe {
            libc::sendto(
                fd,
                bytes.as_ptr() as *const _,
                bytes.len(),
                libc::MSG_DONTWAIT,
                &daemon_addr as *const _ as *const libc::sockaddr,
                daemon_len,
            )
        };
        if sent < 0 {
            // 发送失败通常是 daemon 尚未绑定套接字：丢掉 fd，下次重建。
            self.fd = None;
            return None;
        }
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
            let mut poll_fd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll_fd 指向一个已初始化的 pollfd，超时受单次查询绝对截止点限制。
            let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
            if ready < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            if ready <= 0 || (poll_fd.revents & libc::POLLIN) == 0 {
                return None;
            }
            let mut buffer = [0u8; 2048];
            // SAFETY: buffer 有 buffer.len() 字节可写，fd 由本结构体持有。
            let Some(size) = recv_from_root(fd, &mut buffer) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&buffer[..size]) else {
                continue;
            };
            if let Some(reply) = parse_reply(text, pid, uid, nonce) {
                return Some(reply);
            }
            // 迟到的旧 nonce 回复只被丢弃；继续在原始 60ms 截止点内等待当前应答。
        }
    }
}

/// 回包序号只能排除迟到消息，发送者的 root 凭据另由内核校验。
fn recv_from_root(fd: i32, buffer: &mut [u8]) -> Option<usize> {
    #[repr(align(8))]
    struct Control([u8; 64]);
    let mut control = Control([0; 64]);
    let mut iov = libc::iovec {
        iov_base: buffer.as_mut_ptr().cast(),
        iov_len: buffer.len(),
    };
    // SAFETY: 以下 msg 的所有指针均指向本作用域内的可写缓冲。
    let mut msg: libc::msghdr = unsafe { mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.0.as_mut_ptr().cast();
    msg.msg_controllen = control.0.len();
    // fork 继承的读端可能在 poll 后取走回包，接收本身也必须保持非阻塞。
    // SAFETY: msg 的各缓冲仍有效，recvmsg 仅写入声明的容量。
    let size = unsafe { libc::recvmsg(fd, &mut msg, libc::MSG_DONTWAIT) };
    if size <= 0 || msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
        return None;
    }
    // SAFETY: msg 来自成功且未截断的 recvmsg，控制缓冲仍在本作用域内。
    let header = unsafe { libc::CMSG_FIRSTHDR(&msg) };
    if header.is_null() {
        return None;
    }
    // SAFETY: 已排除空指针，CMSG_FIRSTHDR 返回控制缓冲内的首个消息头。
    let header_ref = unsafe { &*header };
    if header_ref.cmsg_level != libc::SOL_SOCKET
        || header_ref.cmsg_type != libc::SCM_CREDENTIALS
        || header_ref.cmsg_len < mem::size_of::<libc::cmsghdr>() + mem::size_of::<libc::ucred>()
    {
        return None;
    }
    // SAFETY: 消息类型与长度已核验，Control 保证 ucred 所需的对齐。
    let credentials = unsafe { &*libc::CMSG_DATA(header).cast::<libc::ucred>() };
    (credentials.uid == 0).then_some(size as usize)
}

/// 校验请求序号与任务身份，拒绝旧回复、畸形快照及带包名的终止边界。
fn parse_reply(text: &str, pid: i32, uid: i32, nonce: u64) -> Option<RequestIdentity> {
    let body = text.strip_prefix(REPLY_PREFIX)?;
    let mut fields = body.splitn(4, '\t');
    let reply_pid = fields.next()?.parse::<i32>().ok()?;
    let reply_uid = fields.next()?.parse::<i32>().ok()?;
    let reply_nonce = fields.next()?.parse::<u64>().ok()?;
    if reply_pid != pid || reply_uid != uid || reply_nonce != nonce {
        return None;
    }
    let identity: RequestIdentity = serde_json::from_str(fields.next()?).ok()?;
    if identity.pid != pid
        || identity.uid != uid
        || identity.start_time == 0
        || identity.native.len() > 255
        || identity.selinux_context.len() > 512
        || identity.is_terminal()
            && (!identity.package.is_empty() || !identity.shared_user_id.is_empty())
    {
        return None;
    }
    Some(identity)
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

/// daemon 按发送方凭据核验查询权限，再读取目标任务；不可读取或核验失败时不回包。
pub(crate) fn answer_query(packet: &str, sender: Option<(u32, i32)>) -> Option<String> {
    let body = packet.strip_prefix(QUERY_PREFIX)?;
    let mut fields = body.split('\t');
    let pid = fields.next()?.trim().parse::<i32>().ok()?;
    let uid = fields.next()?.parse::<i32>().ok()?;
    let nonce = fields.next()?.parse::<u64>().ok()?;
    if pid <= 0 || uid < 0 || fields.next().is_some() {
        return None;
    }
    let (sender_uid, sender_pid) = sender?;
    if sender_uid != 0
        && i32::try_from(sender_uid).ok() != Some(uid)
        && !crate::platform::proc_identity::read_proc_identity_for_request(
            sender_pid,
            i32::try_from(sender_uid).ok()?,
        )
        .is_some_and(|identity| identity.is_system_writer())
    {
        return None;
    }
    let identity = crate::platform::proc_identity::read_proc_identity_for_request(pid, uid)?;
    note_identity_demand();
    let payload = serde_json::to_string(&identity).ok()?;
    Some(format!("{REPLY_PREFIX}{pid}\t{uid}\t{nonce}\t{payload}"))
}

#[cfg(test)]
#[path = "../tests/native-unit/identity_query.rs"]
mod tests;
