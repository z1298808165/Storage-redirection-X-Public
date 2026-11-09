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

use crate::fuse_host::record_registered_policy;
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
/// 等待应答的总预算上限（毫秒），等于「单轮超时 × 可容忍条数」的最坏值。
///
/// 控制通道是 daemon 与所有挂载 worker 共用的同一条 socket：并发登记时先到的应答可能
/// 属于别的调用方，本方只能继续等自己那条。没有预算约束时，最坏情况会阻塞
/// `HOST_POLICY_ACK_TIMEOUT_MS × HOST_POLICY_ACK_SKIP_LIMIT`，而挂载 worker 的父进程只等
/// 5 秒（外加每个挂载根 4 秒）——子进程会在等应答时被父进程超时判定卡死并 SIGKILL，
/// 这一轮挂载彻底失败，只能靠后续 reconcile 重挂。调用方必须按自己还能等多久给预算。
pub(crate) const HOST_POLICY_ACK_BUDGET_MAX_MS: i64 =
    HOST_POLICY_ACK_TIMEOUT_MS * HOST_POLICY_ACK_SKIP_LIMIT as i64;

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

/// 等待宿主会话对本次登记的应答，总耗时不超过 `budget_ms`。
///
/// `budget_ms` 由调用方按「自己还能等多久」给出：挂载 worker 用父进程等待预算的剩余量，
/// 常驻 daemon 用 [`HOST_POLICY_ACK_BUDGET_MAX_MS`]。预算不足一整轮时**不再直接放弃**，
/// 而是把本轮接收超时收敛到剩余时间：宿主侧是同一会话里的阻塞 `recv` + 立刻回包，
/// 正常应答是毫秒级，历史实现却要求「剩余 ≥ 一轮超时」才肯尝试，于是「预算紧一点、
/// 但完全来得及」的轮次被判成登记失败并整轮回退 namespace bind。
/// 真机实证（2026-11-09，Android 14 CI 腿）：热重载在清理旧层之后才开始接入，剩余预算
/// 落在 4–6 秒区间时必然命中该分支，`policy_registration_failed` 会让本该由宿主服务的
/// 视图降级成 bind，应用随即看到「根 FUSE + Android/data 系统视图 bind」的混合视图。
/// 预算真的耗尽（剩余 ≤ 0）时才放弃，保证父进程仍能拿到确定结果。
fn await_policy_ack(fd: libc::c_int, uid: i32, budget_ms: i64) -> bool {
    let mut buffer = [0u8; 8];
    let deadline_ms = crate::platform::paths::monotonic_ms().saturating_add(budget_ms.max(0));
    for _ in 0..HOST_POLICY_ACK_SKIP_LIMIT {
        let remaining_ms = deadline_ms.saturating_sub(crate::platform::paths::monotonic_ms());
        if remaining_ms <= 0 {
            log::warn!(
                "fuse host policy ack budget exhausted uid={} budget_ms={} remaining_ms={}",
                uid,
                budget_ms,
                remaining_ms
            );
            return false;
        }
        // 本轮最多等一轮完整超时，预算不够就按剩余时间收，避免「来不及整轮」等于「不试」。
        set_socket_recv_timeout_ms(fd, remaining_ms.min(HOST_POLICY_ACK_TIMEOUT_MS));
        // SAFETY: fd 是本模块持有的控制端 socket，buffer 是本地可写数组。
        let read = unsafe {
            libc::recv(
                fd,
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
                0,
            )
        };
        // 下一轮（含继续等自己的应答、跳过他人应答）恢复正常超时，避免被上一轮的
        // 收敛值长期粘住。
        set_socket_recv_timeout_ms(fd, HOST_POLICY_ACK_TIMEOUT_MS);
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
            // 属于并发登记的另一次调用：不能代它消费，继续等自己那条。数据报已被本次
            // recv 取走，对方只能再等一轮；留下告警让这种串扰在日志里可观测。
            log::warn!(
                "fuse host policy ack for other uid={} expected={} remaining_ms={}",
                ack_uid,
                uid,
                deadline_ms.saturating_sub(crate::platform::paths::monotonic_ms())
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
/// 通知宿主子进程摘除传播副本（attach 成功、传播已发生之后调用）。
///
/// fire-and-forget：副本摘除只是挂载表卫生（数据面已由 f2fs 直连锚点解耦），
/// 通知失败不影响接入流程；宿主收到后在 control 线程内同步执行摘除。
// quality-allow(lint-suppression): 调用方在 bin 侧 daemon_mount（接入完成时通知宿主），lib 视角无调用者；与控制协议 'U' 消息配对不拆分。
#[allow(dead_code)]
pub(crate) fn notify_host_unpropagate() {
    let fd = HOST_CONTROL_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    // SAFETY: fd 是本模块保存的控制端 socket，字面量在调用期间保持存活。
    let sent = unsafe {
        libc::send(
            fd,
            b"U
"
            .as_ptr() as *const libc::c_void,
            2,
            0,
        )
    };
    if sent != 2 {
        log::warn!("fuse host unpropagate notify failed sent={sent}");
    }
}

/// 把详细日志开关同步给宿主会话子进程。
///
/// 会话子进程在 fork 时继承当时的 `DEBUG_LOGGING_ENABLED`，运行期再改全局配置不会
/// 传到它：用户在设置页打开「详细日志」后复现问题，会话侧的目录扫描/缓存指标与
/// `perf_snapshot` 仍然一条不落，日志包里看不到媒体读取链路的任何耗时证据。
/// 这里用与 'U' 相同的一次性控制消息（`V0`/`V1`）在开关变化时同步过去。
///
/// fire-and-forget：日志开关不参与挂载或策略判定，通知失败只影响取证粒度。
pub(crate) fn notify_host_verbose(enabled: bool) {
    let fd = HOST_CONTROL_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    let payload: [u8; 2] = [b'V', if enabled { b'1' } else { b'0' }];
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
        log::warn!("fuse host verbose notify failed sent={sent}");
    }
}

/// 构造策略消息、发给宿主会话并等待应答。
///
/// 返回 `true` 表示宿主会话已按该消息建好策略（收到应答），而不是"数据已发出"。
fn send_policy_config(
    config: &crate::fuse_redirect::FuseRedirectConfig,
    ack_budget_ms: i64,
) -> bool {
    let fd = HOST_CONTROL_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return false;
    }
    // 登记本身只发一条控制消息并等应答，不再在调用方进程内做任何挂载/绑定的准备工作：
    // 策略构造只读配置与账本，MediaProvider 视图按「用到时临时克隆」在自愈现场建立
    // （见 `fuse_redirect::media_view`），因此这里不存在「绑定耗时吃掉应答预算」的问题，
    // 预算全额留给应答等待。
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
    await_policy_ack(fd, config.uid, ack_budget_ms)
}

pub fn register_app_policy(
    config: &crate::fuse_redirect::FuseRedirectConfig,
    policy_fingerprint: u64,
    ack_budget_ms: i64,
) -> bool {
    if !send_policy_config(config, ack_budget_ms) {
        return false;
    }
    // 登记结果必须同步进快照：companion 据此判断"我的 uid 已登记"才敢接入，
    // 否则未登记 uid 的请求会被宿主 fail-closed 拒绝（整片 ENOENT）。
    record_registered_policy(config.uid as u32, policy_fingerprint);
    true
}

/// 只登记应用 mount namespace 维度的策略，不写按 uid 表。
///
/// 命名空间策略按**进程实例**绑定（namespace dev+ino），而按 uid 的幂等指纹刻意不含
/// 应用 pid：应用重启后 namespace 是一个新 inode，旧条目对新进程无效，但按 uid 的
/// 指纹仍然命中，于是「已登记」的结论把这个变化盖住了。此后该 namespace 里的 root
/// 调用方（挂载子进程、MediaProvider 的 root 线程）请求会落进 namespace miss +
/// deny_all 回退，表现为整片 ENOENT——自有私有目录恢复窗口的失败正是这个形态。
/// 因此调用方在按 uid 的幂等判断**之前**先用这里补登一次 namespace。
///
/// 不更新宿主快照：本次只写了 namespace 表，按 uid 的就绪状态未变；若据此标记
/// 已登记，companion 会在 uid 表仍为空时接入（整片 ENOENT）。
// quality-allow(lint-suppression): 调用方在 bin 侧 daemon_mount（预登记前补登 namespace），
// lib 视角无调用者；与 `register_app_policy` 共用同一条策略消息，不拆分。
#[allow(dead_code)]
pub fn register_app_namespace_only(
    config: &crate::fuse_redirect::FuseRedirectConfig,
    ack_budget_ms: i64,
) -> bool {
    let mut config = config.clone();
    config.namespace_policy_only = true;
    send_policy_config(&config, ack_budget_ms)
}

/// 把 daemon 自身的 mount namespace 登记为共享宿主的直通策略（仅 namespace 维度）。
///
/// daemon 进程内的 root FUSE 请求（文件监视、挂载子进程的探测与 mkdir）以 uid=0 到达
/// 共享宿主；宿主按调用方 namespace 精确回查后放行直通。未登记时这些请求持续落入
/// namespace miss + deny_all 回退（真机实测稳态 ~10 req/s 的空转流量与计数告警）。
/// 登记只写 namespace 表：uid=0 绝不进按 uid 表，否则任何 namespace 的 root 进程都会
/// 命中这份策略，破坏「未登记 uid 一律 deny_all」的 fail-closed 边界。
/// 每代宿主调用一次即可（宿主重建后由 [`crate::fuse_host::set_global`] 重新触发）。
///
/// 返回 `true` 表示宿主已确认登记。
pub fn register_daemon_namespace_passthrough() -> bool {
    let fd = HOST_CONTROL_FD.load(Ordering::Relaxed);
    if fd < 0 {
        return false;
    }
    let daemon_pid = std::process::id() as i32;
    let Some(start_ticks) = crate::platform::process_start_time_ticks(daemon_pid) else {
        log::warn!(
            "daemon namespace passthrough start ticks unavailable pid={}",
            daemon_pid
        );
        return false;
    };
    let user_id = 0;
    let storage_root = crate::platform::paths::storage_user_root_for_user(user_id);
    let config = crate::fuse_redirect::FuseRedirectConfig {
        package_name: "srx_daemon".to_string(),
        app_pid: daemon_pid,
        app_start_time_ticks: Some(start_ticks),
        uid: 0,
        app_data_dir: String::new(),
        // 直通会话：重定向根就是存储根本身，策略侧不做任何重定向、不创建目录。
        redirect_target: storage_root.clone(),
        mount_root: Some(storage_root.clone()),
        real_root_override: None,
        is_file_monitor_enabled: false,
        allowed_real_paths: Vec::new(),
        excluded_real_paths: Vec::new(),
        sandboxed_paths: Vec::new(),
        read_only_paths: Vec::new(),
        path_mappings: Vec::new(),
        is_mapping_mode_only: false,
        is_monitor_only: false,
        is_passthrough_host: true,
        namespace_policy_only: true,
    };
    // daemon 常驻路径，不在挂载 worker 的父进程等待预算内，用最坏值即可。
    send_policy_config(&config, HOST_POLICY_ACK_BUDGET_MAX_MS)
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
                // 'U' 前缀是 daemon 的传播副本摘除指令（attach 成功后触发，fire-
                // and-forget）；'{' 开头是既有的策略登记 JSON。宿主与 daemon 同包
                // 部署，无跨版本兼容负担。
                if payload.first() == Some(&b'U') {
                    crate::fuse_host::unpropagate_media_realm();
                    continue;
                }
                // 'V' 前缀是详细日志开关同步（见 notify_host_verbose）：会话子进程
                // 继承的是 fork 时刻的取值，运行期只靠这条消息才能跟上设置页的开关。
                if payload.first() == Some(&b'V') {
                    let enabled = payload.get(1) == Some(&b'1');
                    crate::logging::set_debug_logging_enabled(enabled);
                    // 用 warn 记录：关闭后 info/debug 会被丢弃，这条是开关确实生效的唯一凭据。
                    log::warn!("fuse host verbose updated enabled={}", enabled);
                    continue;
                }
                match serde_json::from_slice::<crate::fuse_redirect::FuseRedirectConfig>(payload) {
                    Ok(config) => {
                        let uid = config.uid;
                        let package_name = config.package_name.clone();
                        let namespace_only = config.namespace_policy_only;
                        let content_fp =
                            crate::fuse_redirect::config::fuse_config_policy_fingerprint(&config);
                        let size = if namespace_only {
                            // daemon 自身（uid=0）的直通登记只进 namespace 表；
                            // uid=0 绝不允许进按 uid 表（会盖住所有 root 调用方）。
                            policy_table.register_namespace_only(config).unwrap_or(0)
                        } else {
                            policy_table.register(config).unwrap_or(0)
                        };
                        if size > 0 {
                            log::info!(
                                "fuse host policy registered uid={} pkg={} table={} ns_only={} content_fp={:016x}",
                                uid,
                                package_name,
                                size,
                                namespace_only,
                                content_fp
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
