use crate::config::StorageBackendMode;
use crate::domain::PathMapping;
use crate::platform::errno::last as last_errno;
use crate::platform::{fs, module_paths, mountinfo, paths};
use fuser::{MountOption, SessionACL};
use serde::{Deserialize, Serialize};
use std::ffi::CString;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FuseCapability {
    Unknown,
    Available,
    Unavailable,
}

/// 单个应用 scoped 挂载连续失败达到该次数后，本轮开机内该应用不再尝试 scoped 挂载。
///
/// 单次失败可能来自开机竞态、目标进程正在退出或挂载点标签尚未就绪等可恢复事件，因此用少量
/// 额外尝试换取可恢复性。预算**按应用单独计**：某个应用自身的失败只影响它自己，不会让别的
/// 应用一起退回 namespace。
const SCOPED_MOUNT_FAILURE_BUDGET: u32 = 3;

/// 升格为「设备级不支持 scoped 挂载」所需的不同应用数。
///
/// 单个应用反复失败说明不了设备能力：它可能是每次启动都被杀、或配置本身有问题的应用。只有当
/// **多个不同应用**在没有成功插入的情况下接连失败，才足以判定是设备层面的问题。这样既保留
/// 了对真实设备缺陷的快速收敛，又消除了「一个坏应用把全体拖下水」的放大器。
const DEVICE_UNAVAILABLE_MIN_SCOPES: u32 = 2;

/// 设备级退避的起点与上限（毫秒）。
///
/// `unavailable` 不再是终态：到期后允许一次探测性尝试，成功即整体恢复，失败则把退避加倍。
/// 这条自愈通路取代了过去「只能靠重启恢复」的行为。
///
/// 上限必须显著小于宿主换代或 MediaProvider 重启引发的抖动恢复窗口之外的任何合理等待：
/// 系统应用在这些窗口内的 scoped 失败会持续把设备判为 `unavailable`（CI 实测 backoff_step
/// 累计到 51、锁出 10 分钟），而接入前的死层摘除自愈（`clear_dead_srx_layers_at_target`）
/// 只能在闸门放行后执行——锁出过长会把两个自愈机制互相锁死。防重试风暴由单应用失败预算
/// （[`SCOPED_MOUNT_FAILURE_BUDGET`]，每应用每开机至多 3 次）独立承担，设备级锁出只需覆盖
/// 一次抖动的持续时间。
/// 应用分桶预算耗尽后的探测窗口（毫秒）。
///
/// 与设备级退避同量级：窗口内不再尝试，窗口过后放行一次探测。失败会刷新时间戳并
/// 继续按窗口退避，因此稳态下开销只有「每窗口一次挂载尝试」。
const SCOPE_RETRY_WINDOW_MS: u64 = 60_000;

const RETRY_BACKOFF_BASE_MS: u64 = 30_000;
const RETRY_BACKOFF_MAX_MS: u64 = 60_000;

/// 快照中记录的 scope 数量上限。
///
/// 每个应用进入列表后会保留到下一次任意成功或重启为止，因此需要上限防止长期运行后无限增长；
/// 超出时丢弃最久未失败的那些（列表按最近失败时间排序）。
const MAX_TRACKED_SCOPES: usize = 64;

/// 能力快照内容。
///
/// 快照同时承载两类计数，二者**互不替代**：
///
/// - `device_failures`：设备级连续失败，只在失败来自与上一次不同的应用时累加，用于把「设备
///   不支持」与「某个应用自己有问题」区分开；
/// - `scope_failures`：每个应用自己的连续失败数，成功即清除该应用的分桶。
///
/// `teardown_failures` 只统计收尾（unmount）失败，**不参与挂载准入**：一次摘不掉旧挂载说明
/// 不了下一次 `mount(2)` 会失败，用它去关掉全设备的 FUSE 是口径错配。它保留下来只为可观测
/// 性与 `doctor` 展示。
struct FuseCapabilitySnapshot {
    /// 快照文件是否解析成功（存在且 boot_id 匹配）。
    ///
    /// 用于区分「没有信息」（daemon 从未写过快照）与「信息就是 unknown」（daemon 已给出结论口径、
    /// 只是还没有真实会话结果）。两者在 `Auto` 下的处置不同：前者只剩应用侧节点探测可用，后者
    /// 必须放行尝试。
    present: bool,
    capability: FuseCapability,
    device_failures: u32,
    last_failed_scope: String,
    /// 每个应用的分桶：(包名, 连续失败数, 最近一次失败的 `CLOCK_MONOTONIC` 毫秒)。
    ///
    /// 带上时间戳是为了让「预算耗尽」不再是整轮开机的终态：窗口过后允许一次探测，
    /// 成功即清零，失败则继续退避。此前只在开机时清零，一次启动风暴就能让某个应用
    /// 整轮开机都不再尝试 FUSE。
    scope_failures: Vec<(String, u32, u64)>,
    /// `Unavailable` 后允许下一次探测的 `CLOCK_MONOTONIC` 毫秒；`0` 表示立即可探测。
    retry_at_ms: u64,
    backoff_step: u32,
    teardown_failures: u32,
}

impl Default for FuseCapabilitySnapshot {
    fn default() -> Self {
        Self {
            present: false,
            capability: FuseCapability::Unknown,
            device_failures: 0,
            last_failed_scope: String::new(),
            scope_failures: Vec::new(),
            retry_at_ms: 0,
            backoff_step: 0,
            teardown_failures: 0,
        }
    }
}

impl FuseCapabilitySnapshot {
    fn scope_failures_for(&self, scope: &str) -> u32 {
        self.scope_failures
            .iter()
            .find(|(name, _, _)| name == scope)
            .map(|(_, count, _)| *count)
            .unwrap_or(0)
    }

    /// 该应用最近一次失败的 `CLOCK_MONOTONIC` 毫秒（无记录为 0）。
    fn scope_last_failure_ms(&self, scope: &str) -> u64 {
        self.scope_failures
            .iter()
            .find(|(name, _, _)| name == scope)
            .map(|(_, _, last_ms)| *last_ms)
            .unwrap_or(0)
    }

    /// 写入某个应用的失败计数，并按最近失败优先排序、按上限截断。
    fn bump_scope_failure(&mut self, scope: &str) {
        let count = self.scope_failures_for(scope).saturating_add(1);
        let now_ms = paths::monotonic_ms().max(0) as u64;
        self.scope_failures.retain(|(name, _, _)| name != scope);
        self.scope_failures
            .insert(0, (scope.to_string(), count, now_ms));
        self.scope_failures.truncate(MAX_TRACKED_SCOPES);
    }

    fn clear_scope_failure(&mut self, scope: &str) {
        self.scope_failures.retain(|(name, _, _)| name != scope);
    }

    /// 该应用是否已用完自己的 scoped 挂载预算。
    ///
    /// 预算耗尽后仍按 `SCOPE_RETRY_WINDOW_MS` 周期性放行一次探测：真实失败往往集中
    /// 在开机风暴期（宿主尚未就绪、存储未解密完），把它当成整轮开机的终态会让应用
    /// 在系统稳定后也永远等不到 FUSE 视图。
    fn scope_budget_exhausted(&self, scope: &str) -> bool {
        if self.scope_failures_for(scope) < SCOPED_MOUNT_FAILURE_BUDGET {
            return false;
        }
        let last_ms = self.scope_last_failure_ms(scope);
        if last_ms == 0 {
            return false;
        }
        let now_ms = paths::monotonic_ms().max(0) as u64;
        now_ms.saturating_sub(last_ms) < SCOPE_RETRY_WINDOW_MS
    }

    /// 当前是否已越过退避窗口，允许一次探测性尝试。
    fn retry_window_open(&self, now_ms: u64) -> bool {
        self.retry_at_ms == 0 || now_ms >= self.retry_at_ms
    }
}

/// 单个 FUSE 会话的完整策略配置。
///
/// 支持序列化：宿主共享会话的按 uid 策略在 daemon 侧构造，经控制通道以 JSON 送到持有会话的
/// 子进程后再建策略，因此这里必须能跨进程传输（见 `fuse_host::register_app_policy`）。
#[derive(Clone, Serialize, Deserialize)]
pub struct FuseRedirectConfig {
    pub package_name: String,
    pub app_pid: i32,
    pub app_start_time_ticks: Option<u64>,
    pub uid: i32,
    pub app_data_dir: String,
    pub redirect_target: String,
    pub mount_root: Option<String>,
    pub real_root_override: Option<String>,
    pub is_file_monitor_enabled: bool,
    pub allowed_real_paths: Vec<String>,
    pub excluded_real_paths: Vec<String>,
    pub sandboxed_paths: Vec<String>,
    pub read_only_paths: Vec<String>,
    pub path_mappings: Vec<PathMapping>,
    pub is_mapping_mode_only: bool,
    /// 是否为未配置重定向应用的监视专用直通会话。
    ///
    /// 该模式只记录 FUSE 请求，所有路径都落到真实共享存储，不改变应用看到的后端。
    pub is_monitor_only: bool,
    /// 是否为共享宿主直通会话。
    ///
    /// 宿主会话把整个存储根以真实后端暴露出来，供各应用 namespace 复用同一份 FUSE 会话；
    /// 它不做任何重定向，`redirect_target` 因此就是存储根本身。普通应用配置恒为 `false`。
    pub is_passthrough_host: bool,
    /// 只登记 mount namespace 维度的策略，不写按 uid 的全局表。
    ///
    /// 供 daemon 自身（uid=0）使用：文件监视等 root FUSE 请求按调用方 namespace 精确回查
    /// 策略，而 uid=0 绝不允许进按 uid 表（否则任何 root 进程都会拿到这份策略，破坏
    /// 「未登记 uid 一律 deny_all」的 fail-closed 边界）。序列化时恒缺席，普通应用配置
    /// 与策略指纹不受影响。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub namespace_policy_only: bool,
}

impl FuseRedirectConfig {
    pub(super) fn user_id(&self) -> i32 {
        crate::platform::user_id_from_uid(self.uid)
    }
}

/// 挂载请求中构造 FUSE 配置与计算 scoped 挂载根所需的字段。
///
/// daemon 侧的 `MountRequest` 与 companion 侧的 `CompanionMountRequest` 各自演进，字段
/// 相同但类型不同（前者另有 `operation`），此前两处的配置构造与挂载根计算是逐字重复的。
/// 由该 trait 统一取值，两处共用下面的 [`fuse_config_from_request`] 与
/// [`scoped_fuse_mount_roots_for_request`]，避免规则字段增减时漏改一侧。
pub trait MountRequestFields {
    fn package_name(&self) -> &str;
    fn pid(&self) -> i32;
    fn uid(&self) -> i32;
    fn app_data_dir(&self) -> &str;
    fn redirect_target(&self) -> &str;
    fn is_file_monitor_enabled(&self) -> bool;
    fn storage_backend_mode(&self) -> StorageBackendMode;
    fn allowed_real_paths(&self) -> &[String];
    fn excluded_real_paths(&self) -> &[String];
    fn sandboxed_paths(&self) -> &[String];
    fn read_only_paths(&self) -> &[String];
    fn path_mappings(&self) -> &[PathMapping];
    fn is_mapping_mode_only(&self) -> bool;
    fn is_monitor_only(&self) -> bool;
}

/// 按挂载请求构造 FUSE 重定向配置。
pub fn fuse_config_from_request<R: MountRequestFields + ?Sized>(
    request: &R,
    mount_root: Option<String>,
    real_root_override: Option<String>,
) -> FuseRedirectConfig {
    FuseRedirectConfig {
        package_name: request.package_name().to_string(),
        app_pid: request.pid(),
        app_start_time_ticks: crate::platform::process_start_time_ticks(request.pid()),
        uid: request.uid(),
        app_data_dir: request.app_data_dir().to_string(),
        redirect_target: request.redirect_target().to_string(),
        mount_root,
        real_root_override,
        is_file_monitor_enabled: request.is_file_monitor_enabled(),
        allowed_real_paths: request.allowed_real_paths().to_vec(),
        excluded_real_paths: request.excluded_real_paths().to_vec(),
        sandboxed_paths: request.sandboxed_paths().to_vec(),
        read_only_paths: request.read_only_paths().to_vec(),
        path_mappings: request.path_mappings().to_vec(),
        is_mapping_mode_only: request.is_mapping_mode_only(),
        is_monitor_only: request.is_monitor_only(),
        is_passthrough_host: false,
        namespace_policy_only: false,
    }
}

/// 计算单个 UID 策略的稳定指纹，供 daemon 与 companion 校验宿主策略新鲜度。
pub fn request_policy_fingerprint<R: MountRequestFields + ?Sized>(request: &R) -> u64 {
    let mut config = fuse_config_from_request(request, None, None);
    config.app_pid = 0;
    config.app_start_time_ticks = None;
    fuse_config_policy_fingerprint(&config)
}

/// 对一份已归一的宿主策略配置计算内容指纹（与 [`request_policy_fingerprint`] 同一哈希核）。
///
/// 宿主侧用它给 [`RedirectPolicy`](super::policy::RedirectPolicy) 标注「本策略由哪份配置
/// 构建」，在只读拒绝/ENOENT 等异常回包处打出该指纹，即可直接判定「当时生效的是旧策略
/// 还是新策略」，不必再从指纹去重链路反推。
pub fn fuse_config_policy_fingerprint(config: &FuseRedirectConfig) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut config = config.clone();
    config.app_pid = 0;
    config.app_start_time_ticks = None;
    if config.is_monitor_only {
        // 监视专用会话的全部路径都落真实后端，路由不依赖包名与数据目录（redirect_root
        // 强制为真实根、无路径映射），而宿主策略按 uid 登记，同一 uid 的多进程这两项
        // 都随包名不同（如 com.google.android.gms 与 .unstable）。指纹若含进程身份，
        // 各进程会在每轮 reconcile 互判“配置变更”交替重登记，宿主策略快照随之周期
        // 刷屏。监视模式下把包名与数据目录归一为空串，同 uid 指纹即稳定一致；进程
        // 归因由文件监视侧的调用方识别负责，不经策略指纹。
        config.package_name = String::new();
        config.app_data_dir = String::new();
    }
    let payload = serde_json::to_vec(&config).unwrap_or_default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    payload.hash(&mut hasher);
    hasher.finish()
}

/// 计算挂载请求对应的 scoped 挂载根；未启用 FUSE daemon 重定向时返回空列表。
pub fn scoped_fuse_mount_roots_for_request<R: MountRequestFields + ?Sized>(
    request: &R,
) -> Vec<String> {
    if request.is_monitor_only() && request.is_file_monitor_enabled() {
        let user_id = crate::platform::user_id_from_uid(request.uid());
        return vec![paths::storage_user_root_for_user(user_id)];
    }

    // 监视桥接进程（下载提供器/MTP/externalstorage）自身配置了重定向规则时，不挂任何
    // 模块 FUSE 会话：这类进程的原生写入由进程内 hook 兜底重定向（见
    // InterceptHub::effective_monitor_only）。模块会话压在存储视图上会踩 AOSP
    // FuseDaemon 直通陷阱（rename/mkdir 返回 ENOENT/EDOM 但数据已落盘，真机表现为
    // 应用商店下载无法合并/安装）；且 scoped 根只覆盖规则父目录时，其余视图仍被只读
    // 屏障挡住，引擎初始化 mkdir 全量 EROFS（2026-10-22 真机：商店下载全挂）。
    // 桥接进程走 hook 兜底后，其余路径经系统 FUSE 直通，与未装模块时等价。
    if !request.is_monitor_only()
        && crate::redirect::policy::is_file_monitor_bridge_package(request.package_name())
    {
        return Vec::new();
    }

    if matches!(
        request.storage_backend_mode(),
        StorageBackendMode::Namespace
    ) {
        return Vec::new();
    }

    let backend_mode = request.storage_backend_mode();
    if !scoped_mount_allowed_for_scope(request.package_name(), backend_mode) {
        return Vec::new();
    }

    // 统一走 FUSE：`Auto` 的语义是「FUSE 优先」——能力可用就由本会话接管整棵视图根，
    // 规则仍在策略层逐请求判定；能力不可用时由上面的能力门禁返回空根，挂载层回落
    // namespace。`Fuse` 只是显式锁定，两者此处同构。
    let user_id = crate::platform::user_id_from_uid(request.uid());
    vec![paths::storage_user_root_for_user(user_id)]
}

/// 检查设备是否暴露可读写的 `/dev/fuse`。
///
/// 这只是设备节点探测，不代表当前内核、挂载 namespace 或 FUSE 修复链路支持完整
/// scoped 会话。真实能力必须由实际挂载结果确认，失败后回退到 namespace。
pub fn fuse_device_present() -> bool {
    std::fs::metadata("/dev/fuse")
        .map(|metadata| metadata.file_type().is_char_device())
        .unwrap_or(false)
}

/// 返回 daemon 最近一次记录的设备级 FUSE 能力。
///
/// 普通应用只读取这个原子替换的快照，不直接打开 `/dev/fuse`。快照缺失或来自其它开机时
/// 保持 `Unknown`，由规划层走保守的 namespace fallback 路径。
///
/// 注意这是**设备级**结论：判断某个应用自己是否还能尝试 FUSE，必须用
/// [`scoped_mount_allowed_for_scope`]，否则会把别的应用的失败当成自己的。
pub fn fuse_capability() -> FuseCapability {
    load_fuse_capability_snapshot().capability
}

/// 读取当前开机的能力快照；缺失、来自其它开机或不可解析时返回默认值（`Unknown` 且无计数）。
fn load_fuse_capability_snapshot() -> FuseCapabilitySnapshot {
    read_fuse_capability_snapshot().unwrap_or_default()
}

/// 读取当前开机的能力快照；快照缺失、boot_id 不匹配或内容不可解析时返回 None。
fn read_fuse_capability_snapshot() -> Option<FuseCapabilitySnapshot> {
    let content = std::fs::read_to_string(module_paths::FUSE_CAPABILITY_FILE).ok()?;
    let current_boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string());
    if current_boot_id.as_deref() != snapshot_field(&content, "boot_id") {
        return None;
    }
    let mut snapshot = FuseCapabilitySnapshot {
        present: true,
        capability: match snapshot_field(&content, "state") {
            Some("available") => FuseCapability::Available,
            Some("unavailable") => FuseCapability::Unavailable,
            _ => FuseCapability::Unknown,
        },
        // schema 2 的 `fail_count` 是未分桶的全局计数，按设备级计数读入即可：升级前写下的
        // `unavailable` 会带 `retry_at_ms=0`，因此升级后第一次请求就能探测一次并自愈，
        // 不需要等设备重启。
        device_failures: snapshot_field(&content, "fail_count")
            .or_else(|| snapshot_field(&content, "device_fail_count"))
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0),
        last_failed_scope: snapshot_field(&content, "last_failed_scope")
            .unwrap_or_default()
            .to_string(),
        scope_failures: parse_scope_failures(&content),
        retry_at_ms: snapshot_field(&content, "retry_at_ms")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        backoff_step: snapshot_field(&content, "backoff_step")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0),
        teardown_failures: snapshot_field(&content, "teardown_fail_count")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0),
    };
    snapshot.scope_failures.truncate(MAX_TRACKED_SCOPES);
    Some(snapshot)
}

/// 解析 `scope_fail=<包名>:<次数>` 行。
///
/// 包名不会包含冒号，因此按最后一个冒号切分即可；解析失败的行直接跳过，不让一行坏数据
/// 使整份快照退化成默认值（那会把已经积累的退避一起丢掉）。
fn parse_scope_failures(content: &str) -> Vec<(String, u32, u64)> {
    content
        .lines()
        .filter_map(|line| line.strip_prefix("scope_fail="))
        .filter_map(|value| {
            // 包名不含冒号（与写入侧同一约定），因此按冒号顺序切分即可；
            // 旧快照只有两段（包名:次数），此时按「立即允许探测」处理。
            let mut fields = value.trim().split(':');
            let name = fields.next()?;
            let count = fields.next()?.parse::<u32>().ok()?;
            let last_ms = fields
                .next()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            if name.is_empty() {
                return None;
            }
            Some((name.to_string(), count, last_ms))
        })
        .collect()
}

/// 从能力快照内容中读取一个 `key=value` 字段。
fn snapshot_field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    content
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
        .map(str::trim)
}

pub fn fuse_capability_as_str(capability: FuseCapability) -> &'static str {
    match capability {
        FuseCapability::Unknown => "unknown",
        FuseCapability::Available => "available",
        FuseCapability::Unavailable => "unavailable",
    }
}

/// 某个应用是否允许尝试 scoped 挂载。
///
/// 判定同时看设备级状态与**该应用自己**的失败分桶，这是把「一个应用的失败」与「设备的失败」
/// 分开的关键：
///
/// - `Namespace`：显式要求 namespace 后端，不尝试；
/// - `Fuse`：用户显式选择 FUSE，只做设备节点探测，不受 `Auto` 的熔断约束；
/// - `Auto`：设备可用即尝试；设备未知时按节点探测；设备被判 `Unavailable` 时等退避窗口到期
///   再放行一次探测（成功即整体恢复，失败则退避加倍），不再需要重启。
///
/// 任何状态下，若该应用自己的失败分桶已到预算，本轮开机内它不再尝试；其它应用不受影响。
pub fn scoped_mount_allowed_for_scope(scope: &str, mode: StorageBackendMode) -> bool {
    let verdict = match mode {
        StorageBackendMode::Namespace => false,
        StorageBackendMode::Fuse => fuse_first_capability_available(),
        StorageBackendMode::Auto => {
            let snapshot = load_fuse_capability_snapshot();
            if snapshot.scope_budget_exhausted(scope) {
                false
            } else {
                match snapshot.capability {
                    FuseCapability::Available => true,
                    // `Unknown` 只表示还没有真实会话结果，必须放行尝试：真实能力由实际挂载结果确认
                    // （`fuse_device_present` 的文档也是这么写的）。**不能**在这里退回应用侧节点探测：
                    // 这个判定同样会在应用进程里执行，而应用视角的 `/dev/fuse` 常因 SELinux 不可读
                    // （实测 HyperOS 上是 `crw------- root root`，连 stat 都被拒），据此否定会让应用
                    // 永远规划不出 FUSE 根、只能退回 namespace，且永远等不到那个能解锁的失败计数。
                    // 只有快照完全不存在（daemon 从未写过）时才退回节点探测，此时没有更好的信息源。
                    FuseCapability::Unknown => snapshot.present || fuse_device_present(),
                    FuseCapability::Unavailable => {
                        snapshot.retry_window_open(paths::monotonic_ms().max(0) as u64)
                    }
                }
            }
        }
    };
    // 诊断：视图根上是否会有 scoped FUSE 层完全取决于这里的结论。Android 13 场景 29 的映射
    // 只落地后端别名，而通过的 Android 14 在视图根上多一层 FUSE（0:88 vs 254:40）。
    // 该判定在每个挂载请求路径上都会执行（应用启动风暴期可达每秒数十次），正常放行
    // （verdict=true 且无预算/失败信号）不产生任何新信息，全量打印会形成日志风暴并放大
    // CPU 占用；只在否定或带失败信号时打印，保留排障所需的全部异常证据。
    let snapshot = load_fuse_capability_snapshot();
    let budget_exhausted = snapshot.scope_budget_exhausted(scope);
    if !verdict || budget_exhausted {
        let capability_text = match snapshot.capability {
            FuseCapability::Unknown => "unknown",
            FuseCapability::Available => "available",
            FuseCapability::Unavailable => "unavailable",
        };
        log::warn!(
            "scoped gate diag scope={} mode={} verdict={} capability={} present={} budget_exhausted={} scope_failures={}",
            scope,
            mode.as_str(),
            verdict,
            capability_text,
            snapshot.present,
            budget_exhausted,
            snapshot.scope_failures_for(scope)
        );
    }
    verdict
}

/// 自动后端使用的 fallback 路径决策，应用侧与 daemon 共享同一个判断接口。
///
/// 设备级判定保持原语义：只要不是明确 `Available`（`Unknown` 或 `Unavailable`）就收敛通配
/// 规则，因为这两种状态下规划拿不到可靠的 FUSE 根。在此之上叠加**该应用自己**的失败分桶：
/// 该应用已用完预算时它必然走 namespace，它的规则必须收敛，而这一条不再改写其它应用的规则。
pub fn expand_mount_fallbacks_for_mode(mode: StorageBackendMode, scope: &str) -> bool {
    match mode {
        StorageBackendMode::Namespace => true,
        StorageBackendMode::Fuse => false,
        StorageBackendMode::Auto => {
            let device_not_confirmed_available = fuse_capability() != FuseCapability::Available;
            device_not_confirmed_available
                || load_fuse_capability_snapshot().scope_budget_exhausted(scope)
        }
    }
}

/// 在实际 FUSE 启动失败后，把 namespace fallback 所需的通配规则收敛到父目录。
pub fn expand_namespace_fallback_rules(uid: i32, rules: &[String]) -> Vec<String> {
    let user_id = crate::platform::user_id_from_uid(uid);
    let storage_root = paths::storage_user_root_for_user(user_id);
    let mut expanded = Vec::with_capacity(rules.len());
    for rule in rules {
        let trimmed = rule.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (excluded, body) = if let Some(stripped) = trimmed.strip_prefix('!') {
            (true, stripped.trim_start())
        } else {
            (false, trimmed)
        };
        let resolved = if !excluded && paths::contains_wildcards(body) {
            paths::wildcard_policy_fallback_parent(body, &storage_root)
                .unwrap_or_else(|| body.to_string())
        } else {
            body.to_string()
        };
        expanded.push(if excluded {
            format!("!{resolved}")
        } else {
            resolved
        });
    }
    paths::sort_dedup_paths_case_insensitive(&mut expanded);
    expanded
}

/// 第 `step` 次退避的等待毫秒数：基数逐次加倍，封顶 [`RETRY_BACKOFF_MAX_MS`]。
fn retry_backoff_ms(step: u32) -> u64 {
    let shift = step.saturating_sub(1).min(5);
    RETRY_BACKOFF_BASE_MS
        .saturating_mul(1u64 << shift)
        .min(RETRY_BACKOFF_MAX_MS)
}

fn write_fuse_capability_snapshot(
    snapshot: &FuseCapabilitySnapshot,
    reason: &str,
) -> FuseCapability {
    let capability = snapshot.capability;
    let state = match capability {
        FuseCapability::Available => "available",
        FuseCapability::Unavailable => "unavailable",
        FuseCapability::Unknown => "unknown",
    };
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let mut content = format!(
        "schema=3\nboot_id={boot_id}\nstate={state}\nreason={reason}\ndevice_fail_count={}\nlast_failed_scope={}\nretry_at_ms={}\nbackoff_step={}\nteardown_fail_count={}\n",
        snapshot.device_failures,
        snapshot.last_failed_scope,
        snapshot.retry_at_ms,
        snapshot.backoff_step,
        snapshot.teardown_failures,
    );
    for (scope, count, last_ms) in &snapshot.scope_failures {
        content.push_str(&format!("scope_fail={scope}:{count}:{last_ms}\n"));
    }
    let path = std::path::Path::new(module_paths::FUSE_CAPABILITY_FILE);
    // 快照会被 daemon 与多个 scoped 会话子进程同时写入，固定 temp 名会让并发写入
    // 互相 rename 掉对方的临时文件，这里带上 pid 与自增序号保证唯一。
    let temp = capability_snapshot_temp_path(path);
    let write_result = std::fs::write(&temp, content).and_then(|()| std::fs::rename(&temp, path));
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp);
        log::warn!(
            "fuse capability snapshot write failed state={} reason={} err={}",
            state,
            reason,
            error
        );
    } else {
        log::info!(
            "fuse capability snapshot state={} reason={} device_fail={} scopes={} retry_at_ms={} backoff_step={} teardown_fail={}",
            state,
            reason,
            snapshot.device_failures,
            snapshot.scope_failures.len(),
            snapshot.retry_at_ms,
            snapshot.backoff_step,
            snapshot.teardown_failures
        );
    }
    capability
}

/// 生成能力快照的临时文件路径，供同目录下的原子替换使用。
///
/// 每次写入都使用独立文件名，避免 daemon 与 scoped 会话子进程并发写入时互相
/// 覆盖临时文件，导致其中一方 rename 失败。
fn capability_snapshot_temp_path(path: &std::path::Path) -> std::path::PathBuf {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let mut temp = path.as_os_str().to_os_string();
    temp.push(format!(".{}.{}.tmp", std::process::id(), sequence));
    std::path::PathBuf::from(temp)
}

/// 在 root daemon 或 companion 挂载路径中刷新能力快照。
// quality-allow(lint-suppression): 该入口由 Android daemon 二进制调用，cdylib 目标不会直接调用。
#[allow(dead_code)]
pub fn refresh_fuse_capability_snapshot(reason: &str) -> FuseCapability {
    // 开机 / daemon 重启时整份快照归零：设备级计数、各应用分桶与退避全部清空，让新的一轮
    // 从「未知」重新探测。这正是「重启能恢复」的机制，现在退避窗口也提供了不开机的等价通路。
    let snapshot = FuseCapabilitySnapshot {
        capability: if fuse_first_capability_available() {
            // 打开设备只能证明节点存在；避免在首次真实会话前把 HyperOS/MIUI 的
            // FUSE 兼容性误报为 Available。首次 scoped 挂载会把状态推进到最终结果。
            FuseCapability::Unknown
        } else {
            FuseCapability::Unavailable
        },
        ..FuseCapabilitySnapshot::default()
    };
    write_fuse_capability_snapshot(&snapshot, reason)
}

/// 记录实际 scoped FUSE 挂载结果。
///
/// `scope` 是发起本次挂载的应用（包名）。计数分两层，二者用途不同：
///
/// - **应用分桶**：该应用的连续失败次数，成功即清零；到 [`SCOPED_MOUNT_FAILURE_BUDGET`] 后
///   只让这个应用停止尝试 FUSE，其它应用照旧；
/// - **设备级计数**：只在失败来自与上一次**不同的应用**时累加；达到
///   [`DEVICE_UNAVAILABLE_MIN_SCOPES`] 才把设备判为 `unavailable`，避免单个应用自己的问题
///   （每次启动都被杀、配置异常）把整机 FUSE 一起关掉。
///
/// 判为 `unavailable` 时同时写入退避截止时间；到期后 [`scoped_mount_allowed_for_scope`] 会
/// 放行一次探测，成功即整体恢复，失败则退避加倍。因此 `unavailable` 不再是终态。
pub fn record_fuse_capability_result(available: bool, reason: &str, scope: &str) -> FuseCapability {
    // 计数是读改写序列，而 daemon 会并发处理不同应用的挂载请求；用同目录锁文件串行化，
    // 避免并发失败互相覆盖计数导致预算迟迟达不到。锁获取失败只降低计数精度，不影响写入。
    let _lock = CapabilitySnapshotLock::acquire();
    let mut snapshot = read_fuse_capability_snapshot().unwrap_or_default();
    if available {
        snapshot.capability = FuseCapability::Available;
        snapshot.device_failures = 0;
        snapshot.last_failed_scope.clear();
        snapshot.clear_scope_failure(scope);
        snapshot.retry_at_ms = 0;
        snapshot.backoff_step = 0;
    } else {
        if snapshot.last_failed_scope != scope {
            snapshot.device_failures = snapshot.device_failures.saturating_add(1);
        }
        snapshot.last_failed_scope = scope.to_string();
        snapshot.bump_scope_failure(scope);
        if snapshot.device_failures >= DEVICE_UNAVAILABLE_MIN_SCOPES {
            snapshot.capability = FuseCapability::Unavailable;
            snapshot.backoff_step = snapshot.backoff_step.saturating_add(1);
            // 时间基准是 `CLOCK_MONOTONIC`（Android 上按开机计），快照本身带 boot_id 校验，
            // 因此跨进程比较是安全的；换开机后 boot_id 不匹配，整份快照直接失效。
            let now_ms = paths::monotonic_ms().max(0) as u64;
            snapshot.retry_at_ms = now_ms.saturating_add(retry_backoff_ms(snapshot.backoff_step));
        } else {
            snapshot.capability = FuseCapability::Unknown;
            snapshot.retry_at_ms = 0;
        }
    }
    write_fuse_capability_snapshot(&snapshot, reason)
}

/// 记录一次 scoped 会话收尾（unmount）失败。
///
/// 收尾失败**不参与**挂载准入：一次摘不掉旧挂载说明不了下一次 `mount(2)` 会失败，用它去关掉
/// 全设备的 FUSE 属于口径错配（历史上它确实能单独把整机锁成 `unavailable`）。这里只累计计数
/// 供 `doctor` 与日志观察；挂载准入完全由 [`record_fuse_capability_result`] 的分桶决定。
/// 收尾本身有挂载账本与监督流程兜底，泄漏是可控且有界的。
pub fn record_fuse_teardown_failure(reason: &str) {
    let _lock = CapabilitySnapshotLock::acquire();
    let mut snapshot = read_fuse_capability_snapshot().unwrap_or_default();
    snapshot.teardown_failures = snapshot.teardown_failures.saturating_add(1);
    write_fuse_capability_snapshot(&snapshot, reason);
}

/// 供诊断输出使用的能力快照摘要。
// quality-allow(lint-suppression): 只被 Android daemon 二进制的 doctor 子命令使用，lib 目标不会构造它。
#[allow(dead_code)]
pub struct FuseCapabilitySummary {
    pub capability: FuseCapability,
    pub device_failures: u32,
    pub scope_failures: Vec<(String, u32, u64)>,
    pub retry_at_ms: u64,
    pub backoff_step: u32,
    pub teardown_failures: u32,
}

/// 读取当前能力快照的摘要，供 `doctor` 展示判定依据（而不是只显示一个 state）。
// quality-allow(lint-suppression): 同 `FuseCapabilitySummary`，只服务 daemon 的 doctor 子命令。
#[allow(dead_code)]
pub fn fuse_capability_summary() -> FuseCapabilitySummary {
    let snapshot = load_fuse_capability_snapshot();
    FuseCapabilitySummary {
        capability: snapshot.capability,
        device_failures: snapshot.device_failures,
        scope_failures: snapshot.scope_failures,
        retry_at_ms: snapshot.retry_at_ms,
        backoff_step: snapshot.backoff_step,
        teardown_failures: snapshot.teardown_failures,
    }
}

/// 能力快照的跨进程互斥锁。
///
/// daemon 按 pid 并发处理挂载请求，多个 scoped 会话子进程也会写同一份快照，因此失败计数
/// 需要跨进程串行化。锁文件与快照同目录，关闭 fd 即释放锁。
struct CapabilitySnapshotLock {
    fd: libc::c_int,
}

impl CapabilitySnapshotLock {
    fn acquire() -> Option<Self> {
        let path = CString::new(format!("{}.lock", module_paths::FUSE_CAPABILITY_FILE)).ok()?;
        // SAFETY: path 是以 NUL 结尾的合法路径，flags 与 mode 只用于创建打开锁文件。
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_CREAT | libc::O_RDWR | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return None;
        }
        // SAFETY: fd 来自上面的 open，且在本次调用中尚未交给其它所有者。
        if unsafe { libc::flock(fd, libc::LOCK_EX) } != 0 {
            unsafe { libc::close(fd) };
            return None;
        }
        Some(Self { fd })
    }
}

impl Drop for CapabilitySnapshotLock {
    fn drop(&mut self) {
        // SAFETY: fd 来自 CapabilitySnapshotLock::acquire，并且在此之后不再使用。
        unsafe { libc::close(self.fd) };
    }
}

fn fuse_first_capability_available() -> bool {
    if !fuse_device_present() {
        log::warn!("fuse-first capability missing /dev/fuse");
        return false;
    }
    let Ok(path) = CString::new("/dev/fuse") else {
        return false;
    };
    // SAFETY: path 指向以 NUL 结尾的固定字符串，flags 只读写设备能力探测；返回 fd 立即关闭。
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if fd < 0 {
        log::warn!("fuse-first capability cannot open /dev/fuse");
        return false;
    }
    // SAFETY: fd 来自上面的 open，且在当前线程中尚未交给其它所有者。
    unsafe { libc::close(fd) };
    true
}

/// 探测大核 CPU 集合（cpuinfo_max_freq 等于全机最高频率的 CPU）。
///
/// MTK/高通大小核架构上，FUSE 事件循环线程被调度到 LITTLE 核时吞吐损失可达
/// 1.5 倍以上（真机 2026-10-15：fuser-0/3/6 挤在同一个 2.2GHz 小核上）。探测失败
/// 或单核设备返回 None，调用方保持默认亲和性。
fn big_core_cpu_mask() -> Option<u64> {
    let mut max_freq: u64 = 0;
    let mut freqs: Vec<(u32, u64)> = Vec::new();
    for cpu in 0..32u32 {
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/cpuinfo_max_freq");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(freq) = text.trim().parse::<u64>() else {
            continue;
        };
        max_freq = max_freq.max(freq);
        freqs.push((cpu, freq));
    }
    if freqs.is_empty() || max_freq == 0 {
        return None;
    }
    let mut mask: u64 = 0;
    for (cpu, freq) in &freqs {
        if *freq == max_freq {
            mask |= 1 << cpu;
        }
    }
    (mask != 0).then_some(mask)
}

/// 把当前线程（及其后续派生的会话线程）绑定到大核。
///
/// spawn_mount2 派生的事件循环线程继承调用线程的亲和性掩码，因此在 mount 前设置
/// 一次即可覆盖全部会话线程。绑定失败（mask 越界、权限）静默跳过。
/// 挂载成功后放宽本 FUSE 连接的回写脏页配额。
///
/// 内核 fuse 默认把每个连接 BDI 的 `max_ratio` 限为 1：应用对挂载点的脏页被
/// balance_dirty_pages 严厉限流，回写流水线长期缺请求（实测 daemon 侧 recv
/// 平均阻塞 2.4ms 等内核提交下一个请求），大块顺序写吞吐被腰斩。MediaProvider
/// 的系统 FUSE 由 AOSP 把该值调到 40，这就是两者持续写差距的主因。真机
/// 2026-10-15：max_ratio 1→40 后 1M 持续写 461→1002MB/s、4K 写 559→934MB/s。
/// 每个新连接默认回落为 1，因此必须在每次挂载成功后设置；失败只降级不影响功能。
fn set_bdi_max_ratio(mount_point: &str, identity: &ScopedMountIdentity) {
    const TARGET_RATIO: &str = "40";
    let device = bdi_device_for_mount(identity);
    let Some(device) = device else {
        log::debug!(
            "fuse session bdi device not found mp={mount_point} dev={}",
            identity.dev
        );
        return;
    };
    let sysfs = format!("/sys/class/bdi/{device}/max_ratio");
    if let Err(error) = std::fs::write(&sysfs, TARGET_RATIO) {
        log::debug!("fuse session bdi max_ratio write failed path={sysfs} err={error}");
    } else {
        log::info!("fuse session bdi max_ratio set dev={device} ratio={TARGET_RATIO}");
    }
}

/// 解析本会话挂载对应的 BDI 设备号（形如 `0:142`）。
///
/// 直接用会话登记下来的超级块设备号：BDI 目录名就是 `major:minor`，与 mountinfo 的
/// `dev` 字段一致。此前按挂载源唯一匹配，源原生化之后会匹配到系统 MediaProvider 的连接，
/// 把配额放宽到别人的连接上（自己的连接没放宽，写入吞吐收益丢失）。
///
/// 刻意**不** `stat` 挂载点取 `st_dev`：端点卡死时那次 lookup 会把 daemon 拖进不可中断的
/// D 态，而这只是性能优化，不值得让数据面故障升级成守护进程故障。
fn bdi_device_for_mount(identity: &ScopedMountIdentity) -> Option<String> {
    (!identity.dev.is_empty()).then(|| identity.dev.clone())
}

fn pin_current_thread_to_big_cores() {
    let Some(mask) = big_core_cpu_mask() else {
        log::warn!("fuse session big core detect failed — pin skipped");
        return;
    };
    // SAFETY: cpu_set_t 为纯 POD 位图结构，全零初始化是合法的空 CPU 集合。
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    for bit in 0..64u32 {
        if mask & (1 << bit) != 0 {
            // SAFETY: bit < 64 < CPU_SETSIZE，set 已零初始化，CPU_SET 写入范围内。
            unsafe { libc::CPU_SET(bit as usize, &mut set) };
        }
    }
    // SAFETY: set 为完整初始化的 cpu_set_t，size 与类型匹配。
    let result =
        unsafe { libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set) };
    if result != 0 {
        // 绑定失败只降级为默认调度（部分机型热管控会动态重置核掩码，属正常现象）。
        log::warn!("fuse session big core pin failed mask={mask:#x}");
    } else {
        log::debug!("fuse session big core pin mask={mask:#x}");
    }
}

/// 宿主会话就绪阶段码。
///
/// 负值表示失败阶段，`0` 表示就绪。父进程据此把"未就绪"还原成具体阶段，避免只看到
/// 一个无法定位的现象。
pub const HOST_READY_DIR_FAILED: i32 = -10;
pub const HOST_READY_POLICY_FAILED: i32 = -20;
pub const HOST_READY_MOUNT_FAILED: i32 = -30;
pub const HOST_READY_STABILITY_FAILED: i32 = -40;
pub const HOST_READY_SHARED_FAILED: i32 = -50;

/// 把宿主就绪阶段码翻译成可读阶段名，供父进程日志与阶段文件对齐。
pub fn host_ready_stage(code: i32) -> &'static str {
    match code {
        0 => "ready",
        HOST_READY_DIR_FAILED => "dir",
        HOST_READY_POLICY_FAILED => "policy",
        HOST_READY_MOUNT_FAILED => "mount",
        HOST_READY_STABILITY_FAILED => "stability",
        HOST_READY_SHARED_FAILED => "shared",
        _ => "unknown",
    }
}

/// 在指定宿主挂载点挂一个共享 FUSE 会话,并把它设为 shared propagation。
///
/// 宿主会话是**唯一**的 FUSE 载体：它不跟随任何应用生命周期，挂载点由调用方指定（模块
/// 私有目录，而非存储别名），挂载源用 `srx_fuse_host` 前缀，挂载稳定后额外把宿主挂载点设为
/// shared，供后续各应用 namespace 用移动挂载引用；会话常驻直到会话线程结束。宿主挂载点在
/// daemon 自己的私有 namespace 里，对应用不可见。
// quality-allow(lint-suppression): 宿主会话挂载函数仅在 daemon 二进制路径使用,lib 目标不调用。
#[allow(dead_code)]
pub fn mount_host_fuse(
    config: FuseRedirectConfig,
    host_mount_point: &str,
    ready_sock: Option<libc::c_int>,
    control_sock: Option<libc::c_int>,
) -> bool {
    let session_mount_source = host_mount_source(std::process::id());

    // 看门狗连接识别：宿主 mount namespace 里 spawn 前已存在的 FUSE 连接（MediaProvider
    // 等系统连接）与本会话无关；spawn 后新增的那个才是本会话的连接，sysfs 的 waiting
    // 计数按它采样。/sys/fs/fuse/connections 按 mount namespace 可见，宿主子进程恰好
    // 是唯一能稳定看到本会话连接的地方（游离挂载不出现在 daemon 命名空间）。
    let connections_before = list_fuse_connections();

    let mount_uid = if crate::metadata_repair::enabled() {
        config.uid
    } else {
        -1
    };
    if !fs::create_directory(host_mount_point, mount_uid) {
        log::error!("fuse host mount point missing: {}", host_mount_point);
        crate::fuse_host::host_stage("dir_failed");
        send_ready_result(ready_sock, HOST_READY_DIR_FAILED);
        return false;
    }
    crate::fuse_host::host_stage("dir_ok");

    let fs = match super::FuseRedirectFs::new(config) {
        Some(fs) => fs,
        None => {
            log::error!("fuse host policy init failed mp={}", host_mount_point);
            crate::fuse_host::host_stage("policy_failed");
            send_ready_result(ready_sock, HOST_READY_POLICY_FAILED);
            return false;
        }
    };
    // 交出 `fs` 之前取出按 uid 策略表句柄：宿主会话运行期靠它接收应用策略。
    let policy_table = fs.policy_table();
    crate::fuse_host::host_stage("policy_ok");
    // 与 scoped 会话同口径：宿主事件循环线程同样继承大核亲和性。
    pin_current_thread_to_big_cores();
    let mut mount_options = fuser::Config::default();
    // 与系统 MediaProvider 的挂载选项逐项对齐：应用把自己的存储根与同目录下的其它
    // MediaProvider 视图（`/storage/emulated`、`/mnt/*/0/emulated`）放在一起看时，
    // `nosuid,nodev,noexec,noatime` 与超级选项 `lazytime` 必须一模一样——少任何一项
    // 本身就是"这个挂载不是系统建的"的判据（`lazytime` 是通过 vendored fuser 的
    // `MountOption::LazyTime` 传的）。绑定到应用命名空间的视图与别名 bind 会继承这份
    // mnt_flags，因此只在这里配一次。
    mount_options.mount_options = vec![
        MountOption::FSName(session_mount_source.clone()),
        MountOption::RW,
        MountOption::NoSuid,
        MountOption::NoDev,
        MountOption::NoExec,
        MountOption::NoAtime,
        MountOption::LazyTime,
        MountOption::Async,
    ];
    mount_options.acl = SessionACL::All;
    // worker 池取 16：宿主服务全部接入应用，worker 池是停摆的第一道结构防线——
    // 任一 worker 永久阻塞即永久占坑，池越深同等数量的阻塞点越难把服务拖停。
    // 线程成本为栈虚拟内存与少量内核对象，宿主 RSS 主导项是分配器而非线程。
    mount_options.n_threads = Some(16);
    mount_options.clone_fd = true;

    // 会话退出事件：vendor 的 fuser-bg 线程返回时写一字节。有了这条边沿，宿主服务
    // 循环不必再轮询 `guard.is_finished()`，空闲时可以一直睡到下一个定时任务截止点。
    // SAFETY: eventfd 只创建内核对象并返回 fd；标志均为合法常量。
    let exit_event_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    // RAII：离开函数时关闭该 fd（会话线程此时已被 join，不存在写到复用 fd 的竞态）。
    let _exit_guard = OwnedFd(exit_event_fd);
    if exit_event_fd < 0 {
        log::warn!(
            "fuse host exit eventfd unavailable, fallback to polling session state errno={}",
            last_errno()
        );
    }
    let background = match if exit_event_fd >= 0 {
        fuser::spawn_mount2_with_exit_notify(fs, host_mount_point, &mount_options, exit_event_fd)
    } else {
        fuser::spawn_mount2(fs, host_mount_point, &mount_options)
    } {
        Ok(background) => background,
        Err(error) => {
            crate::fuse_host::host_stage("mount_failed");
            send_ready_result(ready_sock, HOST_READY_MOUNT_FAILED);
            log::warn!(
                "fuse host mount failed mp={} err={}",
                host_mount_point,
                error
            );
            return false;
        }
    };
    crate::fuse_host::host_stage("mount_ok");
    let watchdog_conn = {
        let after = list_fuse_connections();
        let added: Vec<_> = after
            .iter()
            .filter(|n| !connections_before.contains(n))
            .collect();
        match added.len() {
            1 => {
                // 宿主子进程的 info 级日志不进 running.log（仅 warn 以上转发），armed
                // 是看门狗可用性的唯一落盘证据，按 warn 记录。
                log::warn!(
                    "fuse host watchdog armed conn={} waiting={}",
                    added[0],
                    read_fuse_waiting(*added[0]).unwrap_or(0)
                );
                Some(*added[0])
            }
            _ => {
                // 识别失败不阻止会话建立，只降级为无看门狗（与旧行为一致）。
                log::warn!(
                    "fuse host watchdog conn identify failed added={} total={}",
                    added.len(),
                    after.len()
                );
                None
            }
        }
    };

    let Some(identity) = wait_for_stable_session_mount(host_mount_point) else {
        log::warn!(
            "fuse host mount not stable mp={} source={}",
            host_mount_point,
            session_mount_source
        );
        let identity = ScopedMountIdentity {
            mount_id: 0,
            dev: String::new(),
        };
        finish_background_session(background, host_mount_point, false, Some(&identity));
        crate::fuse_host::host_stage("stable_failed");
        send_ready_result(ready_sock, HOST_READY_STABILITY_FAILED);
        return false;
    };
    crate::fuse_host::host_stage("stable_ok");
    // 与 scoped 会话同口径：宿主连接的 BDI 脏页配额同样默认被限为 1%。
    set_bdi_max_ratio(host_mount_point, &identity);

    // 宿主挂载点必须显式设为 shared propagation：`MS_REC|MS_PRIVATE` 只切断向外传播，
    // 不会让子 namespace 得见该挂载；缺了 shared，应用侧的 MS_BIND 会静默失败。参考
    // huniangitb/Fuse-Proxy 的 ns_make_shared。
    let Ok(c_point) = CString::new(host_mount_point) else {
        log::error!(
            "fuse host shared propagation path invalid mp={}",
            host_mount_point
        );
        finish_background_session(background, host_mount_point, false, Some(&identity));
        crate::fuse_host::host_stage("shared_path_invalid");
        send_ready_result(ready_sock, HOST_READY_SHARED_FAILED);
        return false;
    };
    // SAFETY: c_point 是合法 NUL 结尾路径，且在调用期间保持存活。
    if unsafe {
        libc::mount(
            std::ptr::null(),
            c_point.as_ptr(),
            std::ptr::null(),
            libc::MS_SHARED,
            std::ptr::null(),
        )
    } != 0
    {
        let errno = last_errno();
        log::warn!(
            "fuse host shared propagation failed mp={} errno={} {}",
            host_mount_point,
            errno,
            crate::platform::errno::text(errno)
        );
        // 没有 shared propagation 时，应用侧 `MS_BIND` 引用不到宿主挂载：这种状态对外
        // 表现为"宿主已建立但所有应用接入静默失败"，比直接判定失败更难排查，因此这里
        // 明确按失败上报，让 daemon 退回 scoped FUSE。
        finish_background_session(background, host_mount_point, false, Some(&identity));
        crate::fuse_host::host_stage("shared_failed");
        send_ready_result(ready_sock, HOST_READY_SHARED_FAILED);
        return false;
    }
    crate::fuse_host::host_stage("shared_ok");

    log::info!(
        "fuse host session mount registered mp={} mount_id={} dev={} source={}",
        host_mount_point,
        identity.mount_id,
        identity.dev,
        session_mount_source
    );
    crate::fuse_host::host_stage("ready");
    send_ready_result(ready_sock, 0);

    // 就绪之后再启动按 uid 策略的控制通道：登记请求必须落在已挂载的会话上，否则策略建好却
    // 没有会话可服务。控制线程与 FUSE 会话同生命周期，随子进程退出而结束。
    if let Some(control_sock) = control_sock {
        crate::fuse_host::spawn_host_control_loop(control_sock, policy_table);
    }

    // 宿主会话常驻：不跟随任何应用生命周期，只等会话线程结束。服务循环顺带
    // 按节流把分配器空闲页归还内核——宿主承接全部接入应用的 FUSE 服务，媒体
    // 扫描等突发会把 primary 峰值滞留在 RSS（真机实测单风暴即涨至 60MB 级，
    // 也是用户侧"常驻内存几百兆"的主要来源），5 分钟级归还在静息期完成。
    let mut last_purge_ms = crate::platform::paths::monotonic_ms();
    let mut last_anchor_check_ms = crate::platform::paths::monotonic_ms();
    spawn_host_stall_watchdog(watchdog_conn);
    if let Some(heartbeat_conn) = watchdog_conn {
        spawn_host_heartbeat_writer(heartbeat_conn);
    }
    // 服务循环：**事件驱动**。三件事（会话结束、锚点自愈、空闲页归还）里有两件是定时
    // 任务、一件是外部事件，因此唯一正确的等待方式是「睡到最近的截止点，或睡到事件到达」。
    //
    // 会话结束由 vendor 的 `spawn_with_exit_notify` 通过 [`OwnedFd`] 通知；若无
    // eventfd（创建失败），退回按 [`HOST_SESSION_TICK_MS`] 探询 `guard.is_finished()`。
    // 活动边沿 eventfd：请求「从空闲进入在途」与「在途清零」时各写一次。锚点校验与
    // 空闲页归还都挂在这条边沿上——它们只在「真的要服务 I/O」时有意义，因此完全不需要
    // 独立定时器。空闲（没有任何在途请求）时服务循环以无限超时阻塞。
    // SAFETY: eventfd 只创建内核对象并返回 fd；标志均为合法常量。
    let wake_event_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    let _wake_guard = ActivityWakeGuard(wake_event_fd);
    if wake_event_fd >= 0 {
        fuser::activity::set_wake_eventfd(wake_event_fd);
    }
    let wake_epfd = if exit_event_fd >= 0 && wake_event_fd >= 0 {
        // SAFETY: epoll_create1(0) 只创建内核对象；epoll_ctl 挂本函数创建的合法 fd。
        let epfd = unsafe { libc::epoll_create1(0) };
        if epfd >= 0 {
            let mut exit_event = libc::epoll_event {
                events: libc::EPOLLIN as u32,
                u64: EPollTag::Exit as u64,
            };
            let mut wake_event = libc::epoll_event {
                events: libc::EPOLLIN as u32,
                u64: EPollTag::Wake as u64,
            };
            // SAFETY: epfd、exit_event_fd 都是本函数刚创建并校验过返回值的合法 fd，
            // epoll_event 是栈上结构体、生命周期覆盖本次调用。
            let added_exit = unsafe {
                libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, exit_event_fd, &mut exit_event)
            };
            // SAFETY: 同上；wake_event_fd 与 wake_event 也都是本函数创建的合法对象。
            let added_wake = unsafe {
                libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, wake_event_fd, &mut wake_event)
            };
            if added_exit == 0 && added_wake == 0 {
                Some(epfd)
            } else {
                // SAFETY: epfd 由本函数创建，失败路径的清理点。
                unsafe { libc::close(epfd) };
                None
            }
        } else {
            None
        }
    } else {
        None
    };
    let _wake_epfd_guard = OwnedFd(wake_epfd.unwrap_or(-1));
    // 「自上次归还以来真的分配过」：决定空闲期是否需要为一次归还而醒来。
    //
    // 归还空闲页只在**刚发生过批量分配**之后才有意义；若空闲期也按 5 分钟醒一次，
    // 等于用一次唤醒换一次空操作。反过来，如果完全不醒，一轮突发之后的 RSS 会永久
    // 滞留在峰值（用户侧「常驻内存几百兆」）。因此只在「有过 I/O」时安排一次归还。
    let mut purge_pending = false;
    loop {
        // 只有没有事件源时才需要探询线程状态。
        if _wake_epfd_guard.0 < 0 && background.guard.is_finished() {
            return finish_background_session(background, host_mount_point, false, Some(&identity));
        }
        let now_ms = crate::platform::paths::monotonic_ms();
        let in_flight = fuser::activity::in_flight();
        if in_flight > 0 {
            // 有在途请求 = 正在分配：安排一次突发结束后的归还。
            purge_pending = true;
        }
        if now_ms.saturating_sub(last_purge_ms)
            >= crate::platform::allocator::QUIESCENT_PURGE_INTERVAL_MS
        {
            last_purge_ms = now_ms;
            crate::platform::allocator::release_free_pages();
            purge_pending = false;
        }
        // 锚点存在性周期校验（10 秒，降低窗口期）：数据面 real_root 现在依赖锚点 bind，
        // 而 gen4 冻结现场实证 bind 会静默消失（mount info 无条目；根因为 MS_BIND 复制
        // 共享传播组成员时新 bind 加入同一 peer 组、系统侧 umount 传播连带卸载，已在
        // establish 内 MS_PRIVATE 私有化根治）。周期校验作为最后防线，捕获 MS_PRIVATE
        // 私有化未能覆盖的边缘场景（如内核 bug、外部 umount 等）。锚点缺失时数据面
        // 不会死锁（plain 目录只是 ENOENT），但共享存储内容会对应用整体消失，属静默
        // 数据故障，必须自愈。校验只读 mountinfo 账本，不触发任何 FUSE I/O。
        if now_ms.saturating_sub(last_anchor_check_ms) >= HOST_ANCHOR_CHECK_INTERVAL_MS {
            last_anchor_check_ms = now_ms;
            if !crate::fuse_host::media_real_anchor_is_valid()
                && let Some(anchor) = paths::media_real_anchor_root()
            {
                forensics_log(&format!(
                    "fuse host media anchor invalid runtime anchor={anchor} topmost-at-source={:?} re-establishing",
                    crate::mount_ledger::topmost_live_mount(
                        0,
                        &paths::data_media_user_root_for_user(0)
                    )
                    .map(|live| (live.mount_id, live.fs_type, live.source))
                ));
                let rebuilt = crate::fuse_host::establish_media_real_anchor();
                let mounted = crate::fuse_host::media_real_anchor_is_valid();
                if rebuilt && mounted {
                    forensics_log(&format!(
                        "fuse host media anchor re-established runtime anchor={anchor}"
                    ));
                } else {
                    forensics_log(&format!(
                        "fuse host media anchor re-establish failed anchor={anchor} action=terminate-host"
                    ));
                    // 锚点重建失败表明底层存储或挂载系统异常；结束本次会话由既有
                    // generation/pidfd 流程重建，而不是回退到已知会触发自再入死锁的
                    // /data/media/0 直连。
                    return finish_background_session(
                        background,
                        host_mount_point,
                        false,
                        Some(&identity),
                    );
                }
            }
        }
        // 睡到「下一个截止点」或者「会话退出事件」，两者谁先到就醒谁。
        //
        // 会话结束现在有真正的边沿（vendor 在 fuser-bg 线程返回时写 eventfd），因此
        // 空闲期不需要任何定期探询：下一次唤醒就是 10 秒后的锚点校验。没有 eventfd
        // 的退化路径才按 [`HOST_SESSION_TICK_MS`] 探询线程状态。
        let wait_now_ms = crate::platform::paths::monotonic_ms();
        let deadline_ms = HOST_ANCHOR_CHECK_INTERVAL_MS
            .saturating_sub(wait_now_ms.saturating_sub(last_anchor_check_ms))
            .min(
                crate::platform::allocator::QUIESCENT_PURGE_INTERVAL_MS
                    .saturating_sub(wait_now_ms.saturating_sub(last_purge_ms)),
            )
            .max(1);
        if _wake_epfd_guard.0 >= 0 {
            // 空闲（无在途请求）时无限期阻塞：唤醒只可能来自「请求边沿」或「会话退出」。
            // 有在途请求时才按截止点醒来做锚点/归还检查——这正是「有活才醒」。
            // 唯一的例外是「上一轮活动还欠一次归还」：那时睡到归还截止点醒一次，做完
            // 就回到无限期阻塞（一轮突发只多一次唤醒）。
            let timeout_ms = if in_flight > 0 {
                deadline_ms.min(i32::MAX as i64) as i32
            } else if purge_pending {
                crate::platform::allocator::QUIESCENT_PURGE_INTERVAL_MS
                    .saturating_sub(now_ms.saturating_sub(last_purge_ms))
                    .clamp(1, crate::platform::allocator::QUIESCENT_PURGE_INTERVAL_MS)
                    .min(i32::MAX as i64) as i32
            } else {
                // 空闲上限不是无限：边沿只覆盖「有请求被取走」，若边沿通路本身失效
                // （eventfd 被替换、vendor 侧写入失败），无限等待会让锚点自愈与空闲页
                // 归还永久停摆。10 分钟一次（0.0017 Hz）属于纯保险，代价可忽略。
                HOST_SESSION_IDLE_MAX_MS
            };
            let mut events = [
                libc::epoll_event { events: 0, u64: 0 },
                libc::epoll_event { events: 0, u64: 0 },
            ];
            // SAFETY: epfd 与 events 均为本作用域拥有的合法对象，超时已算好。
            let ready =
                unsafe { libc::epoll_wait(_wake_epfd_guard.0, events.as_mut_ptr(), 2, timeout_ms) };
            if ready > 0 {
                let mut exit_fired = false;
                for event in events.iter().take(ready as usize) {
                    if event.u64 == EPollTag::Exit as u64 {
                        exit_fired = true;
                    } else {
                        drain_eventfd(wake_event_fd);
                    }
                }
                if exit_fired {
                    drain_eventfd(exit_event_fd);
                    log::info!("fuse host session loop exited mp={}", host_mount_point);
                    return finish_background_session(
                        background,
                        host_mount_point,
                        false,
                        Some(&identity),
                    );
                }
            }
        } else {
            std::thread::sleep(std::time::Duration::from_millis(
                deadline_ms.min(HOST_SESSION_TICK_MS) as u64,
            ));
        }
    }
}

/// 服务循环 epoll 的事件标签。
#[repr(u64)]
enum EPollTag {
    Exit = 1,
    Wake = 2,
}

/// 排空一个非阻塞 eventfd 的累计计数。
fn drain_eventfd(fd: libc::c_int) {
    if fd < 0 {
        return;
    }
    loop {
        let mut value: u64 = 0;
        // SAFETY: fd 是本进程创建的 eventfd，读 8 字节计数是其契约。
        let n = unsafe {
            libc::read(
                fd,
                &mut value as *mut u64 as *mut libc::c_void,
                std::mem::size_of::<u64>(),
            )
        };
        if n <= 0 {
            return;
        }
    }
}

/// 活动边沿 eventfd 的独占持有：离开作用域时**先解绑再关闭**。
///
/// 顺序很重要：vendor 侧保存的是裸 fd 号，若先关闭再留着号码，后续请求完成时的边沿
/// 写入会落到一个可能已被复用的 fd 上（写 8 字节到别人的管道/socket 就是数据损坏）。
struct ActivityWakeGuard(libc::c_int);

impl Drop for ActivityWakeGuard {
    fn drop(&mut self) {
        fuser::activity::set_wake_eventfd(-1);
        if self.0 >= 0 {
            // SAFETY: fd 由本结构体独占持有，且已先从 vendor 侧解绑。
            unsafe { libc::close(self.0) };
        }
    }
}

/// 独占的 fd（eventfd / epoll 实例）：离开作用域时关闭。
///
/// 会话结束 eventfd 与 epoll 实例的持有语义完全一致，无需分成两个类型。
struct OwnedFd(libc::c_int);

impl Drop for OwnedFd {
    fn drop(&mut self) {
        if self.0 >= 0 {
            // SAFETY: fd 由本结构体独占持有，此处是唯一关闭点。
            unsafe { libc::close(self.0) };
        }
    }
}

/// 服务循环空闲期的最大等待时长（毫秒）：纯保险，正常路径靠边沿唤醒。
const HOST_SESSION_IDLE_MAX_MS: i32 = 600_000;

/// 服务循环的退化探询节拍（毫秒）。
///
/// 只有 eventfd/epoll 创建失败时才会用到：那时无法收到「会话结束」的边沿，只能按这个
/// 节拍查一次 `guard.is_finished()`。正常路径下这个常量不参与任何唤醒。
const HOST_SESSION_TICK_MS: i64 = 1_000;

/// 停摆判定的「无请求完成」时长：单请求处理超过该值在 f2fs 直连后端上已属异常
/// （常规读写为毫秒级），配合 waiting 积压即为停摆候选。取 6 秒在快速确认与
/// 「极端慢操作」误判之间平衡。
const HOST_STALL_NO_COMPLETION_MS: i64 = 6_000;
/// 端到端探测超时：探测子在超时内返回说明会话仍能服务（慢而活着），超时才判定
/// 停摆。取 10 秒——真实停摆必然超时，而极端慢操作通常在此窗口内完成。
const HOST_STALL_PROBE_TIMEOUT_MS: u32 = 10_000;
/// 探测超时后的二次确认窗：探测失败到 abort 之间再等该时长，期间若 waiting 清零
/// 或有请求完成则回到正常。覆盖「全 worker 忙于一个极端慢操作」的误判形态。
const HOST_STALL_CONFIRM_MS: i64 = 4_000;
/// 探测节流：一次探测后至少间隔该时长才允许再次探测，避免持续慢负载下反复 fork。
const HOST_STALL_PROBE_THROTTLE_MS: i64 = 30_000;
/// 看门狗事件循环的等待超时：活跃期的检查节拍。
const HOST_STALL_EPOLL_TIMEOUT_MS: i32 = 1_000;
/// 空闲期（无在途、无近期完成）的兜底检查节拍。
///
/// 边沿信号只覆盖「worker 取走了请求」，覆盖不了「请求在内核队列里没人取」。这个
/// 节拍是那种形态的唯一发现手段，因此必须是有限值；60 秒 = 0.017 Hz，与心跳同量级。
const HOST_STALL_IDLE_POLL_MS: i32 = 60_000;
/// 最后一个请求完成之后仍保持该节拍的时长。
///
/// 停摆只可能发生在「有请求在飞」的时候：内核队列为空时 `waiting` 恒为 0，读它没有
/// 任何信息量。因此空闲期看门狗改为无限期睡在 eventfd 上（零唤醒），由
/// `fuser::activity` 的 0→1 边沿唤醒；这个宽限窗口覆盖「刚完成、可能还有后续请求
/// 正在排队」的那一小段，避免在活动收尾的瞬间把节拍断掉。
const HOST_STALL_IDLE_GRACE_MS: i64 = 10_000;
/// candidate（未确认）阶段取证的最小间隔。
///
/// 只约束误报路径：真停摆的 confirmed 取证不受它限制。取 10 分钟是为了「一个会话
/// 生命周期内至少留下一份可比对的现场」，同时把误报造成的落盘量压到可忽略。
const HOST_STALL_SCENE_THROTTLE_MS: i64 = 600_000;
/// 取证时单个线程内核栈的最大输出行数。
const HOST_STALL_STACK_LINES: usize = 6;

/// 活跃期的心跳间隔。
///
/// 写的是 `platform::host_beat` 的父子共享页（纯内存写，零系统调用、零磁盘 I/O），
/// 因此这个间隔只决定「frozen 检测的粒度」，不产生任何写放大；共享页不可用时才退回
/// 「写临时文件 + rename」的文件心跳。
///
/// 判据侧：daemon 的陈旧阈值取 `max(EXTERNAL_ABORT_BEAT_STALE_MS=20s, 2 × 本间隔)`，
/// 且只在这条心跳的陈旧截止点醒来；「无请求完成」判定（25 秒）用的是心跳里携带的
/// 采样值，采样只会让判定偏晚，不会提前误判。
const HOST_HEARTBEAT_ACTIVE_INTERVAL_MS: u64 = 5_000;
/// 空闲期的心跳间隔。
///
/// 心跳的唯一职责是「向进程外的观察者证明我还活着」。空闲时没有请求可服务，一次冻结
/// 的代价只是「下一个请求会挂住」，因此不需要 5 秒级的证明粒度；把它拉长到 60 秒，
/// 空闲期这条脉冲就从 0.2 Hz 降到 0.017 Hz。
///
/// 心跳里会带上**本次使用的间隔**（`interval_ms`），daemon 按「2 倍间隔」推算陈旧
/// 阈值，因此间隔变长不会造成误判；而一旦有在途请求/积压，下一轮立刻回到
/// [`HOST_HEARTBEAT_ACTIVE_INTERVAL_MS`]，把发现延迟压回 20 秒。
const HOST_HEARTBEAT_IDLE_INTERVAL_MS: u64 = 60_000;
/// 判定「仍算活跃」的完成宽限：最后一次请求完成之后的这段时间内仍按活跃节奏心跳。
///
/// 必须覆盖 daemon 的「无完成」判据窗（25 秒）加上采样误差，否则突发刚结束就把间隔
/// 拉长，可能让 daemon 用活跃阈值去看一条由空闲间隔写出的心跳。
const HOST_HEARTBEAT_ACTIVE_GRACE_MS: i64 = 30_000;
/// 心跳写失败日志节流：会话拆除窗口内 sysfs/文件系统可能短暂不可写，避免每次心跳刷屏。
const HOST_HEARTBEAT_FAIL_LOG_THROTTLE_MS: i64 = 30_000;
/// 锚点存在性周期校验间隔：私有化后（bind 即 MS_PRIVATE，脱离一切传播组）锚点
/// 理论上不会再被系统侧 umount 传播摘除，本校验降级为最后防线——10 秒粒度在
/// 应用可感知前自愈（缺失只造成 ENOENT 不造成死锁），开销为每 10 秒一次
/// mountinfo 账本查找。
const HOST_ANCHOR_CHECK_INTERVAL_MS: i64 = 10_000;

/// 启动宿主心跳写入线程。
///
/// 线程不经过任何 FUSE 路径（只读 sysfs 计数与进程内原子量、写模块私有目录），
/// 因此宿主 FUSE 停摆时它仍能继续写——daemon 侧兜底监控正是靠「心跳新鲜但
/// 无完成时长超阈值」识别「宿主活着但看门狗失效」。线程随宿主进程退出而结束，
/// 无需退出条件。
fn spawn_host_heartbeat_writer(connection: i32) {
    let spawned = std::thread::Builder::new()
        .name("srx-host-beat".to_string())
        .spawn(move || host_heartbeat_loop(connection));
    if spawned.is_err() {
        // 心跳缺席只影响 daemon 兜底的触发依据之一（其余判据仍在），降级记录即可。
        log::warn!("fuse host heartbeat spawn failed conn={}", connection);
    }
}

/// 宿主心跳主循环：优先写父子共享页；共享页不可用时退回文件心跳（tmp + rename）。
///
/// 共享页字段与文件里的竖线格式一一对应：`host_pid|conn|waiting|last_completion_ms|
/// wall_ms|interval_ms`。`last_completion_ms` 为墙钟口径（与 [`fuser::activity::now_ms`]
/// 同源），daemon 与宿主共享墙钟，可直接计算无完成时长；`boot_id` 只在文件通道里
/// 有意义，用于让 daemon 剔除上次开机的残留文件。
///
/// 间隔**按活跃度自适应**：有在途请求、内核队列有积压、或刚有请求完成时用
/// [`HOST_HEARTBEAT_ACTIVE_INTERVAL_MS`]，否则用
/// [`HOST_HEARTBEAT_IDLE_INTERVAL_MS`]。间隔本身写进心跳，daemon 据此推算陈旧阈值。
fn host_heartbeat_loop(connection: i32) {
    // SAFETY: eventfd 只创建内核对象并返回 fd，标志均为合法常量；返回值在下面统一判负。
    let heartbeat_event_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if heartbeat_event_fd >= 0 {
        fuser::activity::set_heartbeat_eventfd(heartbeat_event_fd);
    }
    let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|text| text.trim().to_string())
        .unwrap_or_default();
    let beat_path = std::path::Path::new(module_paths::FUSE_HOST_HEARTBEAT_FILE);
    let tmp_path =
        std::path::Path::new("/data/adb/modules/storage.redirect.x/tmp/.fuse_host.beat.tmp");
    let mut last_fail_log_ms = 0i64;
    // 上一轮是否处于活跃档：只在「空闲→活跃」边沿唤醒 daemon，不在每次心跳上打扰它。
    let mut was_active = false;
    loop {
        let waiting = read_fuse_waiting(connection).unwrap_or(0);
        let last_completion_ms = fuser::activity::last_completion_ms();
        let wall_ms = fuser::activity::now_ms();
        let in_flight = fuser::activity::in_flight();
        // 活跃度：有在途请求、内核队列有积压、或刚有请求完成。
        let active = in_flight > 0
            || waiting > 0
            || (last_completion_ms > 0
                && wall_ms.saturating_sub(last_completion_ms) < HOST_HEARTBEAT_ACTIVE_GRACE_MS);
        let interval_ms = if active {
            HOST_HEARTBEAT_ACTIVE_INTERVAL_MS
        } else {
            HOST_HEARTBEAT_IDLE_INTERVAL_MS
        };
        if active && !was_active {
            // 空闲→活跃：daemon 此刻可能正按「空闲间隔 × 2」的长时限睡着，必须立刻
            // 叫醒它改按活跃阈值评估，否则活跃期冻结的发现会被推迟到 120 秒。
            crate::platform::host_beat::signal_activity();
        }
        was_active = active;
        // 共享页优先：daemon fork 之前建立的同一页映射，写入是纯内存操作，不产生任何
        // 系统调用与磁盘 I/O（模块唯一一处空闲期落盘就此消失）。共享页不可用时退回
        // 文件心跳，语义完全相同。
        let shared_ok = crate::platform::host_beat::write_beat(crate::platform::host_beat::Beat {
            host_pid: std::process::id() as i32,
            conn: connection,
            waiting: waiting as i64,
            last_completion_ms,
            wall_ms,
            interval_ms: interval_ms as i64,
        });
        if shared_ok {
            wait_for_heartbeat_activity(heartbeat_event_fd, interval_ms);
            continue;
        }
        let line = format!(
            "srx_beat|{}|{}|{}|{}|{}|{}|{}",
            boot_id,
            std::process::id(),
            connection,
            waiting,
            last_completion_ms,
            wall_ms,
            interval_ms
        );
        let write_result = (|| -> io::Result<()> {
            std::fs::write(tmp_path, line.as_bytes())?;
            std::fs::rename(tmp_path, beat_path)
        })();
        if let Err(error) = write_result {
            let now = fuser::activity::now_ms();
            if now.saturating_sub(last_fail_log_ms) >= HOST_HEARTBEAT_FAIL_LOG_THROTTLE_MS {
                last_fail_log_ms = now;
                log::warn!(
                    "fuse host heartbeat write failed conn={} error={}",
                    connection,
                    error
                );
            }
        }
        wait_for_heartbeat_activity(heartbeat_event_fd, interval_ms);
    }
}

fn wait_for_heartbeat_activity(fd: i32, interval_ms: u64) {
    if fd < 0 {
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
        return;
    }
    let mut poll_fd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: poll_fd 是栈上结构体，n=1 与实参一致；超时已限幅到 i32 可表示范围。
    let ready = unsafe { libc::poll(&mut poll_fd, 1, interval_ms.min(i32::MAX as u64) as i32) };
    if ready > 0 && (poll_fd.revents & libc::POLLIN) != 0 {
        let mut value = 0u64;
        // SAFETY: fd 是本函数开头校验过的读端，value 是栈上 u64，读取长度与之相同。
        unsafe {
            libc::read(
                fd,
                &mut value as *mut u64 as *mut libc::c_void,
                std::mem::size_of::<u64>(),
            );
        }
    }
}

/// 启动宿主停摆看门狗线程。
///
/// 线程阻塞在 eventfd 的 epoll 上：worker 只在「在途从 0 变 1」与「在途清零」两条
/// 边沿写一次 eventfd（见 `fuser::activity`），连续大流量下不再每请求写一次；空闲时
/// 看门狗睡在 epoll_wait（零 CPU），判定依赖 1 秒节拍的 `waiting` 读取与完成时刻，
/// 因此发现延迟是「无完成时长」本身，不是轮询间隔。eventfd 由本函数创建并注入会话。
fn spawn_host_stall_watchdog(connection: Option<i32>) {
    let Some(connection) = connection else {
        return;
    };
    // SAFETY: eventfd(0, flags) 只创建内核对象并返回 fd；标志均为合法常量。
    let event_fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if event_fd < 0 {
        log::warn!(
            "fuse host watchdog eventfd create failed errno={}",
            last_errno()
        );
        return;
    }
    fuser::activity::set_completion_eventfd(event_fd);
    let spawned = std::thread::Builder::new()
        .name("srx-stall-watchdog".to_string())
        .spawn(move || {
            host_stall_watchdog_main(connection, event_fd);
            // SAFETY: event_fd 由本闭包创建、仅本线程使用，退出前是唯一清理点。
            unsafe { libc::close(event_fd) };
        });
    if spawned.is_err() {
        fuser::activity::set_completion_eventfd(-1);
        // SAFETY: spawn 失败后 event_fd 无使用者，由本处关闭。
        unsafe { libc::close(event_fd) };
        log::warn!("fuse host watchdog spawn failed");
    }
}

/// 看门狗主体。
///
/// 判定链（全部信号都不经过 FUSE，看门狗自身不可能被停摆卡死）：
/// 1. eventfd 停止唤醒且 sysfs `waiting` 积压——请求在内核无人取走；
/// 2. 距上次请求完成超过 [`HOST_STALL_NO_COMPLETION_MS`]——worker 全部阻塞，
///    不是「忙而慢」；
/// 3. 端到端探测超时——宿主挂载点对一次 stat 请求都无法在窗口内应答。
///
/// 三条同时成立才 abort：内核把在途与后续请求全部置错（应用拿到 EIO 而非无限
/// 挂死），宿主随会话退出，daemon 的 pidfd 换代通道重建新会话。
fn host_stall_watchdog_main(connection: i32, event_fd: i32) {
    // SAFETY: epoll_create1(0) 只创建内核对象；epoll_ctl 挂载本函数创建的合法 fd。
    let epfd = unsafe { libc::epoll_create1(0) };
    if epfd < 0 {
        log::warn!(
            "fuse host watchdog epoll create failed errno={}",
            last_errno()
        );
        return;
    }
    let mut event = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: 0,
    };
    // SAFETY: epfd 与 event_fd 均为本函数持有的合法 fd，event 指向栈上有效内存。
    if unsafe { libc::epoll_ctl(epfd, libc::EPOLL_CTL_ADD, event_fd, &mut event) } != 0 {
        log::warn!("fuse host watchdog epoll add failed errno={}", last_errno());
        // SAFETY: epfd 由本函数创建，失败路径的清理点。
        unsafe { libc::close(epfd) };
        return;
    }

    // 看门狗启动时刻：作为「本会话从未完成过任何请求」时的无完成时长基线。
    // 缺了它，`last_completion_ms == 0` 会让 `now - 0` 变成一个天文数字（真机实测
    // 1794191806031 ms），于是会话刚建立、第一个请求还在飞时就必然被判成候选停摆
    // ——每次开机都白白做一次取证 + 一次探针 fork。
    let watchdog_start_ms = fuser::activity::now_ms();
    let mut last_check_ms = 0i64;
    let mut last_probe_ms = 0i64; // 本轮停摆事件内是否已做过 candidate 现场取证：candidate 阶段（探测之前）就
    // 必须落盘，因为探测子进程自身可能陷入 D 态拖住看门狗线程——2026-10-29 真机
    // 冻结中 confirmed 永远没到、现场全缺，正是只有 confirmed 才 dump 的漏洞。
    // 条件恢复（stalled 归零）时复位，保证下次停摆重新取证。
    let mut scene_dumped = false;
    // 上一次 candidate 取证的时刻。
    //
    // 取证体积本身就意味着必须限频：真机实测 `/proc/self/mountinfo` 有 700 行、约
    // 130 KiB，而「单个请求慢 20 秒」这类误报在开机与媒体扫描期很常见（实测 10 分钟内
    // 两次，每次 probe 都判定 slow-but-alive）。若每个 candidate 都全量落盘，光日志
    // 就会顶到轮转、把真正的冻结现场冲掉——反而与取证初衷相反。因此 candidate 阶段的
    // 取证按 `HOST_STALL_SCENE_THROTTLE_MS` 限频；confirmed（真停摆）阶段不受限制。
    let mut last_scene_dump_ms = 0i64;
    // 探测超时后的二次确认窗起点：真停摆在该窗内 waiting 不会清零、也无请求完成；
    // 「全 worker 忙慢操作」的极端场景会在窗内完成而回到正常。没有这一步，开机
    // 风暴的大目录遍历可能被误判为停摆。
    let mut confirm_deadline_ms: Option<i64> = None;
    loop {
        let mut events = [event; 1];
        // 等待超时：只有「确实可能有停摆」的时候才按 1 秒节拍醒来。
        //
        // 判据是「有在途请求」「探测/确认进行中」或「刚刚还有请求完成」
        // （[`HOST_STALL_IDLE_GRACE_MS`]）。都不成立时线程以
        // [`HOST_STALL_IDLE_POLL_MS`]（60 秒）超时睡在 eventfd 上——**不是**无限。
        //
        // 为什么留一个 60 秒兜底而不是无限等待：`fuser::activity` 的边沿只覆盖
        // 「worker 取走了请求」（`begin_request` 在 dispatch 之前打点），覆盖不了
        // 「请求已在内核队列里但没有任何 worker 去取」这种形态（分发线程自身被冻结、
        // 内核没能唤醒读端）。那种情况下 `in_flight` 恒为 0、没有任何边沿可等，只有
        // 亲眼看一眼 `waiting` 才发现。60 秒一次 sysfs 读（0.017 Hz）与心跳同量级，
        // 换来「不存在任何无法发现的停摆形态」，值得。
        let idle_now = fuser::activity::now_ms();
        let idle_timeout_ms = if fuser::activity::in_flight() > 0
            || confirm_deadline_ms.is_some()
            || idle_now.saturating_sub(fuser::activity::last_completion_ms())
                < HOST_STALL_IDLE_GRACE_MS
        {
            HOST_STALL_EPOLL_TIMEOUT_MS
        } else {
            HOST_STALL_IDLE_POLL_MS
        };
        // SAFETY: events 是栈上数组，容量 1 与传入的 maxevents=1 一致；epfd 由本线程创建并已校验。
        let ready = unsafe { libc::epoll_wait(epfd, events.as_mut_ptr(), 1, idle_timeout_ms) };
        if ready < 0 {
            if last_errno() == libc::EINTR {
                continue;
            }
            break;
        }
        if ready > 0 {
            // 排空 eventfd 计数：一次 read 清零，非阻塞读到 EAGAIN 为止。
            loop {
                let mut value: u64 = 0;
                // SAFETY: value 是栈上可写 u64，read 到它的完整大小是 eventfd 契约。
                let n = unsafe {
                    libc::read(
                        event_fd,
                        &mut value as *mut u64 as *mut libc::c_void,
                        std::mem::size_of::<u64>(),
                    )
                };
                if n <= 0 {
                    break;
                }
            }
        }
        let now = fuser::activity::now_ms();
        if now.saturating_sub(last_check_ms) < 1_000 {
            continue;
        }
        last_check_ms = now;
        let Some(waiting) = read_fuse_waiting_logged(connection) else {
            // sysfs 读失败通常意味着连接已被 abort/会话退出：看门狗随之结束。
            // 失败必须落日志（含 errno）——静默退出曾让「会话何时消失」变成排查黑洞。
            log::error!(
                "fuse host watchdog sysfs read failed conn={} errno={} watchdog-exit",
                connection,
                last_errno()
            );
            break;
        };
        let last_completion = fuser::activity::last_completion_ms();
        let baseline = if last_completion > 0 {
            last_completion
        } else {
            watchdog_start_ms
        };
        let stalled = waiting > 0;
        let no_completion_ms = if stalled {
            now.saturating_sub(baseline)
        } else {
            0
        };
        if !stalled || no_completion_ms < HOST_STALL_NO_COMPLETION_MS {
            // 恢复（含二次确认窗内自行完成）：回到正常态，下次停摆重新走全链路。
            confirm_deadline_ms = None;
            scene_dumped = false;
            continue;
        }
        if let Some(deadline) = confirm_deadline_ms {
            // 二次确认窗：条件仍成立即放弃本连接。
            if now >= deadline {
                log::error!(
                    "fuse host stall confirmed conn={} waiting={} no_completion_ms={} in_flight={}",
                    connection,
                    waiting,
                    no_completion_ms,
                    fuser::activity::in_flight()
                );
                dump_stall_scene(connection, no_completion_ms, true);
                abort_fuse_connection(connection);
                // abort 后内核把积压请求全部置错，worker 的 read 随即报错退出，宿主
                // 进程结束；daemon 的 pidfd 换代通道重建新会话。看门狗完成使命退出。
                break;
            }
            continue;
        }
        // 停摆候选：先做端到端确认，探测能应答就是「忙而慢」，放行并节流。
        if now.saturating_sub(last_probe_ms) < HOST_STALL_PROBE_THROTTLE_MS {
            continue;
        }
        last_probe_ms = now;
        log::warn!(
            "fuse host stall candidate conn={} waiting={} no_completion_ms={} in_flight={}",
            connection,
            waiting,
            no_completion_ms,
            fuser::activity::in_flight()
        );
        // candidate 阶段立即取证（每轮停摆事件一次）：此刻现场就是根因本身。探测
        // 与确认窗都放在取证之后——探测子进程可能陷入 D 态拖住看门狗线程，若等
        // confirmed 才 dump，真机实证冻结场景下现场会整场缺席。
        if !scene_dumped {
            scene_dumped = true;
            if now.saturating_sub(last_scene_dump_ms) >= HOST_STALL_SCENE_THROTTLE_MS {
                last_scene_dump_ms = now;
                dump_stall_scene(connection, no_completion_ms, false);
            } else {
                log::warn!(
                    "fuse host stall scene skipped (throttled) conn={} no_completion_ms={}",
                    connection,
                    no_completion_ms
                );
            }
        }
        match probe_mount_point_alive(HOST_STALL_PROBE_TIMEOUT_MS) {
            ProbeOutcome::Alive => {
                log::warn!(
                    "fuse host stall probe ok conn={} in_flight={} slow-but-alive",
                    connection,
                    fuser::activity::in_flight()
                );
                continue;
            }
            ProbeOutcome::Unavailable(reason) => {
                // 无法验证（fork 失败等）不能当作停摆证据：宁可漏判交给 daemon 兜底，
                // 也不能在证据不足时 abort 掉可能健康的会话。
                log::warn!(
                    "fuse host stall probe unavailable conn={} reason={}",
                    connection,
                    reason
                );
                continue;
            }
            ProbeOutcome::TimedOut => {}
        }
        // 探测超时：进入二次确认窗（总延迟 ≈ 6s 冻结 + 10s 探测 + 4s 窗口）。
        confirm_deadline_ms = Some(now + HOST_STALL_CONFIRM_MS);
    }
    // SAFETY: epfd 由本函数创建，线程退出是唯一清理点。
    unsafe { libc::close(epfd) };
}

/// 端到端探测结果。
#[derive(Debug, Eq, PartialEq)]
enum ProbeOutcome {
    /// 探测子在超时内退出：连接仍能应答（慢而活着）。
    Alive,
    /// 探测超时：lookup 无限挂起，连接已无法应答。SIGKILL 后有界回收——探针若
    /// 陷入 D 态（TASK_KILLABLE 只在 FUSE 应答或 abort 时醒来）回收会失败，这
    /// 本身就是队列已死的直接证据，按超时上报，探针将由 abort 释放。
    TimedOut,
    /// 探测无法执行（fork 失败等）：不能作为停摆证据。
    Unavailable(&'static str),
}

/// 端到端探测：fork 子进程 stat 宿主挂载点下的探针名。
///
/// 探针名必然不存在——stat 返回 ENOENT 同样是「连接可应答」的证据。刻意不 stat
/// 挂载点本身：根 dentry 常驻且免重验证，stat 挂载根可能不产生任何 FUSE 往返，
/// 会把「全 worker 卡死」误判为存活（用户设备实测的假阴性形态）。
///
/// 探测请求本身也走这条 FUSE 连接，停摆时它必然与其它请求一起挂起。超时路径
/// **绝不阻塞等待**：SIGKILL 之后只在有界窗口内等待退出事件并回收，探针杀不死就记录
/// 现场后放弃——阻塞 waitpid 曾让看门狗线程整体卡死、自愈链路失效
/// （2026-10-29 真机冻结实证）。
fn probe_mount_point_alive(timeout_ms: u32) -> ProbeOutcome {
    crate::logging::prepare_for_fork();
    // SAFETY: fork 前已完成日志准备；子进程只做一次 stat 后立即 _exit。
    let child = unsafe { libc::fork() };
    if child < 0 {
        return ProbeOutcome::Unavailable("fork-failed");
    }
    if child == 0 {
        // SAFETY: c_path 指向 NUL 结尾静态串；buf 指向栈上未初始化内存，stat 返回
        // 时由内核完整写入。结果不检查：ENOENT 证明 lookup 有应答，其它 errno
        // 同样证明连接活着。
        unsafe {
            if let Ok(c_path) = CString::new(format!(
                "{}/.srx_stall_probe",
                crate::platform::module_paths::FUSE_HOST_MOUNT_DIR
            )) {
                let mut buf = std::mem::MaybeUninit::<libc::stat>::uninit();
                libc::stat(c_path.as_ptr(), buf.as_mut_ptr());
            }
            libc::_exit(0);
        }
    }
    use crate::platform::child_wait::{self, ChildExit};
    match child_wait::wait_child_exit(child, i64::from(timeout_ms)) {
        ChildExit::TimedOut => {
            // SAFETY: child 已确认仍存活（未回收）；SIGKILL 只作用于它。
            unsafe { libc::kill(child, libc::SIGKILL) };
            reap_probe_child_bounded(child);
            ProbeOutcome::TimedOut
        }
        ChildExit::Failed(_) => ProbeOutcome::Unavailable("waitpid-failed"),
        ChildExit::Exited(_) | ChildExit::Signaled(_) => ProbeOutcome::Alive,
    }
}

/// SIGKILL 之后的有界回收：最多等 2 秒退出事件，绝不阻塞看门狗线程。
///
/// 回收失败（探针陷入 D 态）不是错误路径：探针卡死在 FUSE 请求上正是停摆证据
/// 的一部分，把它连同进程状态一起落日志，随后由连接 abort 释放。
fn reap_probe_child_bounded(child: i32) {
    const REAP_DEADLINE_MS: i64 = 2_000;
    if crate::platform::child_wait::wait_child_exit(child, REAP_DEADLINE_MS).is_settled() {
        return;
    }
    // 探针 SIGKILL 后仍未回收：读它的调度状态作为取证（D=不可中断，只能等 FUSE
    // abort 唤醒），随后放弃回收。僵尸由 abort 后的会话退出统一交给 init 兜底。
    let state = std::fs::read_to_string(format!("/proc/{child}/stat"))
        .ok()
        .and_then(|text| {
            // stat 的 comm 字段可含空格与括号，取最后一个 ')' 之后的首字段。
            text.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().nth(1))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string());
    log::error!(
        "fuse host stall probe child unkillable pid={} state={} released-by-abort",
        child,
        state
    );
}

/// 枚举本 mount namespace 可见的 FUSE 连接 id。
fn list_fuse_connections() -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/sys/fs/fuse/connections") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
        .collect()
}

/// 读取指定连接的 waiting 计数（内核队列中等待被用户态取走的请求数）。
fn read_fuse_waiting(connection: i32) -> Option<u64> {
    // **不分配堆**：这条路径属于停摆看门狗，而它要面对的正是「进程可能已经死锁」的
    // 现场——最典型的就是分配器锁被某个卡住的线程占着。`format!` / `read_to_string`
    // 都会走分配器，一旦死锁在里面，看门狗自己也停摆，第一道防线当场失效（2026-10-29
    // 现场正是「看门狗探测链路被一起拖死」）。因此这里只用栈缓冲 + 裸系统调用。
    let path = fuse_sysfs_path(connection, b"waiting")?;
    let mut buffer = [0u8; 32];
    // SAFETY: path 是 NUL 结尾的栈缓冲；open 只读，失败返回负值。
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    // SAFETY: fd 由本函数打开、buffer 为栈上可写区间且长度取自该数组。
    let read_len = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
    // SAFETY: fd 是本函数私有，关闭一次。
    unsafe { libc::close(fd) };
    if read_len <= 0 {
        return None;
    }
    parse_decimal_u64(&buffer[..read_len as usize])
}

/// 把 `/sys/fs/fuse/connections/<conn>/<leaf>` 拼进栈缓冲（无堆分配）。
fn fuse_sysfs_path(connection: i32, leaf: &[u8]) -> Option<[u8; 64]> {
    const PREFIX: &[u8] = b"/sys/fs/fuse/connections/";
    let mut path = [0u8; 64];
    let mut len = 0usize;
    let mut push = |bytes: &[u8]| -> Option<()> {
        let end = len.checked_add(bytes.len())?;
        if end >= path.len() {
            return None;
        }
        path[len..end].copy_from_slice(bytes);
        len = end;
        Some(())
    };
    push(PREFIX)?;
    if connection < 0 {
        return None;
    }
    // 十进制展开，最多 10 位。
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    let mut value = connection as u32;
    loop {
        digits[count] = b'0' + (value % 10) as u8;
        count += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for index in (0..count).rev() {
        push(&[digits[index]])?;
    }
    push(b"/")?;
    push(leaf)?;
    // path 剩余部分是 0，天然 NUL 结尾。
    Some(path)
}

/// 解析十进制（跳过前后空白），**不分配**、不依赖标准库解析器。
fn parse_decimal_u64(bytes: &[u8]) -> Option<u64> {
    let mut value: u64 = 0;
    let mut seen = false;
    for byte in bytes {
        match byte {
            b'0'..=b'9' => {
                value = value.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
                seen = true;
            }
            b' ' | b'\t' | b'\r' | b'\n' => {
                if seen {
                    break;
                }
            }
            _ => return None,
        }
    }
    seen.then_some(value)
}

/// [`read_fuse_waiting`] 的带日志版本：读失败时先落一条含 errno 的 error 再返回
/// None。看门狗判定链用它，保证「连接消失」这一事件本身永远留有证据。
fn read_fuse_waiting_logged(connection: i32) -> Option<u64> {
    let value = read_fuse_waiting(connection);
    if value.is_none() {
        log::error!(
            "fuse host watchdog waiting read failed conn={} errno={}",
            connection,
            last_errno()
        );
    }
    value
}

/// 触发内核对指定连接的 abort：所有在途与后续请求立即以错误完成。
fn abort_fuse_connection(connection: i32) {
    // 同样不分配堆：解冻动作必须能在「堆可能已死锁」的现场执行，否则第一道防线在
    // 最关键的一步上仍然会卡住。写入内容任意（sysfs abort 为触发型节点）。
    let Some(path) = fuse_sysfs_path(connection, b"abort") else {
        log::error!(
            "fuse host stall abort path build failed conn={}",
            connection
        );
        return;
    };
    // SAFETY: path 是 NUL 结尾的栈缓冲；open 只写，失败返回负值。
    let fd = unsafe { libc::open(path.as_ptr().cast(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        log::error!("fuse host stall abort open failed conn={}", connection);
        return;
    }
    // SAFETY: fd 由本函数打开；写入 1 字节常量。
    let written = unsafe { libc::write(fd, b"1".as_ptr().cast(), 1) };
    // SAFETY: fd 是本函数私有，关闭一次。
    unsafe { libc::close(fd) };
    if written <= 0 {
        log::error!("fuse host stall abort write failed conn={}", connection);
    }
}

/// 停摆取证双写落盘：running.log 之外独立追加一份到 [`STALL_FORENSICS_LOG`]。
///
/// running.log 按体积轮转且挂载风暴期每秒可产生数千行——gen4 冻结现场的会话
/// 启动期记录（含锚点建立结果）在数分钟内即被轮出丢失，归因被迫依赖推断。独立
/// 文件只在超过 2 MiB 时就地截断，保证冻结取证与锚点状态永远可回溯。写失败静默
/// （取证尽力而为，不影响主流程）。
pub(crate) fn forensics_log(line: &str) {
    log::error!("{line}");
    append_stall_forensics(line);
}

/// 只写独立取证文件，不在运行日志里以错误级别出现。
///
/// 用于「正常但值得留档」的一次性状态事件（例如宿主数据面锚点建立成功）：它们必须
/// 进 stall-forensics 供事后对照，但不是错误，按 error 级别写进 running.log 会让
/// 现场排障误判为失败。
pub(crate) fn forensics_record(line: &str) {
    append_stall_forensics(line);
}

/// 追加一行（带墙钟时间戳）到独立取证文件；超限先截断再续写。
fn append_stall_forensics(line: &str) {
    use std::io::{Seek, SeekFrom, Write as _};
    use std::os::fd::AsRawFd;
    const MAX_BYTES: u64 = 2 * 1024 * 1024;
    let path = crate::platform::module_paths::STALL_FORENSICS_LOG;
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .open(path)
    else {
        return;
    };
    // 宿主与 daemon 是不同进程并发写同一路径；flock 保证尺寸检查、截断和追加
    // 作为一个临界区执行，避免某进程刚截断后另一进程写入的证据被清空。
    // SAFETY: fd 由 file 持有，锁在 file drop 时随描述符释放。
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return;
    }
    if file.metadata().map(|meta| meta.len()).unwrap_or(0) > MAX_BYTES && file.set_len(0).is_err() {
        return;
    }
    if file.seek(SeekFrom::End(0)).is_err() {
        return;
    }
    let _ = writeln!(file, "{timestamp} {line}");
}

/// 单次场景取证的行数预算。
///
/// 一次场景要覆盖宿主 + 全部 MediaProvider 线程（每线程 comm/wchan/syscall/栈），不设
/// 上限时真机实测一次就是 900 余行、约 110 KiB——而**未确认**的候选场景在开机与媒体
/// 扫描期很常见（实测每 5~10 分钟一次）。这不只是磁盘写放大：它会把 running.log 的
/// 2 MiB 轮转窗口一次吃掉十分之一，真正出问题时反而翻不到更早的记录。200 行足够
/// 覆盖「哪些线程卡在哪个内核函数」这个核心问题。
/// 候选（未确认）场景的行数预算。
///
/// 候选阶段是**高频**的（单个请求慢十几秒就会触发，实测 5~10 分钟一次），而它的结论
/// 绝大多数是「忙而慢」。所以只留足够定位「谁在等、等在哪」的量：锚点、宿主各线程、
/// MediaProvider 前若干线程、D 态名单、连接统计。
const HOST_STALL_SCENE_LINE_BUDGET_CANDIDATE: usize = 80;
/// 确认（真停摆）场景的行数预算。真停摆是灾难级且极罕见，这里可以给足。
const HOST_STALL_SCENE_LINE_BUDGET_CONFIRMED: usize = 600;
/// 当前场景已输出的行数（每次场景开始时清零）。
static SCENE_LINES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// 当前场景的行数预算（候选/确认两档，由 [`dump_stall_scene`] 在开始时设定）。
static SCENE_BUDGET: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(HOST_STALL_SCENE_LINE_BUDGET_CANDIDATE);

/// 场景取证专用落盘：受 [`HOST_STALL_SCENE_LINE_BUDGET`] 约束。
///
/// 与 [`forensics_log`] 分开，是因为后者还被「确认停摆」「锚点建立」等一次性事件使用，
/// 那些不该被场景预算截断。
fn scene_log(line: &str) {
    use std::sync::atomic::Ordering;
    let budget = SCENE_BUDGET.load(Ordering::Relaxed);
    let index = SCENE_LINES.fetch_add(1, Ordering::Relaxed);
    if index < budget {
        forensics_log(line);
    } else if index == budget {
        forensics_log(&format!(
            "fuse host stall scene truncated at_lines={budget}"
        ));
    }
}

/// 停摆取证：把宿主全部线程与 MediaProvider 全部线程的内核栈、等待通道和连接统计
/// 写入运行日志。调用点处于停摆已持续 20 秒的时刻，现场就是根因本身（哪个 worker
/// 卡在哪个内核函数、谁持有不放）。
fn dump_stall_scene(connection: i32, elapsed_ms: i64, detailed: bool) {
    use std::sync::atomic::Ordering;
    SCENE_LINES.store(0, Ordering::Relaxed);
    SCENE_BUDGET.store(
        if detailed {
            HOST_STALL_SCENE_LINE_BUDGET_CONFIRMED
        } else {
            HOST_STALL_SCENE_LINE_BUDGET_CANDIDATE
        },
        Ordering::Relaxed,
    );
    if !detailed {
        scene_log("fuse host stall scene detail=candidate (confirmed 场景才会带完整线程栈)");
    }
    scene_log(&format!(
        "fuse host stall scene begin conn={} elapsed_ms={}",
        connection, elapsed_ms
    ));
    // 锚点状态是停摆归因的关键：有锚点时数据面不经 /data/media/0 原路径，
    // 自再入应另有来源；无锚点时自再入即首选根因。静态登记与实际挂载分别输出：
    // gen4 冻结现场实证两者会背离（静态已登记而 bind 已消失），只报其一将误判。
    match crate::platform::paths::media_real_anchor_root() {
        Some(anchor) => {
            let mounted = crate::mount_ledger::topmost_live_mount(0, anchor).is_some();
            scene_log(&format!(
                "fuse host stall scene anchor present root={anchor} mounted={mounted}"
            ));
        }
        None => scene_log("fuse host stall scene anchor absent"),
    }
    for (label, pid) in [("host", std::process::id() as i32)].into_iter().chain(
        find_media_provider_pids()
            .into_iter()
            .map(|pid| ("mp", pid)),
    ) {
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            scene_log(&format!(
                "fuse host stall scene {label} pid={pid} task-list-failed"
            ));
            continue;
        };
        for task in tasks.flatten().take(64) {
            let tid_path = task.path();
            let tid = task.file_name().to_string_lossy().to_string();
            let comm = std::fs::read_to_string(tid_path.join("comm"))
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            let wchan = std::fs::read_to_string(tid_path.join("wchan"))
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            scene_log(&format!(
                "fuse host stall thread {label} tid={tid} comm={comm} wchan={wchan}"
            ));
            // 当前系统调用号与参数（一线一行）：与 wchan/stack 互补——stack 受
            // ptrace 权限与栈深限制，syscall 能直接给出「卡在哪个内核入口」。
            if let Ok(syscall) = std::fs::read_to_string(tid_path.join("syscall")) {
                let syscall = syscall.trim();
                if !syscall.is_empty() {
                    scene_log(&format!(
                        "fuse host stall syscall {label} tid={tid} {syscall}"
                    ));
                }
            }
            if let Ok(stack) = std::fs::read_to_string(tid_path.join("stack")) {
                for line in stack.lines().take(HOST_STALL_STACK_LINES) {
                    scene_log(&format!("fuse host stall stack {label} tid={tid} {line}"));
                }
            }
        }
        // 进程级 status 关键字段：线程总数与调度状态是判断「全体 D 态」还是
        // 「部分阻塞」的快速依据。
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
            for line in status.lines() {
                if line.starts_with("State:")
                    || line.starts_with("Threads:")
                    || line.starts_with("VmRSS:")
                {
                    scene_log(&format!("fuse host stall status {label} pid={pid} {line}"));
                }
            }
        }
    }
    // 全系统 D 态任务扫描与内核日志尾**只在已确认的停摆里收集**。
    //
    // 两者回答的是同一个问题：「这是底层存储停滞（整机在等 I/O），还是只有本会话
    // 再入？」——候选阶段还没到这一步：那一刻探针子进程（下几步才 fork）往往会判定
    // slow-but-alive。真机实测（2026-11-09）一次候选场景因此写出 80 行场景 + 64 行
    // D 态 + 64 行 kmsg ≈ 41 KiB，而结论是「忙而慢」。这两个集合与行数预算无关（走
    // `forensics_log`），不受 `HOST_STALL_SCENE_LINE_BUDGET_CANDIDATE` 约束，是候选
    // 场景体积失控的真正来源。
    if detailed {
        dump_system_dstate_tasks();
        dump_kmsg_forensics();
    }
    if let Ok(statistics) =
        std::fs::read_to_string(format!("/sys/fs/fuse/connections/{connection}/statistics"))
    {
        for line in statistics.lines() {
            scene_log(&format!("fuse host stall conn-stat {line}"));
        }
    }
    // 宿主命名空间全量挂载表：用户设备第二次停摆实证 worker 卡在 f2fs 直连路径
    // （/data/media/0/Android）的 stat 却走了 fuse_dentry_revalidate，abort 后立即
    // 恢复——宿主 ns 的挂载/dcache 形态是唯一未取证面（此前的死层取证在 attach
    // 子进程即应用 ns）。全量输出避免按关键字过滤漏掉 sdcardfs/overlay 等非 fuse
    // 但参与路径解析的挂载层。
    dump_host_mountinfo();
    scene_log("fuse host stall scene end");
}

/// 宿主命名空间挂载表取证（**有界**）。
///
/// 原始 `/proc/self/mountinfo` 在真机上有 700 行左右（约 130 KiB），其中绝大多数是
/// `debugfs` / `sysfs` / `tmpfs` 的系统内部挂载，与「路径解析走了哪一层」无关。整份
/// 落盘的问题有两个：真机上每次停摆候选都会写出 130 KiB（一次开机就足够把 running.log
/// 顶到轮转），而真正需要的证据只占十几行。
///
/// 因此按「是否可能参与存储路径解析」优先输出：命中 `storage` / `/data` / `fuse` /
/// `emulated` / 模块目录的行先写，其余按剩余额度补足，总量封顶
/// [`HOST_STALL_MOUNTINFO_LINES`]。截断时显式写一行 `truncated`，避免事后误判为
/// 「挂载表就这么长」。
fn dump_host_mountinfo() {
    const HOST_STALL_MOUNTINFO_LINES: usize = 48;
    let Ok(mountinfo) = std::fs::read_to_string("/proc/self/mountinfo") else {
        scene_log("fuse host stall host-mountinfo-unreadable");
        return;
    };
    let relevant = |line: &str| {
        line.contains("storage")
            || line.contains("/data")
            || line.contains("fuse")
            || line.contains("emulated")
    };
    let mut emitted = 0usize;
    let mut total = 0usize;
    for line in mountinfo.lines() {
        total += 1;
        if !relevant(line) {
            continue;
        }
        if emitted >= HOST_STALL_MOUNTINFO_LINES {
            break;
        }
        emitted += 1;
        scene_log(&format!("fuse host stall host-mountinfo {line}"));
    }
    // 剩余额度给非存储相关挂载：保留「未命中关键字但其实参与解析」的兜底（例如
    // overlay / sdcardfs 这类不含上述关键字的层），但绝不无界输出。
    for line in mountinfo.lines() {
        if emitted >= HOST_STALL_MOUNTINFO_LINES {
            break;
        }
        if relevant(line) {
            continue;
        }
        emitted += 1;
        scene_log(&format!("fuse host stall host-mountinfo-other {line}"));
    }
    if emitted < total {
        scene_log(&format!(
            "fuse host stall host-mountinfo truncated emitted={} total={}",
            emitted, total
        ));
    }
}

/// 全系统 D 态任务扫描（有界 64 条）：pid、comm、内核等待通道。
///
/// 只读 /proc，不触发任何文件系统访问，冻结现场下安全。D 态 = 不可中断睡眠，
/// 在存储语境下几乎总是「等一个永远不会来的 I/O 应答」。宿主停摆取证与 daemon
/// 侧外部 abort 兜底取证共用（后者无宿主内 dump 时它是唯一的全系统视图）。
pub(crate) fn dump_system_dstate_tasks() {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        forensics_log("fuse host stall dstate scan failed proc-unreadable");
        return;
    };
    let mut dumped = 0usize;
    for entry in entries.flatten() {
        if dumped >= 64 {
            forensics_log("fuse host stall dstate truncated at=64");
            return;
        }
        let Some(pid) = entry.file_name().to_string_lossy().parse::<i32>().ok() else {
            continue;
        };
        let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
            continue;
        };
        // 内核线程（`cmdline` 为空）天生「永远 D 态」：`wdtk-*`、`hang_detect`、
        // `ccci_poll` 这类在真机上占满整张名单，而它们与存储停摆无关——恰好把真正
        // 需要的用户态任务挤出去（2026-11-09 实测 10 条里 10 条是这类）。cmdline 按
        // pid 惰性读一次，不按键程放大。
        let mut is_kernel_thread: Option<bool> = None;
        for task in tasks.flatten() {
            let tid_path = task.path();
            // /proc/<pid>/stat 第二字段为状态字符；comm 可含空格，从右往左取更稳。
            let Ok(stat) = std::fs::read_to_string(tid_path.join("stat")) else {
                continue;
            };
            let Some(state) = stat
                .rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .map(str::to_string)
            else {
                continue;
            };
            if state != "D" {
                continue;
            }
            let kernel_thread = *is_kernel_thread.get_or_insert_with(|| {
                std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                    .map(|text| text.trim_matches('\0').trim().is_empty())
                    .unwrap_or(true)
            });
            if kernel_thread {
                continue;
            }
            let comm = std::fs::read_to_string(tid_path.join("comm"))
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            let wchan = std::fs::read_to_string(tid_path.join("wchan"))
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            forensics_log(&format!(
                "fuse host stall dstate pid={pid} tid={} comm={comm} wchan={wchan}",
                task.file_name().to_string_lossy()
            ));
            dumped += 1;
            if dumped >= 64 {
                forensics_log("fuse host stall dstate truncated at=64");
                return;
            }
        }
    }
    if dumped == 0 {
        forensics_log("fuse host stall dstate none");
    }
}

/// 内核日志过滤尾（尽力而为）：非阻塞读 /dev/kmsg，只保留与存储停摆相关的行。
///
/// 读不出（权限受限、节点缺失）只留一条 warn，不影响取证其余部分。关键词覆盖：
/// 任务阻塞/hung、f2fs 与块层错误、FUSE、内存回收与 OOM——「底层存储停滞」假设
/// 的直接证据只可能出现在这里，重启后即被覆盖。宿主停摆取证与 daemon 侧外部
/// abort 兜底取证共用。
pub(crate) fn dump_kmsg_forensics() {
    let Ok(c_kmsg) = CString::new("/dev/kmsg") else {
        return;
    };
    // SAFETY: c_kmsg 指向 NUL 结尾静态串；O_RDONLY|O_NONBLOCK|O_CLOEXEC 打开是无副作用操作。
    let fd = unsafe {
        libc::open(
            c_kmsg.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        log::warn!(
            "fuse host stall kmsg open failed errno={} (forensics skipped)",
            last_errno()
        );
        return;
    }
    // 命中判定：**先排除系统噪音，再匹配存储停摆关键词**。
    //
    // 真机实测（2026-11-09）上一版关键词表命中 64 条，`logged=64` 里**没有一条**与
    // 存储相关：`dm-` 是为了盯 device-mapper 报错加进去的，结果把每一条 SELinux 审计
    // 行的 `dev="dm-60"` 都吸了进来；`timeout` 则把 wlan 的省电日志全捞了。这类噪音
    // 除了挤占轮转窗口没有任何价值——它恰好把「真正该看的那几行」挤掉了。
    fn is_stall_kmsg_line(line: &str) -> bool {
        // 与存储停摆无关但高频的系统日志：审计拒绝（`avc:`/`audit(`）、wlan 省电、
        // 唤醒源与水印统计。它们在每个取证窗口都占满配额，必须先剔除。
        const NOISE: [&str; 6] = [
            "avc:",
            "audit(",
            "[wlan]",
            "Pending Wakeup Sources",
            "PowerDet:",
            "active wake lock",
        ];
        if NOISE.iter().any(|noise| line.contains(noise)) {
            return false;
        }
        const KEYWORDS: [&str; 10] = [
            "blocked for more than",
            "hung_task",
            "task:",
            "f2fs",
            "I/O error",
            "FUSE",
            "fuse",
            "oom",
            "Out of memory",
            "Read-only file system",
        ];
        KEYWORDS.iter().any(|keyword| line.contains(keyword))
    }

    let mut buffer = [0u8; 4096];
    let mut pending: Vec<u8> = Vec::with_capacity(1024);
    // 命中行环形保留最近 64 条：kmsg 从最旧开始流出，冻结时刻的相关日志在缓冲
    // 末尾——顺序截断会只留下开机早期噪音，错过现场。
    let mut recent: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut lines_scanned = 0usize;
    // 上限双保险：最多扫 20000 行 / 8MB（覆盖 1MB 默认内核环形缓冲的满载形态），
    // 防止异常输出拖住取证线程。
    const MAX_LINES: usize = 20_000;
    const KEEP_MATCHED: usize = 64;
    'outer: loop {
        // SAFETY: fd 是本函数打开的有效描述符，buffer 为栈上可写缓冲。
        let n = unsafe { libc::read(fd, buffer.as_mut_ptr() as *mut libc::c_void, buffer.len()) };
        if n <= 0 {
            break;
        }
        for byte in buffer[..n as usize].iter() {
            if *byte == b'\n' {
                lines_scanned += 1;
                let line = String::from_utf8_lossy(&pending);
                if is_stall_kmsg_line(&line) {
                    if recent.len() >= KEEP_MATCHED {
                        recent.pop_front();
                    }
                    // kmsg 行首带 <priority> 前缀与时间戳，原样保留便于对时。
                    recent.push_back(line.into_owned());
                }
                pending.clear();
                if lines_scanned >= MAX_LINES {
                    break 'outer;
                }
            } else {
                pending.push(*byte);
            }
        }
    }
    // SAFETY: fd 是本函数打开的描述符，退出前是唯一清理点。
    unsafe { libc::close(fd) };
    for line in &recent {
        forensics_log(&format!("fuse host stall kmsg {line}"));
    }
    forensics_log(&format!(
        "fuse host stall kmsg done scanned={lines_scanned} logged={}",
        recent.len()
    ));
}

/// 按 comm/cmdline 定位 MediaProvider 进程（取证对象：它的线程栈能区分「MP 卡死
/// 拖累宿主」与「宿主自身停摆」）。
fn find_media_provider_pids() -> Vec<i32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<i32>().ok())
        .filter(|pid| {
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm"))
                .map(|text| text.trim().to_string())
                .unwrap_or_default();
            if comm.contains("media.module") {
                return true;
            }
            std::fs::read_to_string(format!("/proc/{pid}/cmdline"))
                .map(|text| text.contains("com.android.providers.media.module"))
                .unwrap_or(false)
        })
        .take(2)
        .collect()
}

fn finish_background_session(
    background: fuser::BackgroundSession,
    mount_point: &str,
    app_exited: bool,
    identity: Option<&ScopedMountIdentity>,
) -> bool {
    // 收尾前必须确认挂载点仍由本次会话持有：daemon 的重新挂载会在同一路径叠加新会话的
    // 挂载，此时按路径卸载摘掉的是新会话的挂载，应用会直接看到 ENOTCONN。fuser 的
    // `umount_and_join` 与 `BackgroundSession` 的 Drop 都会按路径卸载，因此这种情况下只能
    // 跳过收尾，让服务进程直接退出（进程退出会关闭 `/dev/fuse`，内核随之结束本次会话）。
    let ownership = mount_ownership(mount_point, identity);
    if !matches!(ownership, MountOwnership::Session) {
        log::info!(
            "fuse redirect session mount not owned mp={} app_exited={} state={} session_mount_id={}",
            mount_point,
            app_exited,
            ownership.as_str(),
            identity
                .map(|identity| identity.mount_id)
                .unwrap_or_default()
        );
        std::mem::forget(background);
        return true;
    }
    match background.umount_and_join() {
        Ok(()) => {
            log::info!(
                "fuse redirect session ended cleanly mp={} app_exited={}",
                mount_point,
                app_exited
            );
            true
        }
        Err(error) => finish_failed_session(mount_point, app_exited, &error, identity),
    }
}

/// 处理 scoped 会话收尾失败。
///
/// 挂载点被回收时 `umount` 可能失败：daemon 重新挂载会先摘除目标挂载点再终止旧服务
/// 进程，应用退出时系统也会连带销毁它的挂载 namespace。此时 EINVAL/ENOENT 只说明
/// 挂载点已经不存在；ENOTCONN 则表示 FUSE 连接已断但挂载记录仍可能留在应用 namespace
/// 中，必须继续用会话身份执行延迟卸载，否则应用会永久看到失效挂载。应用仍在运行且
/// 挂载点仍有引用时则是 EBUSY，两者都只是收尾事件，不能证明设备不支持 scoped 会话。
/// 能力快照是整机状态，一旦写成 unavailable，Auto 后端在本轮开机内不会再次尝试 scoped
/// 挂载，因此只有挂载点仍由本次会话持有且延迟卸载也失败时才记录能力失败。
fn finish_failed_session(
    mount_point: &str,
    app_exited: bool,
    error: &io::Error,
    identity: Option<&ScopedMountIdentity>,
) -> bool {
    let error_no = error.raw_os_error().unwrap_or_default();
    if app_exited || is_already_unmounted_errno(error_no) {
        log::info!(
            "fuse redirect session ended mp={} app_exited={} err={}",
            mount_point,
            app_exited,
            error
        );
        return true;
    }

    if detach_mount_point(mount_point, identity) {
        log::warn!(
            "fuse redirect session detach ok mp={} app_exited={} err={}",
            mount_point,
            app_exited,
            error
        );
        return true;
    }

    // 挂载点仍由本次会话持有且延迟卸载也没能摘除，说明这次 scoped 会话确实无法收尾。
    // 注意这里**不能**改动挂载准入：摘不掉旧挂载并不能说明下一次 mount(2) 会失败，用它去关掉
    // 全设备的 FUSE 是口径错配。只累计收尾失败计数供观察，准入由应用分桶决定。
    record_fuse_teardown_failure("scoped_session_end_error");
    log::warn!(
        "fuse redirect session ended with error mp={} app_exited={} err={}",
        mount_point,
        app_exited,
        error
    );
    false
}

/// 判断 errno 是否表示挂载点已经不在当前挂载命名空间。
///
/// - EINVAL：`umount2` 要求目标仍是挂载点，重新挂载流程已用 `MNT_DETACH` 摘掉旧挂载时
///   就会返回该错误；
/// - ENOENT：挂载点路径已不存在；
///
/// `ENOTCONN` 不在此列：它通常表示挂载记录还在但 FUSE 服务已退出，必须由调用方继续
/// 执行 `MNT_DETACH`，否则目标进程会永久保留返回 ENOTCONN 的死挂载。
fn is_already_unmounted_errno(error_no: i32) -> bool {
    matches!(error_no, libc::EINVAL | libc::ENOENT)
}

/// 用延迟卸载兜底清理会话挂载点。
///
/// 返回 true 表示挂载点已经不在当前命名空间：本次卸载成功，或系统（应用退出、挂载
/// namespace 销毁）已经把它摘掉。
fn detach_mount_point(mount_point: &str, identity: Option<&ScopedMountIdentity>) -> bool {
    // 只有挂载点最顶层确实是本次会话的 scoped 挂载才做延迟卸载；否则宁可保留告警，
    // 也不能在收尾失败时误摘同路径上新挂载的其它文件系统。
    if !matches!(
        mount_ownership(mount_point, identity),
        MountOwnership::Session
    ) {
        return false;
    }

    let Ok(c_target) = CString::new(mount_point) else {
        return false;
    };
    // SAFETY: c_target 是以 NUL 结尾的合法 C 字符串，并在调用期间保持存活。
    if unsafe { libc::umount2(c_target.as_ptr(), libc::MNT_DETACH) } == 0 {
        return true;
    }
    is_already_unmounted_errno(last_errno())
}

/// 本模块所有 FUSE 会话使用的挂载源。
///
/// 取系统 MediaProvider FUSE 的同一个源（`/dev/fuse`）：挂载表里任何一条记录都不再
/// 带模块标识。归属判定因此完全改由内核事实承担——超级块设备号（`dev`）+ 挂载 ID，
/// 登记在账本与宿主快照里（见 `mount_ledger::known_module_fuse_devs` 与
/// `fuse_host::current_host_dev`）。
///
/// **不要再往这里加会话标识**（例如 `srx_fuse_host[<pid>]`）：那正是"被接管应用一读
/// 挂载表就能认出本模块"的根源，而且会让"挂载源"同时承担身份与诊断两种职责，改任何
/// 一处都要连带改十几处判定。
pub const NATIVE_FUSE_SOURCE: &str = "/dev/fuse";

/// 生成共享宿主会话的挂载源（`MountOption::FSName`）。
///
/// 保留函数形态只为让调用点语义清晰（"这次会话的源"）；取值恒为 [`NATIVE_FUSE_SOURCE`]。
// quality-allow(lint-suppression): 宿主挂载源生成函数仅在 daemon 二进制路径使用,lib 目标不调用。
#[allow(dead_code)]
pub fn host_mount_source(_service_pid: u32) -> String {
    NATIVE_FUSE_SOURCE.to_string()
}

/// `/proc/self/mountinfo` 中一条挂载记录里用于判定挂载归属的字段。
#[derive(Clone)]
struct MountEntry {
    mount_id: u64,
    /// 父挂载 ID：同挂载点叠放时即下面一层的挂载 ID，用于栈顶判定。
    parent_mount_id: u64,
    /// 超级块设备号：bind/克隆继承同一超级块，是"哪一层的文件系统"的身份。
    dev: String,
    fs_type: String,
}

impl MountEntry {
    /// 判断这条记录是否本模块的 FUSE 挂载（任一 scoped 会话或共享宿主会话）。
    ///
    /// 判据是超级块身份而不是挂载源：挂载源已原生化成 `/dev/fuse`，与系统 MediaProvider
    /// 的 FUSE 完全同形；"是不是我们的"只能靠本模块自己登记过的 dev 集合。
    fn is_module_fuse(&self) -> bool {
        self.fs_type.starts_with("fuse")
            && !self.dev.is_empty()
            && crate::mount_ledger::known_module_fuse_devs().contains(&self.dev)
    }
}

/// scoped 会话在挂载后登记的挂载身份。
///
/// 收尾时必须确认挂载点仍由本次会话持有。挂载源已原生化，无法再用源文本认领，
/// 因此只保留超级块身份：`mount_id` 区分叠放层次，`dev` 区分"哪一代会话"。
#[derive(Clone, PartialEq, Eq)]
struct ScopedMountIdentity {
    mount_id: u64,
    dev: String,
}

impl ScopedMountIdentity {
    /// 挂载完成后登记本次会话身份。
    ///
    /// 必须在 `spawn_mount2` 成功返回之后调用：此时该挂载点最顶层就是刚挂上的这一层，
    /// 因此"取最顶层"本身就是"取本次会话"。挂载源原生化之后无法再用源文本认领，
    /// 超级块身份（mount_id + dev）是唯一还能区分"本轮挂载"与"系统挂载"的事实。
    fn capture(mount_point: &str) -> Option<Self> {
        let entry = topmost_mount_entry(mount_point)?;
        if entry.dev.is_empty() || !entry.fs_type.starts_with("fuse") {
            return None;
        }
        Some(Self {
            mount_id: entry.mount_id,
            dev: entry.dev,
        })
    }
}

/// 在有限窗口内连续确认本会话挂载，避免启动后立即报告不稳定状态。
fn wait_for_stable_session_mount(mount_point: &str) -> Option<ScopedMountIdentity> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1200);
    let mut stable: Option<ScopedMountIdentity> = None;
    let mut consecutive_matches = 0;
    while std::time::Instant::now() < deadline {
        match ScopedMountIdentity::capture(mount_point) {
            // 连续两轮看到同一个超级块身份，才认为这一层稳定落定（首轮的身份就是本次会话：
            // 挂载刚成功返回，最顶层必然是它）。
            Some(identity) if stable.as_ref() == Some(&identity) => {
                consecutive_matches += 1;
                if consecutive_matches >= 2 {
                    return Some(identity);
                }
            }
            Some(identity) => {
                stable = Some(identity);
                consecutive_matches = 1;
            }
            None => {
                stable = None;
                consecutive_matches = 0;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

/// 读取当前挂载表中指定挂载点上的全部记录。
///
/// 同一路径可能叠着多层挂载（daemon 重挂载期间旧会话与新会话并存），因此返回列表，由
/// [`topmost_mount_entry`] 选出最顶层的一条。
fn mount_entries_at(mount_point: &str) -> Vec<MountEntry> {
    let Ok(content) = std::fs::read_to_string("/proc/self/mountinfo") else {
        return Vec::new();
    };
    let normalized = paths::normalize(mount_point);
    content
        .lines()
        .filter_map(|line| {
            let entry = mountinfo::parse_entry(line)?;
            if !paths::eq_ignore_case(
                &paths::normalize(&mountinfo::unescape_field(entry.target)),
                &normalized,
            ) {
                return None;
            }
            Some(MountEntry {
                mount_id: entry.mount_id,
                parent_mount_id: entry.parent_mount_id,
                dev: entry.dev.to_string(),
                fs_type: entry.fs_type.to_string(),
            })
        })
        .collect()
}

/// 返回挂载点上最顶层的挂载记录（按父挂载链判定叠放顺序）。
///
/// 挂载 ID 在 detached tree + `move_mount` 搬运形态下不随叠放顺序递增（真机
/// 2026-10-15：scoped FUSE 层 id=233 小于其下屏障层 id=10034，按最大 ID 选取把栈底
/// 屏障当成栈顶，健康会话被误判 not stable 后终止，留下 ENOTCONN 死挂载导致应用商店
/// 更新全部失败）。叠层顺序只能按 `parent_mount_id` 父链判定，详见
/// [`mountinfo::topmost_by_stack`]。
fn topmost_mount_entry(mount_point: &str) -> Option<MountEntry> {
    let entries = mount_entries_at(mount_point);
    mountinfo::topmost_by_stack(
        &entries,
        |entry: &MountEntry| entry.mount_id,
        |entry: &MountEntry| entry.parent_mount_id,
    )
    .cloned()
}

/// 判断挂载点当前由谁持有。
fn mount_ownership(mount_point: &str, identity: Option<&ScopedMountIdentity>) -> MountOwnership {
    let Some(entry) = topmost_mount_entry(mount_point) else {
        return MountOwnership::Released;
    };
    let owned_by_session = match identity {
        // 挂载 ID + 超级块双重比对：ID 随命名空间内的挂载存在，dev 保证"同一超级块"。
        Some(identity) => entry.mount_id == identity.mount_id && entry.dev == identity.dev,
        // 未能登记会话身份（挂载时挂载表不可读）时退回按超级块归属判断，至少不会把
        // 系统媒体 FUSE 挂载当成模块挂载。
        None => entry.is_module_fuse(),
    };
    if owned_by_session {
        MountOwnership::Session
    } else {
        MountOwnership::Superseded
    }
}

/// scoped 会话收尾前判断挂载点归属的结果。
enum MountOwnership {
    /// 挂载点最顶层仍是本次会话的挂载。
    Session,
    /// 挂载点上已没有记录：本次会话的挂载已经被摘除。
    Released,
    /// 挂载点被其它挂载接管（daemon 重新挂载叠加了新会话的挂载）。
    Superseded,
}

impl MountOwnership {
    fn as_str(&self) -> &'static str {
        match self {
            MountOwnership::Session => "session",
            MountOwnership::Released => "released",
            MountOwnership::Superseded => "superseded",
        }
    }
}

/// P5-c 总开关：视图根是否上提一层（`…/emulated` 取代 `…/emulated/<user>`）。
///
/// 置 true 时必须与 `apply.rs` 的主挂载目标同步改动，两者不一致会让"会话挂载点"和
/// "策略里认定的视图根"错位（策略侧 `view_lifted` 由挂载点形态派生）。改这里等于同时
/// 打开 C2 的剥段与 C3 的根条目合成，必须整轮真机验证。
pub(crate) const VIEW_ROOT_LIFTED: bool = true;

/// 共享宿主会话的接入目标：上提模式下是视图根（`…/emulated`），否则是用户根。
///
/// daemon 与 companion 两条接入路径都只从这里取，绝不各自推导：两侧各写一份的后果是
/// "一侧抬了、另一侧没抬"，症状是应用静默失去重定向且没有任何报错。
pub(crate) fn host_attach_root_for_user(user_id: i32) -> String {
    let user_root = crate::platform::paths::storage_user_root_for_user(user_id);
    if VIEW_ROOT_LIFTED {
        crate::platform::paths::parent(&user_root)
    } else {
        user_root
    }
}

pub(super) fn fuse_mount_point(config: &FuseRedirectConfig, user_id: i32) -> String {
    let storage_root = paths::storage_user_root_for_user(user_id);
    let Some(raw_mount_root) = config.mount_root.as_deref() else {
        return lifted_view_root(&storage_root);
    };
    let mut mount_root = paths::resolve_user_path(&paths::normalize(raw_mount_root), user_id);
    if !paths::is_absolute(&mount_root) {
        mount_root = paths::normalize(&paths::join(&storage_root, &mount_root));
    }
    if paths::eq_ignore_case(&mount_root, &storage_root) {
        // 整体重定向（根就是用户根）：上提模式下换成父级视图根。
        lifted_view_root(&storage_root)
    } else if paths::is_child(&mount_root, &storage_root) {
        // scoped 子路径会话（hybrid 映射）：保持原位，局部会话不上提。
        mount_root
    } else {
        lifted_view_root(&storage_root)
    }
}

/// 用户根 → 视图根：上提模式下是父级 `…/emulated`，否则原样返回用户根。
fn lifted_view_root(storage_root: &str) -> String {
    let lifted = if VIEW_ROOT_LIFTED {
        paths::parent(storage_root)
    } else {
        storage_root.to_string()
    };
    // 这里是纯计算辅助函数，每次挂载点推导都会调用一次（重试轮次里一轮几十次）。
    // 曾经是 warn 级诊断，真机实测一天能刷出上万行、并持续触发落盘与日志轮转——
    // 这正是「空闲也在写盘」的典型形态。需要现场排查时打开 verbose 即可看到。
    log::debug!(
        "fuse mount point storage_root={} lifted={}",
        storage_root,
        lifted
    );
    lifted
}

fn send_ready_result(sock: Option<libc::c_int>, result: i32) {
    let Some(sock) = sock else {
        return;
    };
    // SAFETY: sock 是有效的 socket fd，buffer 指针指向栈上有效数据，size 与类型匹配，调用期间保持有效。
    let _ = unsafe {
        libc::send(
            sock,
            &result as *const _ as *const libc::c_void,
            std::mem::size_of::<i32>(),
            0,
        )
    };
    // SAFETY: sock 是有效的 socket fd，此处是唯一的关闭点，调用后不再使用。
    unsafe { libc::close(sock) };
}

/// 请求是否携带任何策略规则（完整重定向，或放行/排除/映射/沙箱/只读任一非空）。
///
/// 宿主接入由策略规则驱动：无规则的请求接入宿主只会得到 deny_all（未配置
/// 应用空目录、写入全拒，2026-10-30 issue #2 报告者 test14 实测）。preattach
/// 快捷路径与 planner 完整路径都必须以本判定作为接入前置；daemon 与
/// companion 两条挂载链路、lib 与 bin 两套编译副本共用本判定。
///
/// 既非仅监视也非仅映射的请求是完整重定向：五类列表全空时语义为“整个共享
/// 存储进入应用私有空间”，本身就是挂载需求，不能按无规则短路。未配置应用
/// 只会以仅监视形态发出请求，不受此分支影响。
/// 请求是否带**显式路径规则**（五个规则数组任一非空）。
///
/// 与 [`mount_request_has_policy_rules`] 的区别：后者把「全隔离」（`enabled` 且非
/// mapping_mode_only）也算作有规则，因为 deny-all + 沙盒本身就是一种策略；而本函数
/// 只承认写出来的规则。共享宿主接入闸门当前用本函数，因此全隔离应用不会接入 FUSE，
/// 而是走 namespace bind（原因单独上报为 `no_explicit_rules`）。
pub fn mount_request_has_explicit_rules<R: MountRequestFields + ?Sized>(request: &R) -> bool {
    !request.allowed_real_paths().is_empty()
        || !request.excluded_real_paths().is_empty()
        || !request.path_mappings().is_empty()
        || !request.sandboxed_paths().is_empty()
        || !request.read_only_paths().is_empty()
}

pub fn mount_request_has_policy_rules<R: MountRequestFields + ?Sized>(request: &R) -> bool {
    (!request.is_monitor_only() && !request.is_mapping_mode_only())
        || !request.allowed_real_paths().is_empty()
        || !request.excluded_real_paths().is_empty()
        || !request.path_mappings().is_empty()
        || !request.sandboxed_paths().is_empty()
        || !request.read_only_paths().is_empty()
}
