// 共享宿主会话的控制通道协议。
//
// 这里只负责 daemon 与宿主子进程之间那条 socketpair 上的一问一答：策略登记的编码、
// 应答解析、超时与 fd 生命周期。宿主进程本身的存在性判断与附着留在 fuse_host，
// 协议改动不会碰到会话生命周期。

//! 共享宿主 FUSE 会话（阶段 2 基础设施，B2-a/B2-b）。
//!
//! 目标是把「每应用各自 fork 一个 FUSE 服务」改成「daemon 持有一个持久共享会话，各应用
//! namespace 用 `MS_BIND` 引用」。B2-a 搭起宿主会话骨架，B2-b 实现应用侧接入逻辑，
//! 阶段 1 的按 uid 策略注册通过会话内的控制通道补齐。

use crate::fuse_host::record_registered_uid;
use std::sync::atomic::{AtomicI32, Ordering};

/// 单条策略载荷上限（一次 `send` 不超过它）。
const HOST_CONTROL_PAYLOAD_LIMIT: usize = 32 * 1024;
/// 控制线程栈大小：要容纳与载荷等长的栈缓冲，避免在 fork 之后为读缓冲分配堆内存。
const HOST_CONTROL_STACK_SIZE: usize = 256 * 1024;
/// 策略登记应答的等待上限（毫秒）。
///
/// 登记是接入的前置条件，必须先确认宿主会话真的建好了该 uid 的策略，否则应用会拿到
/// 「未登记即拒绝」的空视图。同一条控制通道被所有应用的挂载 worker 共用，应答因此带
/// uid 标记，读到别人的应答就继续等自己那条。
const HOST_POLICY_ACK_TIMEOUT_MS: i64 = 2_000;
/// 一次登记最多容忍几条不属于自己的应答，避免通道异常时无限空转。
const HOST_POLICY_ACK_SKIP_LIMIT: usize = 8;

/// 宿主会话控制端 fd（daemon 侧）；`-1` 表示当前没有可用宿主会话。
static HOST_CONTROL_FD: AtomicI32 = AtomicI32::new(-1);

/// 保存宿主会话控制端 fd，并关闭被替换掉的旧 fd（宿主重建时避免 fd 泄漏）。
pub(crate) fn set_host_control_fd(fd: libc::c_int) {
    // 登记要等待应答，因此接收超时必须先设好：挂载 worker 是 fork 出来的，超时设置会随
    // fd 一起继承，等真正登记时再设置就晚了。
    set_socket_recv_timeout_ms(fd, HOST_POLICY_ACK_TIMEOUT_MS);
    let previous = HOST_CONTROL_FD.swap(fd, Ordering::Relaxed);
    if previous >= 0 && previous != fd {
        // SAFETY: previous 是本模块此前保存的控制端 fd，本次替换负责关闭它。
        unsafe { libc::close(previous) };
    }
}

/// 设置 socket 接收超时，避免等待应答时无限阻塞。
fn set_socket_recv_timeout_ms(fd: libc::c_int, timeout_ms: i64) {
    let timeout = libc::timeval {
        tv_sec: timeout_ms / 1000,
        tv_usec: ((timeout_ms % 1000) * 1000) as libc::suseconds_t,
    };
    // SAFETY: timeout 是合法 timeval，fd 是本模块持有的 socket。
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &timeout as *const libc::timeval as *const libc::c_void,
            std::mem::size_of::<libc::timeval>() as libc::socklen_t,
        );
    }
}

/// 丢弃控制端 fd。宿主会话失效或重建失败时调用，避免后续注册请求写到已死的会话上。
pub(crate) fn close_host_control_fd() {
    let previous = HOST_CONTROL_FD.swap(-1, Ordering::Relaxed);
    if previous >= 0 {
        // SAFETY: previous 是本模块保存的控制端 fd，本次清理负责关闭它。
        unsafe { libc::close(previous) };
    }
}

/// 策略登记应答：`(uid, 表大小)`，表大小为 0 表示宿主会话拒绝了这份策略。
///
/// 应答必须带 uid：控制通道被所有应用的挂载 worker 共用（worker 是 daemon 的 fork，
/// 继承同一个 fd），并发登记时先到的应答未必属于本次调用。只按「收到一包就当成功」
/// 判断，会让某个应用在策略尚未登记的情况下接入共享会话。
fn decode_policy_ack(buffer: &[u8]) -> Option<(i32, i32)> {
    if buffer.len() < 8 {
        return None;
    }
    let uid = i32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]);
    let size = i32::from_ne_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]);
    Some((uid, size))
}

/// 等待宿主会话对本次登记的应答。
fn await_policy_ack(fd: libc::c_int, uid: i32) -> bool {
    let mut buffer = [0u8; 8];
    for _ in 0..HOST_POLICY_ACK_SKIP_LIMIT {
        // SAFETY: fd 是本模块持有的控制端 socket，buffer 是本地可写数组。
        let read = unsafe {
            libc::recv(
                fd,
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
                0,
            )
        };
        if read < 0 {
            let errno = crate::platform::errno::last();
            log::warn!(
                "fuse host policy ack unavailable uid={} errno={} {}",
                uid,
                errno,
                crate::platform::errno::text(errno)
            );
            return false;
        }
        let Some((ack_uid, table_size)) = decode_policy_ack(&buffer[..read as usize]) else {
            continue;
        };
        if ack_uid != uid {
            // 属于并发登记的另一次调用：不能代它消费，继续等自己那条。
            log::debug!(
                "fuse host policy ack for other uid={} expected={}",
                ack_uid,
                uid
            );
            continue;
        }
        if table_size <= 0 {
            log::warn!("fuse host policy ack rejected uid={}", uid);
            return false;
        }
        return true;
    }
    log::warn!("fuse host policy ack not received uid={}", uid);
    false
}

/// 把某个应用的策略登记到共享宿主会话（按其 uid 生效）。
///
/// 这是阶段 1「按 uid 策略注册」的 daemon 侧入口：策略在 daemon 侧构造（与 scoped 会话同一份
/// `fuse_config_from_request`），序列化后经控制通道送到持有会话的子进程再建策略。
///
/// 返回 `true` 表示**宿主会话已确认建好该 uid 的策略**（收到应答），而不是"数据已发出"：
/// 接入会让应用的存储根直接由宿主会话承载，策略没登记上去就是整片 ENOENT，因此这里必须
/// 拿应答而不是拿 `send` 的返回值。返回 `false` 时调用方保持 scoped 路径不变。
pub fn register_app_policy(config: &crate::fuse_redirect::FuseRedirectConfig) -> bool {
    let fd = HOST_CONTROL_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return false;
    }
    let Ok(payload) = serde_json::to_vec(config) else {
        log::warn!("fuse host policy encode failed pkg={}", config.package_name);
        return false;
    };
    if payload.len() > HOST_CONTROL_PAYLOAD_LIMIT {
        log::warn!(
            "fuse host policy payload too large pkg={} bytes={} limit={}",
            config.package_name,
            payload.len(),
            HOST_CONTROL_PAYLOAD_LIMIT
        );
        return false;
    }
    // SAFETY: fd 是本模块保存的控制端 socket，payload 在调用期间保持存活。
    let sent = unsafe {
        libc::send(
            fd,
            payload.as_ptr() as *const libc::c_void,
            payload.len(),
            0,
        )
    };
    if sent != payload.len() as isize {
        let errno = crate::platform::errno::last();
        log::warn!(
            "fuse host policy send failed uid={} sent={} bytes={} errno={} {}",
            config.uid,
            sent,
            payload.len(),
            errno,
            crate::platform::errno::text(errno)
        );
        return false;
    }
    if await_policy_ack(fd, config.uid) {
        // 登记结果必须同步进快照：companion 据此判断"我的 uid 已登记"才敢接入，
        // 否则未登记 uid 的请求会被宿主 fail-closed 拒绝（整片 ENOENT）。
        record_registered_uid(config.uid as u32);
        true
    } else {
        false
    }
}

/// 在宿主会话内启动按 uid 策略控制通道的读取循环。
///
/// 线程随会话存续：控制端被关闭（或会话结束）时 `recv` 返回 0，循环退出。读缓冲放在线程栈上，
/// 不在 fork 之后分配堆内存。
pub(crate) fn spawn_host_control_loop(
    control_sock: libc::c_int,
    policy_table: crate::fuse_redirect::SharedPolicyTable,
) {
    let spawned = std::thread::Builder::new()
        .name("srx_host_policy".to_string())
        .stack_size(HOST_CONTROL_STACK_SIZE)
        .spawn(move || {
            let mut buffer = [0u8; HOST_CONTROL_PAYLOAD_LIMIT];
            loop {
                // SAFETY: control_sock 是本次会话的控制端 fd，buffer 是本地可写数组。
                let read = unsafe {
                    libc::recv(
                        control_sock,
                        buffer.as_mut_ptr() as *mut libc::c_void,
                        buffer.len(),
                        0,
                    )
                };
                if read <= 0 {
                    break;
                }
                let payload = &buffer[..read as usize];
                match serde_json::from_slice::<crate::fuse_redirect::FuseRedirectConfig>(payload) {
                    Ok(config) => {
                        let uid = config.uid;
                        let package_name = config.package_name.clone();
                        let size = policy_table.register(config).unwrap_or(0);
                        if size > 0 {
                            log::info!(
                                "fuse host policy registered uid={} pkg={} table={}",
                                uid,
                                package_name,
                                size
                            );
                        } else {
                            log::warn!(
                                "fuse host policy rejected uid={} pkg={}",
                                uid,
                                package_name
                            );
                        }
                        // 应答必须发出去，否则调用方会一直等到超时再退回 scoped 路径；
                        // 表大小为 0 也是一种明确答复（拒绝），不能省略。
                        let mut ack = [0u8; 8];
                        ack[..4].copy_from_slice(&uid.to_ne_bytes());
                        ack[4..].copy_from_slice(&(size as i32).to_ne_bytes());
                        // SAFETY: control_sock 是本次会话的控制端 fd，ack 是本地数组。
                        let sent = unsafe {
                            libc::send(
                                control_sock,
                                ack.as_ptr() as *const libc::c_void,
                                ack.len(),
                                0,
                            )
                        };
                        if sent != ack.len() as isize {
                            let errno = crate::platform::errno::last();
                            log::warn!(
                                "fuse host policy ack send failed uid={} errno={} {}",
                                uid,
                                errno,
                                crate::platform::errno::text(errno)
                            );
                        }
                    }
                    Err(error) => log::warn!("fuse host policy decode failed err={}", error),
                }
            }
            log::info!("fuse host policy channel closed");
        });
    if spawned.is_err() {
        log::warn!("fuse host policy channel thread unavailable");
        // SAFETY: 线程创建失败时由本函数关闭控制端 fd，避免泄漏。
        unsafe { libc::close(control_sock) };
    }
}
