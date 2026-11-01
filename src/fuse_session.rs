//! FUSE 会话状态与挂载子进程 IPC 的共享定义。
//!
//! daemon 侧（`daemon_mount`）与 companion 侧（`lifecycle::companion_mount`）共用：
//! 两条挂载路径写同一份会话语义、走同一个 socket 协议收发挂载结果。历史上两处
//! 各写一份已发生过行为分叉，因此这里收敛为单一实现，两侧只保留各自的请求建模
//! 与宿主会话发现方式（daemon 进程内持有、companion 经快照发现）。

use crate::fuse_redirect::MountRequestFields;
use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::fs;
use crate::platform::module_paths;
use libc::{
    O_CLOEXEC, O_CREAT, O_TRUNC, O_WRONLY, SO_RCVTIMEO, SOL_SOCKET, c_int, c_void, open, recv,
    send, setsockopt,
};
use std::ffi::CString;

/// 一次 scoped FUSE 挂载的会话记录。
#[derive(Clone)]
pub(crate) struct FuseMountState {
    pub(crate) target: String,
    /// 服务该挂载的 scoped FUSE 子进程；接入共享宿主会话时为 0（见 `host_session`）。
    pub(crate) child: i32,
    pub(crate) child_start_time_ticks: u64,
    /// 该挂载由共享宿主会话承载时记录 `(pid, start_time_ticks)`。
    ///
    /// 宿主会话是**跨应用共享**的：终止它会打掉所有接入该会话的应用挂载，因此
    /// 状态文件、回滚与清理都必须按这个字段把宿主挂载与 scoped 会话分流。
    pub(crate) host_session: Option<(i32, u64)>,
}

/// 回滚已建立的 scoped FUSE 会话：先卸载挂载点，再按台账字段分流终止子进程。
///
/// 顺序契约：卸载在前、终止在后——先终止子进程会让挂载点进入死挂载状态，卸载
/// 仍可进行但取证信息变少；宿主会话只卸载不终止（见 `host_session` 文档）。
pub(crate) fn rollback_scoped_fuse_services(states: &[FuseMountState]) {
    for state in states.iter().rev() {
        if let Ok(c_target) = std::ffi::CString::new(state.target.as_str()) {
            // SAFETY: c_target 是以 NUL 结尾的合法路径，且在本次调用期间保持存活。
            if unsafe { libc::umount2(c_target.as_ptr(), libc::MNT_DETACH) } != 0 {
                let errno = last_errno();
                if errno != libc::EINVAL && errno != libc::ENOENT {
                    log::warn!(
                        "fuse rollback umount failed target={} errno={} {}",
                        state.target,
                        errno,
                        errno_text(errno)
                    );
                }
            }
        }
        if let Some((host_pid, _)) = state.host_session {
            log::info!(
                "fuse rollback keeps shared host session target={} host={}",
                state.target,
                host_pid
            );
            continue;
        }
        crate::fuse_terminate::terminate_fuse_process(
            state.child,
            (state.child_start_time_ticks != 0).then_some(state.child_start_time_ticks),
        );
    }
}

/// 把本次挂载写入状态文件（原子写：临时文件 + fsync + rename）。
///
/// 状态行契约：`version=` 与 `fingerprint=`（指纹跨进程可比、version 计数器两侧不同域）、
/// `fuse_child=` 只记可终止的 scoped 会话，共享宿主会话单独写 `fuse_host=`（判活用、
/// 不可终止），`target=` 记挂载目标。写入中途失败保留上一份完整状态，避免清理流程
/// 读到截断文件而永久漏卸挂载点。
// quality-allow(lint-suppression): 参数即两侧共用的状态写入契约，包一层结构体只会增加一次性装配对象。
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_mount_state(
    pid: i32,
    uid: i32,
    package_name: &str,
    config_version: u64,
    policy_fingerprint: u64,
    state_path: &str,
    temp_path: &str,
    targets: &[String],
    fuse_children: &[FuseMountState],
) -> bool {
    if pid <= 0 || package_name.is_empty() {
        return false;
    }
    if std::fs::create_dir_all(module_paths::MOUNT_STATE_DIR).is_err() {
        log::warn!(
            "mount state mkdir failed dir={}",
            module_paths::MOUNT_STATE_DIR
        );
        return false;
    }
    // 路径在 fork 之前已由两侧的 ForkPlan 算好，这里直接复用，避免子进程堆分配。
    let Ok(c_temp_path) = CString::new(temp_path) else {
        return false;
    };
    let mut content = String::new();
    content.push_str(&format!("version={}\n", config_version));
    content.push_str(&format!(
        "fingerprint={}\n",
        crate::config::SettingsHub::instance().config_fingerprint()
    ));
    // 包级策略指纹：只随本应用的规则与后端模式变化。幂等判定用它而不是全局目录指纹——
    // 全局指纹覆盖整个配置目录，任何单个应用配置新增/变更都会让**所有**应用的状态失效，
    // 触发全量重挂扫（真机实测：单个新应用被自动启用 → 59 应用全部重挂一轮，反复发生）。
    // 该指纹由 request_policy_fingerprint 按请求内容算出，daemon 与 companion 对同一份
    // 配置得到同一个值，跨进程可比性不变。
    content.push_str(&format!("policy_fp={}\n", policy_fingerprint));
    content.push_str(&format!("package={}\n", package_name));
    content.push_str(&format!("uid={}\n", uid));
    if let Some(start_time_ticks) = crate::platform::process_start_time_ticks(pid) {
        content.push_str(&format!("app_start_time={}\n", start_time_ticks));
    }
    for state in fuse_children {
        if state.host_session.is_some() {
            // 共享宿主会话不能被写进 `fuse_child=`：清理流程会按这一行终止进程，
            // 而宿主会话承载着所有接入应用的挂载。
            continue;
        }
        content.push_str(&format!(
            "fuse_child={}:{}\n",
            state.child, state.child_start_time_ticks
        ));
    }
    // 宿主会话单独记一行：它不参与终止，但会话死亡后本应用的挂载会变成 ENOTCONN 死挂载，
    // 必须让健康判定能据此把这份状态视为失效并重挂。
    if let Some((host_pid, host_start)) = fuse_children.iter().find_map(|state| state.host_session)
    {
        content.push_str(&format!("fuse_host={}:{}\n", host_pid, host_start));
    }
    let mut all_targets = targets.to_vec();
    all_targets.extend(fuse_children.iter().map(|state| state.target.clone()));
    for target in module_paths::normalize_mount_targets(&all_targets) {
        content.push_str("target=");
        content.push_str(&target);
        content.push('\n');
    }
    // SAFETY: c_temp_path 在调用期间保持存活，且是以 NUL 结尾的合法路径。
    let fd = unsafe {
        open(
            c_temp_path.as_ptr(),
            O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        let errno = last_errno();
        log::warn!(
            "mount state open failed path={} errno={} {}",
            temp_path,
            errno,
            errno_text(errno)
        );
        return false;
    }
    let mut ok = fs::write_all(fd, content.as_bytes());
    // SAFETY: fd 为本函数打开且尚未关闭的有效描述符。
    if ok && unsafe { libc::fsync(fd) } != 0 {
        let errno = last_errno();
        log::warn!(
            "mount state fsync failed path={} errno={} {}",
            temp_path,
            errno,
            errno_text(errno)
        );
        ok = false;
    }
    // SAFETY: 同上，关闭与改权限使用的都是本函数持有的 fd 与存活字符串。
    unsafe {
        libc::close(fd);
        let _ = libc::chmod(c_temp_path.as_ptr(), 0o600);
    }
    if ok {
        ok = std::fs::rename(temp_path, state_path).is_ok();
        if !ok {
            log::warn!(
                "mount state rename failed temp={} path={}",
                temp_path,
                state_path
            );
        }
    }
    if ok {
        log::info!(
            "mount state saved pid={} targets={} path={}",
            pid,
            targets.len(),
            state_path
        );
    } else {
        let _ = std::fs::remove_file(temp_path);
    }
    ok
}

/// FUSE 视图没能建立起来的**具体原因**，用于现场诊断「为什么这个应用没有 FUSE」。
///
/// 每条原因都对应一个可以独立验证的事实（会话不存在 / 闸门不许 / 策略没登记 /
/// 无规则 / 接入失败），而不是一句笼统的 "unavailable"。用户设备上出现不支持时，
/// 只凭日志就能判断是设备侧、配置侧还是模块侧的问题。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HostSessionSkip {
    /// 共享宿主会话不存在或已消失（设备不支持 / 宿主启动失败 / 尚未建立）。
    NoSharedHost,
    /// 宿主会话只服务整个存储视图根，该根不满足接入条件。
    AttachGateClosed,
    /// 策略没有登记进宿主会话（daemon 侧登记失败或 fingerprint 不匹配）。
    PolicyRegistrationFailed,
    /// 请求没有任何策略规则，接入宿主只会得到 deny_all 空视图。
    NoPolicyRules,
    /// 登记与闸门都通过，但实际接入失败（超时、errno、后端不可读）。
    AttachFailed,
}

impl HostSessionSkip {
    /// 稳定短名：写进日志与能力快照，供现场与 issue 直接检索。
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoSharedHost => "no_shared_host_session",
            Self::AttachGateClosed => "attach_gate_closed",
            Self::PolicyRegistrationFailed => "policy_registration_failed",
            Self::NoPolicyRules => "no_policy_rules",
            Self::AttachFailed => "host_attach_failed",
        }
    }
}

/// 单根接入的落地结果。
pub(crate) enum HostSessionAttach {
    /// 本次新建了一条宿主挂载。
    Mounted(FuseMountState),
    /// 该根已经被本模块的宿主层服务（例如 daemon 的 reconcile 已先挂好）：本轮不需要
    /// 再动作，但也不能当成失败去重新 bind——那会在已有视图之上再叠一层。
    AlreadyServed,
}

/// 把这些根接入**共享宿主会话**，返回本次接入成功的挂载。
///
/// 这里只做接入，不再 fork per-app scoped 会话。历史实现会在接入失败时 fork 一个
/// 常驻子进程进应用命名空间，而那个子进程本身就是应用命名空间的自持引用：应用退出后
/// 命名空间仍在、它服务的 FUSE 挂载不消失、连接不断开，于是它永远等不到自己的退出
/// 条件（issue #4 真机上累积到 1198 个常驻会话，把 pid 号与线程数顶满，最终触发
/// framework 重启）。把生命周期绑在应用上的进程形态无法靠清理补救，只能不要它：
/// 宿主会话不跟随任何应用生命周期，接入失败就老实降级到 namespace bind。
///
/// 返回 `Err(reason)` 时调用方必须按 namespace bind 继续（规划阶段已不再为这些根
/// 预留 FUSE 覆盖，因此 bind 是完整的、可用的视图）。
pub(crate) fn attach_shared_host_sessions(
    request: &(impl MountRequestFields + ?Sized),
    roots: &[String],
    attach_one: impl Fn(&str) -> Result<HostSessionAttach, HostSessionSkip>,
) -> Result<Vec<FuseMountState>, HostSessionSkip> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let mut states = Vec::with_capacity(roots.len());
    let mut attached_any = false;
    let mut last_skip = HostSessionSkip::NoSharedHost;
    for root in roots {
        match attach_one(root) {
            Ok(HostSessionAttach::Mounted(state)) => {
                attached_any = true;
                states.push(state);
            }
            Ok(HostSessionAttach::AlreadyServed) => {
                attached_any = true;
            }
            Err(reason) => {
                last_skip = reason;
                log::warn!(
                    "fuse host session attach skipped pkg={} pid={} root={} reason={}",
                    request.package_name(),
                    request.pid(),
                    root,
                    reason.as_str()
                );
            }
        }
    }
    if !attached_any {
        return Err(last_skip);
    }
    Ok(states)
}

/// 把 waitpid 的状态位解码为可读文本（exit=/stop sig=/sig= core=）。
pub(crate) fn decode_wait_status(status: c_int) -> String {
    let signal = status & 0x7f;
    if signal == 0 {
        let exit_code = (status >> 8) & 0xff;
        return format!("exit={}", exit_code);
    }
    if signal == 0x7f {
        let stop_signal = (status >> 8) & 0xff;
        return format!("stop sig={}", stop_signal);
    }
    let is_core_dump = (status & 0x80) != 0;
    format!("sig={} core={}", signal, is_core_dump)
}

/// 给挂载子进程的结果 socket 设置接收超时；失败记告警（超时不生效会让父进程永久阻塞）。
pub(crate) fn set_recv_timeout(sock: c_int, child: i32, seconds: i64) {
    let tv = libc::timeval {
        tv_sec: seconds,
        tv_usec: 0,
    };
    // SAFETY: tv 是栈上有效的 timeval，选项长度与其类型完全一致。
    let opt_ret = unsafe {
        setsockopt(
            sock,
            SOL_SOCKET,
            SO_RCVTIMEO,
            &tv as *const _ as *const c_void,
            std::mem::size_of::<libc::timeval>() as u32,
        )
    };
    if opt_ret != 0 {
        let errno = last_errno();
        log::warn!(
            "setsockopt failed child={} sec={} errno={} {}",
            child,
            seconds,
            errno,
            errno_text(errno)
        );
    }
}

/// 从结果 socket 读取一个 i32 结果。
pub(crate) fn recv_result(sock: c_int, result: &mut i32) -> isize {
    // SAFETY: result 指向有效的 i32 存储，recv 只写入不借用。
    unsafe {
        recv(
            sock,
            result as *mut _ as *mut c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    }
}

/// 把挂载结果写回父进程；失败（errno 或短写）记告警并返回 false，成功记 debug。
pub(crate) fn send_mount_result(sock: c_int, result: i32) -> bool {
    let expected_size = std::mem::size_of::<i32>() as isize;
    // SAFETY: send 只接收整型参数与栈上缓冲指针，不涉及借用指针。
    let sent = unsafe {
        send(
            sock,
            &result as *const _ as *const c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    if sent != expected_size {
        if sent < 0 {
            let errno = last_errno();
            log::warn!(
                "send result failed sock={} errno={} {}",
                sock,
                errno,
                errno_text(errno)
            );
        } else {
            log::warn!(
                "send result short sock={} sent={} want={}",
                sock,
                sent,
                expected_size
            );
        }
        return false;
    }
    log::debug!("send result sock={} ret={}", sock, result);
    true
}
