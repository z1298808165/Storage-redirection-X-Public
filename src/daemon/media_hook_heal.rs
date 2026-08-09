// MediaProvider hook 自愈：检测到 MediaProvider 缺少有效 Java hook 时重启该进程。
//
// 背景：MediaProvider 需要同时保有 Java 调用方识别和 media-runtime native
// 目录拦截。两者都会在首次 specialize 时预装；若注入阶段异常而没有留下有效
// 安装记录，则在已有应用启用重定向后从 daemon 侧触发一次自愈重启。
//
// 本模块从 daemon 侧做外部重启：MediaProvider 正在运行、已有应用启用重定向、
// 但安装记录不属于当前 boot 与该 pid 时，重启一次让它重新 specialize。
use crate::config::SettingsHub;
use crate::platform::module_paths;
use crate::redirect::policy;
use std::fs;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// 每次 daemon 生命周期内只自愈一次。
///
/// 注入本身失败（例如 zygisk denylist、模块加载失败）时，重启后记录依旧缺失，
/// 无上限重试会变成持续杀 MediaProvider 的重启循环。宁可放弃自愈，
/// 也不能让系统进程反复重启。
static SELF_HEAL_ATTEMPTED: AtomicBool = AtomicBool::new(false);

/// hook 已就绪的终态标记，与 `record_media_hook_install_state` 写入的 stage 对应。
const READY_STAGE: &str = "init_ok";

pub(super) struct MediaHookState {
    pub(super) stage: String,
    pub(super) pid: i32,
    pub(super) boot_id: String,
}

/// 解析 `stage=... pid=... boot_id=... boot_completed=...` 形式的单行记录。
fn parse_install_state(text: &str) -> Option<MediaHookState> {
    let line = text.lines().next()?.trim();
    if line.is_empty() {
        return None;
    }
    let mut stage = None;
    let mut pid = None;
    let mut boot_id = None;
    for field in line.split_whitespace() {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "stage" => stage = Some(value.to_string()),
            "pid" => pid = value.parse::<i32>().ok(),
            "boot_id" => boot_id = Some(value.to_string()),
            _ => {}
        }
    }
    Some(MediaHookState {
        stage: stage?,
        pid: pid?,
        boot_id: boot_id.unwrap_or_default(),
    })
}

fn read_install_state() -> Option<MediaHookState> {
    let text = fs::read_to_string(module_paths::MEDIA_HOOK_INSTALL_STATE_FILE).ok()?;
    parse_install_state(&text)
}

/// 判断记录是否证明「当前这个 MediaProvider 进程」的 hook 已就绪。
///
/// 记录文件在 `logs/` 下跨重启保留，而 pid 会跨 boot 复用，因此必须同时比对
/// boot id；boot id 缺失或为 unknown 时按不可信处理，不据此认定已就绪。
pub(super) fn is_hook_ready_for(
    state: Option<&MediaHookState>,
    media_pid: i32,
    current_boot_id: &str,
) -> bool {
    let Some(state) = state else {
        return false;
    };
    if state.stage != READY_STAGE || state.pid != media_pid {
        return false;
    }
    if current_boot_id.is_empty() || state.boot_id.is_empty() || state.boot_id == "unknown" {
        return false;
    }
    state.boot_id == current_boot_id
}

/// 上一次已登记的跳过原因，用于「仅在判定结果变化时输出」。
///
/// reconcile 每 3 秒一轮，若每轮都记录跳过原因，会把 running.log 的诊断窗口
/// （artifact 只截取尾部若干行）冲掉，反而丢失需要的证据。
static LAST_SKIP_REASON: AtomicU8 = AtomicU8::new(SKIP_NONE);

const SKIP_NONE: u8 = 0;
const SKIP_NO_MEDIA_PROCESS: u8 = 1;
const SKIP_ALREADY_ATTEMPTED: u8 = 2;
const SKIP_BOOT_INCOMPLETE: u8 = 3;
const SKIP_NO_ENABLED_APPS: u8 = 4;
const SKIP_HOOK_READY: u8 = 5;

/// 记录跳过原因，同一原因连续出现时只记录一次。
fn log_skip_once(reason: u8, detail: &str) {
    if LAST_SKIP_REASON.swap(reason, Ordering::Relaxed) == reason {
        return;
    }
    log::info!("media hook self-heal skip reason={} {}", reason, detail);
}

/// 在 daemon reconcile 中检查并按需自愈。
///
/// `media_processes` 为本轮枚举到的 MediaProvider 进程（pid 由调用方提供，
/// 避免本模块重复扫描 /proc）。`media_like_names` 是名字像 MediaProvider
/// 却未被判定命中的包名，用于区分「MediaProvider 没在跑」与「判定没认出它」。
pub(super) fn heal_if_needed(
    config: &SettingsHub,
    media_processes: &[(i32, i32)],
    media_like_names: &[String],
) {
    if media_processes.is_empty() {
        log_skip_once(
            SKIP_NO_MEDIA_PROCESS,
            &format!("no_media_provider_process unmatched_like={media_like_names:?}"),
        );
        return;
    }
    if SELF_HEAL_ATTEMPTED.load(Ordering::Relaxed) {
        log_skip_once(SKIP_ALREADY_ATTEMPTED, "already_attempted_this_daemon");
        return;
    }

    // 开机早期不介入：此时 MediaProvider 可能正在 specialize，记录尚未落盘，
    // 贸然重启会打断本来会正确装上 hook 的进程。boot_completed 之后，
    // `boot.sh` 的既有推迟重启也已执行完毕，不会与本自愈重复。
    if !crate::platform::is_boot_completed() {
        log_skip_once(SKIP_BOOT_INCOMPLETE, "boot_not_completed");
        return;
    }

    // 没有任何应用启用重定向时，MediaProvider 本就不需要拦截，重启只有副作用。
    let has_enabled_apps = media_processes
        .iter()
        .any(|(_, uid)| config.has_effective_enabled_redirect_apps_for_user(*uid));
    if !has_enabled_apps {
        log_skip_once(
            SKIP_NO_ENABLED_APPS,
            &format!(
                "no_enabled_redirect_apps media_pids={:?} media_uids={:?}",
                media_processes
                    .iter()
                    .map(|(pid, _)| *pid)
                    .collect::<Vec<_>>(),
                media_processes
                    .iter()
                    .map(|(_, uid)| *uid)
                    .collect::<Vec<_>>()
            ),
        );
        return;
    }

    let current_boot_id = crate::platform::read_boot_id();
    let state = read_install_state();
    let stale: Vec<i32> = media_processes
        .iter()
        .map(|(pid, _)| *pid)
        .filter(|pid| !is_hook_ready_for(state.as_ref(), *pid, &current_boot_id))
        .collect();
    if stale.is_empty() {
        log_skip_once(
            SKIP_HOOK_READY,
            &format!(
                "hook_ready_for_all media_pids={:?}",
                media_processes
                    .iter()
                    .map(|(pid, _)| *pid)
                    .collect::<Vec<_>>()
            ),
        );
        return;
    }

    if SELF_HEAL_ATTEMPTED.swap(true, Ordering::AcqRel) {
        return;
    }
    LAST_SKIP_REASON.store(SKIP_NONE, Ordering::Relaxed);

    log::warn!(
        "media hook self-heal: restart MediaProvider pids={:?} stage={} record_pid={} record_boot={}",
        stale,
        state.as_ref().map_or("absent", |s| s.stage.as_str()),
        state.as_ref().map_or(-1, |s| s.pid),
        state
            .as_ref()
            .map_or("absent", |s| if s.boot_id.is_empty() {
                "empty"
            } else {
                s.boot_id.as_str()
            })
    );

    for pid in stale {
        // SAFETY: kill 只按 pid 投递信号，不写入本进程内存；pid 来自本轮
        // /proc 枚举，最坏情况是进程已退出而 kill 返回 ESRCH，无副作用。
        let result = unsafe { libc::kill(pid, libc::SIGKILL) };
        if result != 0 {
            log::warn!("media hook self-heal: kill pid={} failed", pid);
        }
    }
}

/// 供 daemon 复用的 MediaProvider 判定，避免调用方直接依赖 policy 细节。
pub(super) fn is_media_provider_process(package_name: &str) -> bool {
    policy::is_media_provider_package(package_name)
}
