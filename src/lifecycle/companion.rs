use super::companion_mount::execute_companion_mount_request;
use super::companion_request::{CompanionMountRequest, parse_companion_mount_request};
use crate::logging::Logger;
use crate::platform::companion_socket;
use crate::platform::fs;
use libc::{SIG_IGN, SIGPIPE, c_int, signal};
use std::os::fd::AsRawFd;
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

    // 首字节携带发送时的内核凭据：M 为长度+JSON，F 为 uid+FD，H 为固定大小身份。
    let mut request_type = [0u8; 1];
    if !companion_socket::configure(client_fd, 10) {
        return;
    }
    let Ok((1, caller, None)) = companion_socket::receive(client_fd, &mut request_type) else {
        log::error!("request type read failed");
        write_status(client_fd, 0);
        return;
    };
    match request_type[0] {
        b'F' => {
            handle_fchown_request(client_fd, &caller);
        }
        b'M' => {
            handle_mount_request(client_fd, &caller);
        }
        b'H' => {
            let ok = super::hyos::handle_request(client_fd, &caller);
            write_status(client_fd, i32::from(ok));
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
fn handle_fchown_request(client_fd: c_int, caller: &libc::ucred) {
    let mut uid_buffer = [0u8; 4];
    let Ok((received, sender, Some(file))) = companion_socket::receive(client_fd, &mut uid_buffer)
    else {
        log::error!("fchown request fd receive failed");
        write_status(client_fd, 0);
        return;
    };
    if received != uid_buffer.len() || sender.pid != caller.pid || sender.uid != caller.uid {
        write_status(client_fd, 0);
        return;
    }
    let uid = i32::from_ne_bytes(uid_buffer);
    crate::redirect::policy::refresh_shared_uid_cache();
    let authorized = caller.uid == 0
        || i32::try_from(caller.uid).ok().is_some_and(|uid| {
            crate::redirect::policy::get_packages_for_uid(uid % 100000)
                .iter()
                .any(|package| crate::redirect::policy::is_system_writer_package(package))
        });
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: stat 指向可写结构，file 在请求处理期间保持打开。
    let stat_ok = unsafe { libc::fstat(file.as_raw_fd(), stat.as_mut_ptr()) } == 0;
    let target = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok();
    let package = if stat_ok {
        // SAFETY: fstat 成功写满结构。
        target
            .as_ref()
            .and_then(|path| path.to_str())
            .and_then(|path| {
                super::companion_policy::chown_target_package(
                    path,
                    uid,
                    // SAFETY: 此分支仅在 fstat 成功写满结构后执行。
                    unsafe { stat.assume_init() }.st_mode,
                )
            })
    } else {
        None
    };
    let target_valid = package.is_some_and(|package| {
        super::companion_policy::package_uid_matches(
            uid,
            crate::redirect::policy::get_uid_for_package(&package),
        )
    });
    if !authorized || !target_valid {
        log::error!("fchown request uid invalid uid={}", uid);
        write_status(client_fd, 0);
        return;
    }
    // SAFETY: file 独占接收到的 FD，所有返回路径均由 OwnedFd 关闭。
    let result = unsafe { libc::fchown(file.as_raw_fd(), uid as u32, u32::MAX) };
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
pub(super) fn ensure_daemon_alive() {
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

fn handle_mount_request(client_fd: c_int, caller: &libc::ucred) {
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

    if caller.uid != 0 && caller.pid == request.pid && caller.uid == request.uid as u32 {
        // post-specialize 的延后请求来自应用 UID，路径规则必须从 root 配置重新生成。
        let ok = super::hyos::apply_for_caller(&request.package_name, "zygisk", caller);
        write_status(client_fd, i32::from(ok));
        return;
    }
    if !super::companion_policy::mount_caller_matches(caller, request.pid) {
        log::warn!(
            "mount request caller rejected pid={} uid={} target={}",
            caller.pid,
            caller.uid,
            request.pid
        );
        write_status(client_fd, 0);
        return;
    }

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
/// 所以 `EPIPE` 是常态、只记 debug。F 与 H 在特化后新建连接，同步读取该状态。
pub(crate) fn write_status(client_fd: c_int, status: i32) -> bool {
    companion_socket::send(client_fd, &status.to_ne_bytes(), None)
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
