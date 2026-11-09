use super::companion_mount::execute_companion_mount_request;
use super::companion_request::{CompanionMountRequest, parse_companion_mount_request};
use crate::logging::Logger;
use crate::platform::fs;
use libc::{SIG_IGN, SIGPIPE, c_int, signal};
use std::path::Path;
use std::time::Duration;

const MAX_PAYLOAD_BYTES: u32 = 1024 * 1024;
// 伴生进程单次请求的读取超时：对端异常挂起时不能让伴生进程被永久占用
const READ_TIMEOUT: Duration = Duration::from_secs(10);

// 伴生进程主流程：读取请求、解析、执行挂载、返回状态
pub fn run_companion_pipeline(client_fd: c_int) {
    Logger::init(Some("zygisk_companion"));

    unsafe {
        signal(SIGPIPE, SIG_IGN);
    }

    // 协议 v2：首字节为请求类型；'M' 为挂载请求（其后为旧的长度+JSON 负载），
    // 'F' 为 fchown 请求（fd 经 SCM_RIGHTS 传递）。与挂载请求合并连接后按
    // 类型分发，两端同仓库同版本升级，无兼容包袱。
    let mut request_type = [0u8; 1];
    if !fs::read_all_timeout(client_fd, &mut request_type, READ_TIMEOUT) {
        log::error!("request type read failed");
        write_status(client_fd, 0);
        return;
    }
    match request_type[0] {
        b'F' => {
            handle_fchown_request(client_fd);
        }
        b'M' => {
            handle_mount_request(client_fd);
        }
        other => {
            log::error!("unknown request type 0x{:02x}", other);
            write_status(client_fd, 0);
        }
    }
}

/// 处理 fchown 请求：recvmsg 收 4 字节 uid 与 SCM_RIGHTS 携带的 fd，以 root
/// 身份 fchown 后回写状态。fd 由 MP hook 在 open:create 应答前传递，文件在
/// 应用拿到 fd 之前即为正确属主。
fn handle_fchown_request(client_fd: c_int) {
    let mut uid_buffer = [0u8; 4];
    let mut fd_buffer = [-1i32; 1];
    // SAFETY: uid_buffer/fd_buffer 为栈上有效缓冲；msg 名称为空，iovec 指向
    // uid_buffer；control 缓冲用于内核写入 SCM_RIGHTS。
    let received = unsafe {
        let iov = libc::iovec {
            iov_base: uid_buffer.as_mut_ptr() as *mut libc::c_void,
            iov_len: uid_buffer.len(),
        };
        // SAFETY: CMSG_SPACE 为纯常量计算，无运行时副作用。
        let mut cmsg_buffer =
            [0u8; unsafe { libc::CMSG_SPACE(std::mem::size_of::<c_int>() as u32) } as usize];
        let mut msg: libc::msghdr = libc::msghdr {
            msg_name: std::ptr::null_mut(),
            msg_namelen: 0,
            msg_iov: &iov as *const libc::iovec as *mut libc::iovec,
            msg_iovlen: 1,
            msg_control: cmsg_buffer.as_mut_ptr() as *mut libc::c_void,
            msg_controllen: cmsg_buffer.len(),
            msg_flags: 0,
        };
        let received = libc::recvmsg(client_fd, &mut msg, 0);
        if received < 0 {
            false
        } else {
            let mut received_fd = -1i32;
            let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
            while !cmsg.is_null() {
                if (*cmsg).cmsg_level == libc::SOL_SOCKET && (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    let available = ((*cmsg).cmsg_len as usize - libc::CMSG_LEN(0) as usize)
                        / std::mem::size_of::<c_int>();
                    if available >= 1 {
                        std::ptr::copy_nonoverlapping(
                            libc::CMSG_DATA(cmsg) as *const c_int,
                            fd_buffer.as_mut_ptr(),
                            1,
                        );
                        received_fd = fd_buffer[0];
                    }
                }
                cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
            }
            let _ = received_fd;
            received_fd >= 0
        }
    };
    if !received {
        log::error!("fchown request fd receive failed");
        write_status(client_fd, 0);
        return;
    }
    let uid = i32::from_ne_bytes(uid_buffer);
    if uid < 0 {
        log::error!("fchown request uid invalid uid={}", uid);
        write_status(client_fd, 0);
        return;
    }
    // SAFETY: fd_buffer[0] 为对端经 SCM_RIGHTS 传入的本进程 fd，fchown 后关闭。
    let result = unsafe { libc::fchown(fd_buffer[0], uid as u32, u32::MAX) };
    // SAFETY: 同上，fd 归本进程所有。
    unsafe { libc::close(fd_buffer[0]) };
    if result == 0 {
        log::info!("companion fchown ok uid={}", uid);
    } else {
        log::warn!(
            "companion fchown failed uid={} errno={}",
            uid,
            crate::platform::errno::last()
        );
    }
    write_status(client_fd, if result == 0 { 1 } else { 0 });
}

/// 软重启（zygote/system_server 级）不重跑模块的 service.sh：daemon 死后
/// companion 是唯一仍有 root 上下文的常驻入口。挂载请求到达时顺带检查并
/// 拉起 daemon，避免模块进入"已加载但无守护进程"的半死态——监视、巡检、
/// 自愈全部依赖 daemon（2026-10-31 issue #2 报告者实测：两次自动软重启后
/// 监视与巡检全失效）。
///
/// 存活判定与 service.sh 的 `daemon_process_matches` 同口径：按 /proc/<pid>/comm
/// 而非 exe 判定，排除 daemon fork 出来的 srx_fuse_host/srx_fuse 子进程（它们
/// exe 同为 srx_daemon 但 comm 已被 prctl 改名）。拉起走 spawn 脱离，daemon
/// 自行孵化看门狗并获取实例锁，companion 不参与后续管理。
fn ensure_daemon_alive() {
    use std::fs;
    use std::os::fd::AsRawFd;

    use crate::platform::module_paths;

    if Path::new(module_paths::RUNTIME_DISABLE_FILE).exists() {
        return;
    }
    let daemon_bin = module_paths::DAEMON_BIN_FILE;
    if !Path::new(daemon_bin).exists() {
        return;
    }

    // 挂载请求会随多应用 specialize 并发到达。把「检查 daemon 是否存活 + 拉起」串行化，
    // 否则一次软重启会拉起多条 daemon/看门狗链。
    let lock = match fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(module_paths::DAEMON_RESPAWN_LOCK_FILE)
    {
        Ok(file) => file,
        Err(err) => {
            log::warn!("daemon respawn lock open failed err={}", err);
            return;
        }
    };
    // SAFETY: flock 只对刚打开的有效文件描述符加建议锁，不涉及其它内存。
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return;
    }

    // pid 文件优先（与 service.sh 同口径）。
    let pid_file_alive = fs::read_to_string(crate::platform::module_paths::DAEMON_PID_FILE)
        .ok()
        .and_then(|text| text.trim().parse::<i32>().ok())
        .is_some_and(|pid| pid > 0 && daemon_comm_matches(pid));
    if pid_file_alive {
        return;
    }

    // pid 文件缺失/失效时的 /proc 兜底扫描。
    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(pid) = name.parse::<i32>() else {
                continue;
            };
            if pid > 0 && daemon_comm_matches(pid) {
                return;
            }
        }
    }

    log::warn!("daemon not alive after soft reboot, respawning from companion");
    reap_orphan_fuse_children();
    match std::process::Command::new(daemon_bin)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => {
            log::info!("daemon respawned by companion pid={}", child.id());
            let _ = fs::write(
                crate::platform::module_paths::DAEMON_PID_FILE,
                child.id().to_string(),
            );
        }
        Err(err) => log::warn!("daemon respawn failed err={}", err),
    }
}

/// 清理软重启后由 init 收养的旧 FUSE 服务。仅在确认没有 daemon 存活后调用，
/// 因为这些 fork 子进程会继承 daemon instance-lock fd，残留时新 daemon 会
/// already_running 退出；进程名白名单严格限定为本模块的 FUSE 子服务。
fn reap_orphan_fuse_children() {
    for name in ["srx_fuse", "srx_fuse_host"] {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return;
        };
        for entry in entries.flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|value| value.parse::<i32>().ok())
            else {
                continue;
            };
            if pid <= 1 || daemon_comm_matches(pid) {
                continue;
            }
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"));
            if comm.as_deref().is_ok_and(|comm| comm.trim() == name) {
                // SAFETY: pid 来自 /proc 且 comm 与模块自有子进程名精确匹配。
                if unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                    log::warn!(
                        "companion reaped orphan fuse child pid={} comm={}",
                        pid,
                        name
                    );
                }
            }
        }
    }
}

/// 与 service.sh 的 daemon_process_matches 同口径的存活判定。
fn daemon_comm_matches(pid: i32) -> bool {
    use std::fs;
    if pid <= 0 {
        return false;
    }
    // kill -0 等价的存活检查：/proc/<pid> 存在即可，comm 内容再校验。
    let comm_path = format!("/proc/{pid}/comm");
    fs::read_to_string(comm_path)
        .map(|comm| comm.trim() == "srx_daemon")
        .unwrap_or(false)
}

fn handle_mount_request(client_fd: c_int) {
    let payload_len = match read_payload_length(client_fd) {
        Some(len) => len,
        None => {
            // 连请求都没读完就回执：对端已经半途放弃，写不进去也无妨，但协议纪律是
            // 「每一条分支都给结论」，漏掉这条会让应用侧的失败永远无法定位。
            log::error!("payload length read failed");
            write_status(client_fd, 0);
            return;
        }
    };

    if payload_len == 0 || payload_len > MAX_PAYLOAD_BYTES {
        log::error!("payload length invalid len={}", payload_len);
        write_status(client_fd, 0);
        return;
    }
    log::debug!("payload len={}", payload_len);

    let payload = match read_payload_data(client_fd, payload_len) {
        Some(data) => data,
        None => {
            log::error!("payload read failed");
            write_status(client_fd, 0);
            return;
        }
    };

    let request = match parse_companion_mount_request(&payload) {
        Ok(req) => req,
        Err(err) => {
            log::error!("request parse failed err={}", err);
            write_status(client_fd, 0);
            return;
        }
    };

    if !request.package_name.is_empty() {
        Logger::init(Some(&request.package_name));
    }

    ensure_daemon_alive();

    log_mount_request(&request);

    let status = if execute_companion_mount_request(&request) {
        1
    } else {
        0
    };
    // 结论仍按协议写回（4 字节状态），但对 `'M'` 请求它已经是**尽力而为**：应用侧发完即关，
    // 那条连接在应用进程里不存在了（为什么必须这样，见 `mount_request` 模块头注释——SELinux
    // 域迁移让应用侧永远读不到）。因此 `EPIPE` 是常态而非故障，只记 debug；应用侧的结论改用
    // 挂载表观测，执行方的结论就在本进程的日志里（`perf companion mount ... ok=`）。
    match write_status(client_fd, status) {
        true => log::debug!(
            "companion ack sent fd={} status={} pkg={} pid={}",
            client_fd,
            status,
            request.package_name,
            request.pid
        ),
        false => log::debug!(
            "companion ack write skipped fd={} status={} pkg={} pid={} errno={}",
            client_fd,
            status,
            request.package_name,
            request.pid,
            crate::platform::errno::last()
        ),
    }
}

fn read_payload_length(client_fd: c_int) -> Option<u32> {
    let mut buffer = [0u8; 4];
    if !fs::read_all_timeout(client_fd, &mut buffer, READ_TIMEOUT) {
        log::warn!("read length failed fd={}", client_fd);
        return None;
    }
    Some(u32::from_ne_bytes(buffer))
}

// 按长度读取负载并强制 UTF-8 解码
fn read_payload_data(client_fd: c_int, payload_len: u32) -> Option<String> {
    let mut buffer = vec![0u8; payload_len as usize];
    if !fs::read_all_timeout(client_fd, &mut buffer, READ_TIMEOUT) {
        log::warn!("read payload failed fd={} len={}", client_fd, payload_len);
        return None;
    }
    match String::from_utf8(buffer) {
        Ok(text) => Some(text),
        Err(_) => {
            log::warn!("payload not utf8 fd={} len={}", client_fd, payload_len);
            None
        }
    }
}

/// 给一次 companion 连接写 4 字节结论。
///
/// 协议纪律：**每一个**分支（含"还没走到挂载就退出"的早退）都写，保持「有请求就有应答」
/// 这条协议不破。但对 `'M'`（挂载请求）它已是尽力而为——应用侧发完就关闭连接，那条连接
/// 不存在于应用进程里了（SELinux 域迁移让应用侧永远读不到，见 `mount_request` 模块头注释），
/// 所以 `EPIPE` 是常态、只记 debug。真正的消费者是 `'F'`（fchown）：它在应用域里新建连接，
/// 同域读写不受域迁移影响。
pub(crate) fn write_status(client_fd: c_int, status: i32) -> bool {
    fs::write_all(client_fd, &status.to_ne_bytes())
}

fn log_mount_request(request: &CompanionMountRequest) {
    log::info!(
        "req op=apply pid={} pkg={} allow={} excl={} sandbox={} ro={} map={} map_only={} file_monitor={} version={:x}",
        request.pid,
        request.package_name,
        request.allowed_real_paths.len(),
        request.excluded_real_paths.len(),
        request.sandboxed_paths.len(),
        request.read_only_paths.len(),
        request.path_mappings.len(),
        request.is_mapping_mode_only,
        request.is_file_monitor_enabled,
        request.config_version
    );

    if !request.allowed_real_paths.is_empty() {
        for path in &request.allowed_real_paths {
            log::info!("req allow={}", path);
        }
    }
    if !request.excluded_real_paths.is_empty() {
        for path in &request.excluded_real_paths {
            log::info!("req excl={}", path);
        }
    }
    if !request.sandboxed_paths.is_empty() {
        for path in &request.sandboxed_paths {
            log::info!("req sandbox={}", path);
        }
    }
    if !request.read_only_paths.is_empty() {
        for path in &request.read_only_paths {
            log::info!("req readonly={}", path);
        }
    }
    if !request.path_mappings.is_empty() {
        for mapping in &request.path_mappings {
            log::info!("req map {} -> {}", mapping.request_path, mapping.final_path);
        }
    }
}
