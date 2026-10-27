use crate::domain::{PathMapping, sort_path_mappings_shortest_request_first};
use crate::mount_ledger::NamespaceIdentity;
use crate::platform::{fs, module_paths, paths};
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use super::FuseRedirectConfig;
use super::backend_overlay;

/// 系统代写代理会话：平台用来代其它应用写私有目录的受信 provider。
///
/// 仅覆盖 FUSE 会话侧的包名判定；共享 UID 组成员（如 media uid 下的
/// soundpicker）不在其列，它们的会话仍保持隔离语义。
fn is_system_write_delegate_session(package_name: &str) -> bool {
    crate::redirect::policy::is_media_provider_package(package_name)
        || crate::redirect::policy::is_file_monitor_bridge_package(package_name)
}

const FILE_MONITOR_LOG_TAG: &str = "FileMonitorOp";

/// 应用 uid 起点：低于它的 uid（shell/system/媒体服务等）走系统直通回退与
/// 请求者归因，与 hook 侧 `src/hook/caller.rs` 的同名常量语义一致。
pub(crate) const ANDROID_APP_UID_START: i32 = 10000;

/// 系统 uid 请求者的监视归因身份（自有 FUSE 引擎内 daemon root 直读 /proc 解析）。
#[derive(Clone, Debug)]
pub(super) struct RequesterIdentity {
    /// 发起者身份：包名形态取全名，native 二进制取 basename（与 daemon pid 表
    /// 身份语义一致，见 [`crate::platform::proc_identity::read_proc_identity`]）。
    pub package: String,
    /// 发起者 uid（与 FUSE 请求头 uid 一致，解析时已校验）。
    pub uid: i32,
}
const READ_ONLY_DENY_EXTRA: &str = "deny_reason=read_only_rule";
const DUPLICATE_MONITOR_CREATE_WINDOW_MS: i64 = 1500;
const MAX_RECENT_MONITOR_CREATES: usize = 256;
static NAMESPACE_POLICY_FALLBACKS: AtomicU64 = AtomicU64::new(0);
static NAMESPACE_POLICY_MISSES: AtomicU64 = AtomicU64::new(0);

static RECENT_MONITOR_CREATES: Lazy<Mutex<HashMap<String, i64>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// 跨应用只读别名兜底的会话表句柄：共享宿主进程内所有已登记应用的策略。
///
/// 由 [`SharedPolicyTable::register`] 首次按 uid 登记时挂入，始终指向同一份活表，
/// 应用策略的新增与覆盖对兜底解析即时可见。scoped 独立会话进程不经过按 uid 登记，
/// 表中「其它应用」集合为空，跨应用兜底自然不生效，行为与改动前一致。
static CROSS_SESSION_POLICY_TABLE: Lazy<Mutex<Option<SharedPolicyTable>>> =
    Lazy::new(|| Mutex::new(None));

/// 跨应用只读别名兜底的交接信号窗口与容量。
///
/// 应用 A 通过 FileProvider 等机制把 fd 交给应用 B 之前，必须先在自己的 FUSE 会话里
/// 以映射视图路径打开同一个文件；这条访问记录就是「B 正被 A 调用」的唯一可观测等价
/// 信号（FUSE 层看不到 intent/binder 调用链）。窗口取 60 秒：交接在毫秒级完成，
/// 窗口只用于吸收 B 侧多轮探测与重试的间隔；容量上限防止长会话进程内无界增长。
const CROSS_APP_HANDOFF_TTL_MS: i64 = 60_000;
const CROSS_APP_HANDOFF_CAPACITY: usize = 256;
static RECENT_CROSS_APP_HANDOFFS: Lazy<Mutex<HashMap<String, i64>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

pub(super) struct BackendPath {
    pub(super) rel: String,
    pub(super) path: PathBuf,
    pub(super) is_read_only: bool,
    pub(super) is_shared_public_backend: bool,
}

/// 原生 scoped storage 下应用 uid 允许直接写入的公共媒体集合（存储根一级目录）。
///
/// MediaProvider 对普通应用的 FUSE 写放行范围是「应用自有私有目录 + 公共媒体集合」；
/// 宿主会话的数据面直连 f2fs 后端，绕过了这层判定。若不补一道等价门控，
/// mapping/monitor 形态的应用对存储根下任意路径的创建/写入都会成功
/// （真机 2026-10-27 实证：支付宝在存储根创建 `alipay/`、微信创建 `tencent/`，
/// 原生行为下这些请求会被 MP 拒绝——用户感知为「API 行为回退到旧版本」）。
const NATIVE_PUBLIC_MEDIA_TOP_DIRS: &[&str] = &[
    "Download",
    "DCIM",
    "Pictures",
    "Movies",
    "Music",
    "Documents",
    "Audiobooks",
    "Recordings",
    "Alarms",
    "Notifications",
    "Podcasts",
    "Ringtones",
];

/// 判断视图路径是否落在原生允许应用写入的公共媒体集合内。
///
/// 只认存储根一级目录名（`Download/...`、`DCIM/...` 等）；存储根本身、`Android/`
/// 与其它自定义一级目录（`alipay/`、`tencent/` 等）不属于公共集合，与原生
/// MediaProvider 的拒绝范围一致。
fn is_native_public_media_write_path(storage_path: &str) -> bool {
    let Some(rest) = storage_path.strip_prefix("/storage/emulated/") else {
        return false;
    };
    let Some(slash) = rest.find('/') else {
        return false;
    };
    let top = rest[slash + 1..].split('/').next().unwrap_or("");
    NATIVE_PUBLIC_MEDIA_TOP_DIRS.contains(&top)
}

/// `Android/media/<包名>` 子树按原生共享语义放行写操作。
///
/// MediaProvider 对 `Android/media` 的原生判定是同 user 内跨应用共享（读写均放行，
/// 这正是本模块 MP hook 侧 `should_allow_own_android_private_path_access` 恢复的
/// 语义）。写门控若不放行该子树，文件管理器/插件对**其它应用** media 目录的写入
/// 会被误拦（2026-10-27 issue #2 用户复测实证：MT 管理器写
/// `Android/media/com.eg.android.AlipayGphone/Sen/` 与
/// `Android/media/byseven.forest.sen/` 被拒报 ENOENT，Sen 插件配置随之失效）。
fn is_shared_media_subtree_write_path(storage_path: &str) -> bool {
    let owner = paths::extract_android_private_path_owner(storage_path);
    !owner.is_empty()
        && paths::is_valid_package_name(&owner)
        && storage_path.contains(&format!("/Android/media/{owner}"))
}

/// 共享 UID 组内跨包名的私有目录写放行。
///
/// sharedUserId 应用（如微博主进程与 `com.sina.weibo.image` 共享 uid 10374）的各
/// 进程以各自包名发起存储请求，原生 MediaProvider 按 **uid** 判定自有目录访问；
/// 宿主会话策略按包名登记，若只比较包名会把同 uid 兄弟进程对自己组内
/// `Android/data/<主包名>` 的写误拦（2026-10-27 实证：微博图片进程写主进程缓存
/// 与 `wlog` 全部被拒，`wlog` 的 mmap 文件创建失败后继续使用旧映射即 SIGBUS，
/// 图片大图加载失败同源）。这里把路径属主包名解析为 uid，与本会话 uid 一致即放行。
fn is_same_uid_private_write_path(storage_path: &str, session_uid: i32) -> bool {
    let owner = paths::extract_android_private_path_owner(storage_path);
    if owner.is_empty() || !paths::is_valid_package_name(&owner) {
        return false;
    }
    package_uid(&owner).is_some_and(|uid| uid == session_uid)
}

/// `/data/system/packages.list` 的包名→uid 缓存（以文件 mtime+长度为版本键）。
type PackageUidCache = Option<((u64, u64), HashMap<String, i32>)>;

fn package_uid(package: &str) -> Option<i32> {
    const PACKAGES_LIST: &str = "/data/system/packages.list";
    static CACHE: Lazy<Mutex<PackageUidCache>> = Lazy::new(|| Mutex::new(None));
    let meta = std::fs::metadata(PACKAGES_LIST).ok()?;
    let version = (
        meta.modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_nanos() as u64,
        meta.len(),
    );
    let mut guard = CACHE.lock().ok()?;
    if guard.as_ref().is_none_or(|(key, _)| *key != version) {
        let content = std::fs::read_to_string(PACKAGES_LIST).ok()?;
        let mut map = HashMap::new();
        for line in content.lines() {
            let mut fields = line.split_whitespace();
            let Some(name) = fields.next() else {
                continue;
            };
            let Some(uid) = fields.next().and_then(|value| value.parse::<i32>().ok()) else {
                continue;
            };
            map.insert(name.to_string(), uid);
        }
        *guard = Some((version, map));
    }
    guard.as_ref()?.1.get(package).copied()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BackendKind {
    Real,
    Redirect,
}

#[derive(Clone, Copy)]
pub(super) enum OperationKind {
    Read,
    Write,
}

struct BackendDecision {
    kind: BackendKind,
    is_read_only: bool,
    /// 命中 sandboxed_paths 的重定向：监视/直通会话的重定向根是存储根本身，
    /// 此时 backend 必须落到该应用私有 sdcard 沙盒根，否则 native 层创建会
    /// 绕过沙盒直落真实路径（2026-10-21 用户反馈的 .xlDownload 侧漏）。
    is_sandbox: bool,
}

#[derive(Clone, Default)]
struct RuleIndex {
    exact: HashSet<String>,
    wildcard: Vec<String>,
}

impl RuleIndex {
    fn new(rules: &[String]) -> Self {
        let mut index = Self::default();
        for rule in rules {
            if !paths::contains_wildcards(rule) && !rule.contains("//") {
                index
                    .exact
                    .insert(paths::match_key(rule.trim_end_matches('/')));
            } else {
                index.wildcard.push(rule.clone());
            }
        }
        index
    }

    fn matches(&self, storage_path: &str) -> bool {
        if self.matches_exact(storage_path) {
            return true;
        }
        self.wildcard
            .iter()
            .any(|rule| paths::matches(rule, storage_path, true))
    }

    fn matches_exact(&self, storage_path: &str) -> bool {
        let mut candidate = storage_path.trim_end_matches('/');
        loop {
            if self.exact.contains(&paths::match_key(candidate)) {
                return true;
            }
            let Some(separator) = candidate.rfind('/') else {
                return false;
            };
            if separator == 0 {
                return false;
            }
            candidate = &candidate[..separator];
        }
    }
}

#[derive(Clone)]
pub(super) struct RulePrefix {
    pub(super) rel: String,
    /// `rel` 对应的存储路径，在构造时算出。
    ///
    /// 该值由 `storage_root` 与 `rel` 决定，策略建成后即为常量；而 `is_virtual_dir`
    /// 会被目录扫描按条目数反复调用，若在循环内重新拼接，每个目录项都要为每条规则
    /// 多分配一次字符串。
    pub(super) storage_path: String,
    pub(super) full_prefix: bool,
}

#[derive(Clone)]
pub(super) struct RedirectPolicy {
    pub(super) package_name: String,
    pub(super) uid: i32,
    pub(super) user_id: i32,
    pub(super) storage_root: String,
    pub(super) mount_rel: String,
    pub(super) real_root: PathBuf,
    // 应用自有私有目录（Android/data|media|obb）专用的真实后端根，固定指向 /data/media/<user>。
    // real_root 通常是 real_storage anchor，而该 anchor 优先 bind 到 /storage/emulated/<user>，
    // 也就是叠在系统 MediaProvider FUSE 之上；MediaProvider 对 Android/media/<pkg> 下的文件
    // rename 会返回 ERANGE，导致依赖“写临时文件再改名”原子替换的应用安装或保存失败。
    // 私有目录本身不进 MediaStore 索引，直连 f2fs 既能绕开该缺陷又不损失系统语义。
    pub(super) private_real_root: PathBuf,
    /// 宿主命名空间内 MediaProvider FUSE 视图根（`tmp/fuse_host_media/<user>`）。
    ///
    /// 自有 `Android/media/<pkg>/` 下的 sqlite 数据库与边车文件经该视图读写，与其它
    /// 调用方（未重定向应用、MediaProvider 自身）共享同一份内核 inode 缓存：SQLite 的
    /// WAL 依赖跨进程共享的 `-shm` mmap 与文件锁，宿主直连 f2fs 与 MediaProvider FUSE
    /// 的内核缓存是两个互不通知的层，数据库文件删除重建后两层会各持一个代际，读取
    /// 撕裂表现为 `SQLITE_CORRUPT`（真机实测：支付宝内 XRadiant 的 XRadiant.db）。
    /// 为空表示宿主命名空间没有可用视图（scoped 会话、视图未绑定），保持直连行为。
    pub(super) media_sqlite_real_root: PathBuf,
    pub(super) redirect_root: PathBuf,
    /// 重定向根的视图形态（相对存储根，如 `Android/data/<pkg>/sdcard`）。
    ///
    /// 应用视图对重定向根自身的再进入（视图里出现同名子树）与根下同后端路径内容一致，
    /// 但决策路径不同，镜像子树会被反复再映射、逐层自嵌套（真机实测 70+ 层直到
    /// ENAMETOOLONG）。`backend_for_relative` 用它把再进入路径折叠回根下解析。
    /// 直通/监视会话的重定向根就是存储根，此处为空串，不折叠。
    pub(super) redirect_view_rel: String,
    pub(super) rule_prefixes: Vec<RulePrefix>,
    pub(super) allowed_real_paths: Vec<String>,
    pub(super) excluded_real_paths: Vec<String>,
    pub(super) sandboxed_paths: Vec<String>,
    pub(super) read_only_paths: Vec<String>,
    allowed_real_index: RuleIndex,
    excluded_real_index: RuleIndex,
    sandboxed_index: RuleIndex,
    sandboxed_excluded_index: RuleIndex,
    read_only_index: RuleIndex,
    read_only_excluded_index: RuleIndex,
    pub(super) path_mappings: Vec<PathMapping>,
    pub(super) is_mapping_mode_only: bool,
    pub(super) is_monitor_only: bool,
    /// 监视/直通会话的沙盒 backend 根（该应用私有 sdcard 根）；普通重定向会话为 None。
    pub(super) sandbox_backend_root: Option<PathBuf>,
    pub(super) is_file_monitor_enabled: bool,
    /// 一律拒绝：路径决策全部失败，调用方据此返回 `ENOENT`。
    ///
    /// 只用于宿主共享会话的"未登记 uid"回退——宁可让调用方读不到、写不进，也不能让它
    /// 直接落到真实存储上。普通策略恒为 `false`。
    deny_all: bool,
    /// 系统 uid 回退标记：标识本策略是宿主共享会话给系统 uid 的直通视图。
    ///
    /// 语义上 package_name 仍是宿主包名、不代表请求发起者；监视记录发射时据此
    /// 启用请求者归因（见 [`RedirectPolicy::emit_monitor_mutation`]）。
    pub(super) is_system_fallback: bool,
    /// 本策略由哪份配置构建（[`crate::fuse_redirect::config::fuse_config_policy_fingerprint`]）。
    /// 宿主共享会话跨配置 reload 时，异常回包（EROFS/ENOENT）带上它即可直接判定
    /// 「当时生效的是旧策略还是新策略」。
    pub(super) fingerprint: u64,
}

/// 会话策略注册表：把「一个 FUSE 会话绑定单一应用」扩展为「按调用方 uid 解析」。
///
/// FUSE 请求头自带调用方 uid（`fuser::Request::uid()` 直接读输入头的 uid 字段），
/// 共享宿主会话要让同一个挂载点同时服务多个应用，策略解析就不能再是「会话级常量」，
/// 必须改成「每请求按 uid 查表」。
///
/// 阶段 1 只建立查表能力并保持数据面行为不变：注册表仅登记会话默认策略，
/// `for_uid` 对任何 uid 都回退到它，因此结果与改造前直读 `FuseRedirectFs.policy` 完全一致。
/// 阶段 2 引入 `srx_fuse_host` 共享会话后，`by_uid` 才会被挂载点填成多应用表。
pub(super) struct PolicyRegistry {
    /// 会话创建时绑定的策略；用于与调用方身份无关的会话级语义（日志、`init`、`statfs`）。
    session: Arc<RedirectPolicy>,
    /// 系统 uid（非应用 uid）请求的回退策略：仅宿主共享会话构造。
    ///
    /// 系统 uid（shell/system/媒体服务等）不是沙盒逃逸对象——它们本就持有系统级
    /// 存储权限， denying 只会让 shell 管理操作与系统服务写入在宿主会话上无差别
    /// 失败（2026-10-25 CI A13/A14 实证：shell 的 mkdir 被拒，场景 40 连记录都
    /// 没有）。给这一区间放行会话直通视图，与 MP FuseDaemon 对系统 uid 的处理
    /// 语义一致；未登记的应用 uid 仍一律拒绝。
    system_fallback: Option<Arc<RedirectPolicy>>,
    /// 按调用方 uid 预解析的策略表。
    ///
    /// scoped 会话建成后不再变化；宿主共享会话需要在运行期内按 uid 接收应用策略，所以
    /// 这张表必须能在会话对象之外继续写入：宿主会话把句柄交给控制通道，由它按需登记。
    by_uid: SharedPolicyTable,
    /// uid 未命中时的回退策略。
    ///
    /// scoped 会话回退到会话策略（与改造前一致）；**宿主共享会话回退到"一律拒绝"**：
    /// 宿主会话服务的是完整存储视图，如果让未登记 uid 回退到直通策略，那个调用方就会看到
    /// 真实存储、写入也会直接落到真实位置（沙盒失效），而这类越权落点无法靠事后清理恢复。
    /// 因此宿主会话按设计拒绝未登记调用方，而不是放行。
    fallback: Arc<RedirectPolicy>,
}

/// 宿主会话运行期可写、FUSE 回调侧只读的按 uid 策略表句柄。
///
/// 回调热路径只取读锁，未命中时回退会话默认策略，因此登记前后都不改变单应用的既有语义。
#[derive(Clone)]
pub(crate) struct SharedPolicyTable {
    by_uid: Arc<RwLock<HashMap<u32, Arc<RedirectPolicy>>>>,
    by_namespace: NamespacePolicyTable,
}

#[derive(Clone)]
struct NamespacePolicyTable(Arc<RwLock<HashMap<NamespaceIdentity, Arc<RedirectPolicy>>>>);

impl SharedPolicyTable {
    /// 按 uid 登记或覆盖一份策略，并记录该应用 mount namespace 对应的策略。
    ///
    /// root 子进程通常继承应用的 mount namespace，但其 FUSE 请求 uid 已变成 0；namespace
    /// 索引只绑定当前应用进程实例，不把 uid 0 全局放行给某个应用。
    pub(crate) fn register(&self, config: FuseRedirectConfig) -> Option<usize> {
        let uid = u32::try_from(config.uid).ok()?;
        let app_pid = config.app_pid;
        let namespace = registered_mount_namespace(&config);
        let policy = Arc::new(RedirectPolicy::new(config)?);
        let mut table = self.by_uid.write().ok()?;
        table.insert(uid, Arc::clone(&policy));
        drop(table);
        // 首次按 uid 登记时把活表句柄挂入进程级静态：跨应用只读别名兜底从这份表
        // 派生「其它应用的映射规则」。重复赋值无副作用，句柄内的 Arc 指向同一份表。
        if let Ok(mut handle) = CROSS_SESSION_POLICY_TABLE.lock() {
            *handle = Some(self.clone());
        }
        if let Some(namespace) = namespace {
            let mut namespaces = self.by_namespace.0.write().ok()?;
            namespaces.insert(namespace, Arc::clone(&policy));
            log::info!(
                "fuse policy namespace registered uid={} ns={}:{} pkg={}",
                uid,
                namespace.dev,
                namespace.ino,
                policy.package_name
            );
        } else {
            log::warn!(
                "fuse policy namespace unavailable uid={} app_pid={}",
                uid,
                app_pid
            );
        }
        Some(self.by_uid.read().ok()?.len())
    }

    /// 只登记 mount namespace 维度的策略，不写按 uid 的全局表。
    ///
    /// 供 daemon 自身（uid=0）的直通登记使用：按 [`PolicyRegistry::for_request`] 的查找
    /// 顺序，uid=0 一旦进按 uid 表就会盖住**所有** root 调用方（任何 namespace 的 root
    /// FUSE 请求都先按 uid 命中），破坏「未登记 uid 一律 deny_all」的 fail-closed 边界。
    /// 只写 namespace 表时，其它 namespace 的 uid=0 请求仍然走 deny_all 回退。
    /// 返回 namespace 表大小（>0 即登记成功，供控制通道应答）。
    pub(crate) fn register_namespace_only(&self, config: FuseRedirectConfig) -> Option<usize> {
        let namespace = registered_mount_namespace(&config)?;
        let policy = Arc::new(RedirectPolicy::new(config)?);
        let (ns_dev, ns_ino, package_name) =
            (namespace.dev, namespace.ino, policy.package_name.clone());
        let mut namespaces = self.by_namespace.0.write().ok()?;
        namespaces.insert(namespace, policy);
        drop(namespaces);
        let table = self.by_namespace.0.read().ok()?.len();
        log::info!(
            "fuse policy namespace registered (ns-only) ns={}:{} pkg={} table={}",
            ns_dev,
            ns_ino,
            package_name,
            table
        );
        Some(table)
    }
}

impl PolicyRegistry {
    /// 用会话策略构造注册表。
    ///
    /// `deny_unregistered` 为真时（宿主共享会话），uid 未命中的回退是一份"一律拒绝"策略。
    pub(super) fn new(policy: RedirectPolicy, deny_unregistered: bool) -> Self {
        let session = Arc::new(policy);
        let fallback = if deny_unregistered {
            Arc::new(session.copied_as_deny_all())
        } else {
            Arc::clone(&session)
        };
        let system_fallback = if deny_unregistered {
            Some(Arc::new(session.copied_as_system_fallback()))
        } else {
            None
        };
        Self {
            session,
            by_uid: SharedPolicyTable {
                by_uid: Arc::new(RwLock::new(HashMap::new())),
                by_namespace: NamespacePolicyTable(Arc::new(RwLock::new(HashMap::new()))),
            },
            fallback,
            system_fallback,
        }
    }

    /// 会话绑定策略；用于与调用方身份无关的会话级语义（`init` / `statfs`）。
    pub(super) fn session(&self) -> &RedirectPolicy {
        self.session.as_ref()
    }

    /// 按调用方 uid 与 mount namespace 解析策略；未命中时回退到 [`Self::fallback`]。
    ///
    /// root 子进程通常继承应用的 mount namespace，但其 FUSE 请求 uid 已变成 0。共享宿主
    /// 不能把 uid 0 全局登记到某个应用，否则其它 root 进程会获得错误应用的策略；这里仅在
    /// uid 未命中且请求 uid 为 root 时，用请求 pid 的 mount namespace 精确回到发起该请求的应用策略。
    pub(super) fn for_request(&self, uid: u32, pid: u32) -> Arc<RedirectPolicy> {
        if let Ok(table) = self.by_uid.by_uid.read()
            && let Some(policy) = table.get(&uid)
        {
            return Arc::clone(policy);
        }
        // 未登记请求轨迹（2026-10-25 CI A14 场景 40 排查）：shell mkdir 在宿主
        // 会话上零记录且 system fallback 日志零命中，需要区分「请求未到达引擎」
        // 与「到达但走了未预期分支」。限频打印（64 次一次 + 首次），覆盖所有
        // 未命中请求。
        {
            static UNRESOLVED_TRACE: AtomicU64 = AtomicU64::new(0);
            let seen = UNRESOLVED_TRACE.fetch_add(1, Ordering::Relaxed) + 1;
            if seen == 1 || seen.is_multiple_of(64) {
                log::info!(
                    "fuse policy request trace uid={} pid={} session_pkg={} fallback_deny={}",
                    uid,
                    pid,
                    self.session.package_name,
                    self.fallback.is_deny_all()
                );
            }
        }
        if uid == 0 {
            let namespace = request_mount_namespace(pid);
            if let Some(namespace) = namespace
                && let Ok(table) = self.by_uid.by_namespace.0.read()
                && let Some(policy) = table.get(&namespace)
            {
                let count = NAMESPACE_POLICY_FALLBACKS.fetch_add(1, Ordering::Relaxed) + 1;
                if count == 1 || count.is_multiple_of(256) {
                    log::info!(
                        "fuse policy namespace fallback uid={} pid={} ns={}:{} pkg={} count={}",
                        uid,
                        pid,
                        namespace.dev,
                        namespace.ino,
                        policy.package_name,
                        count
                    );
                }
                return Arc::clone(policy);
            }
            let count = NAMESPACE_POLICY_MISSES.fetch_add(1, Ordering::Relaxed) + 1;
            if count == 1 || count.is_multiple_of(256) {
                let namespace_text = namespace
                    .map(|value| format!("{}:{}", value.dev, value.ino))
                    .unwrap_or_else(|| "unavailable".to_string());
                log::warn!(
                    "fuse policy namespace miss uid={} pid={} ns={} count={}",
                    uid,
                    pid,
                    namespace_text,
                    count
                );
            }
        }
        // 系统 uid（1..应用 uid 起点）请求走系统直通回退：shell/system/媒体服务
        // 的存储访问不经过应用沙盒（本就持有系统级权限），deny_all 回退会让系统
        // 管理操作与系统服务写入在宿主会话上无差别失败。uid 0 保持既有 namespace
        // 精确回退（root 子进程必须经 mount namespace 回到发起应用，不能全局放行）。
        if uid > 0
            && uid < ANDROID_APP_UID_START as u32
            && let Some(system_fallback) = self.system_fallback.as_ref()
        {
            static SYSTEM_FALLBACK_COUNT: AtomicU64 = AtomicU64::new(0);
            let count = SYSTEM_FALLBACK_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if count == 1 || count.is_multiple_of(256) {
                log::info!(
                    "fuse policy system fallback uid={} pid={} pkg={} count={}",
                    uid,
                    pid,
                    system_fallback.package_name,
                    count
                );
            }
            return Arc::clone(system_fallback);
        }
        Arc::clone(&self.fallback)
    }

    pub(super) fn for_uid_exact(&self, uid: u32) -> Option<Arc<RedirectPolicy>> {
        self.by_uid
            .by_uid
            .read()
            .ok()
            .and_then(|table| table.get(&uid).cloned())
    }

    /// 按包名反查已登记的属主策略，供宿主共享会话的「系统读取者」路径反解使用：
    /// MediaProvider 等系统读取者按路径访问 `Android/data/<pkg>/**` 时，需要用
    /// 目标应用的属主策略解析后端落点，否则 deny_all 回退必然 ENOENT。
    /// 表规模为已登记应用数（几十级），线性查找代价可忽略。
    pub(super) fn find_owner_policy_by_package(
        &self,
        package_name: &str,
    ) -> Option<Arc<RedirectPolicy>> {
        let table = self.by_uid.by_uid.read().ok()?;
        table
            .values()
            .find(|policy| policy.package_name == package_name)
            .cloned()
    }

    /// 取出可写表句柄，交给宿主会话的控制通道在运行期登记应用策略。
    pub(super) fn shared_table(&self) -> SharedPolicyTable {
        self.by_uid.clone()
    }
}

fn registered_mount_namespace(config: &FuseRedirectConfig) -> Option<NamespaceIdentity> {
    let expected_start = config.app_start_time_ticks?;
    if crate::platform::process_start_time_ticks(config.app_pid) != Some(expected_start) {
        return None;
    }
    crate::mount_ledger::namespace_identity(config.app_pid)
}

fn request_mount_namespace(pid: u32) -> Option<NamespaceIdentity> {
    let pid = i32::try_from(pid).ok()?;
    crate::mount_ledger::namespace_identity(pid)
}

impl RedirectPolicy {
    pub(super) fn new(config: FuseRedirectConfig) -> Option<Self> {
        // 指纹须在任何 config 字段被 move 前计算。
        let fingerprint = super::config::fuse_config_policy_fingerprint(&config);
        let user_id = config.user_id();
        let storage_root = paths::storage_user_root_for_user(user_id);
        let mount_root = super::config::fuse_mount_point(&config, user_id);
        let mount_rel = paths::relative_child_path(&mount_root, &storage_root)
            .unwrap_or("")
            .trim_matches('/')
            .to_string();
        let real_root = real_backend_root_for_config(&config, user_id);
        // 共享宿主直通和未配置应用的监视专用会话都不做任何重定向，`redirect_target`
        // 可以是整个存储根。按子路径推导重定向根对"根本身"返回 None，沿用普通流程会
        // 把这两类合法直通配置误判为非法，导致宿主策略拒绝 UID，应用看不到共享存储。
        // 两种模式都把重定向根指向真实根：即使某条路径意外走到重定向分支，落点仍是真实
        // 后端，不会凭空多出一层沙盒目录，也不需要创建任何目录。
        let redirect_storage = paths::resolve_user_path(
            &paths::resolve_placeholders(
                &paths::normalize(&config.redirect_target),
                &config.app_data_dir,
                &config.redirect_target,
            ),
            user_id,
        );
        let is_mapping_root_passthrough = config.is_mapping_mode_only
            && config.path_mappings.is_empty()
            && config.sandboxed_paths.is_empty()
            && paths::eq_ignore_case(&redirect_storage, &storage_root);
        let redirect_root_string = if config.is_passthrough_host
            || config.is_monitor_only
            || is_mapping_root_passthrough
        {
            real_root.to_string_lossy().to_string()
        } else {
            let redirect_root_string =
                paths::storage_to_data_media_for_user(&redirect_storage, user_id)
                    .unwrap_or_default();
            if redirect_root_string.is_empty() {
                log::error!("fuse redirect target invalid: {}", config.redirect_target);
                return None;
            }
            let redirect_root_uid = if crate::metadata_repair::enabled() {
                config.uid
            } else {
                -1
            };
            let existed_redirect_root = fs::is_directory(&redirect_root_string);
            if !fs::create_directory(&redirect_root_string, redirect_root_uid)
                && !fs::is_directory(&redirect_root_string)
            {
                // errno 区分失败类别：EACCES/EPERM/ENOKEY/ENOTCONN 是开机竞态
                // （sepolicy 未就绪、FBE CE 密钥未解锁，30 秒周期重试可自愈；
                // 真机实测开机 +15s 曾 42 包同秒 ENOKEY 失败），ENOTDIR/EEXIST
                // 则是路径被普通文件占用等配置或环境错误，需要用户侧介入。
                let errno = fs::last_create_directory_errno();
                if fs::errno_is_transient_boot_race(errno) {
                    log::warn!(
                        "fuse redirect target mkdir deferred (boot race): {} errno={} {}",
                        redirect_root_string,
                        errno,
                        crate::platform::errno::text(errno)
                    );
                } else {
                    log::error!(
                        "fuse redirect target mkdir failed: {} errno={} {}",
                        redirect_root_string,
                        errno,
                        crate::platform::errno::text(errno)
                    );
                }
                return None;
            }
            fix_mapped_dir_metadata(&redirect_root_string, config.uid);
            // 本次初始化新建的私有树重定向根按原生语义补 chmod：create_directory
            // 的 mkdir(0755) 受 daemon umask 与 setgid 继承影响，会落下 2750 这类
            // 非原生形态（2026-10-21 duchamp 实测）；既有目录仍交 fix_mapped 的
            // keep-existing 守卫保留。
            if !existed_redirect_root
                && let Some(user_id) =
                    paths::extract_user_id_from_data_media_path(&redirect_root_string)
                && is_android_private_backend_path(&redirect_root_string, user_id)
            {
                let c_root = CString::new(redirect_root_string.as_str()).ok();
                if let Some(c_root) = c_root {
                    // SAFETY: c_root 在调用期间保持有效并以 NUL 结尾。
                    let _ = unsafe { libc::chmod(c_root.as_ptr(), 0o2770) };
                }
            }
            redirect_root_string
        };

        // 重定向根的视图形态（相对存储根）：sdcard 形态重定向时为
        // Android/data/<pkg>/sdcard。直通/监视会话的重定向根就是存储根，此处为空串。
        // 应用视图对重定向根自身的再进入由 backend_for_relative 折叠回根下解析。
        let redirect_view_rel = paths::relative_child_path(&redirect_storage, &storage_root)
            .unwrap_or_default()
            .trim_matches('/')
            .to_string();
        // 监视/直通会话的重定向根是存储根，sandboxed_paths 命中时宿主面需要把
        // backend 换到该应用私有 sdcard 沙盒根（否则 native 层创建绕过沙盒直落
        // 真实路径——2026-10-21 用户反馈的 .xlDownload 侧漏）。普通重定向会话的
        // 重定向根本身就是沙盒根，走既有 redirect_backend_for_rel 即可。
        let sandbox_backend_root = if config.is_mapping_mode_only && redirect_view_rel.is_empty() {
            Some(PathBuf::from(format!(
                "{}/Android/data/{}/sdcard",
                paths::data_media_user_root_for_user(user_id),
                config.package_name
            )))
        } else {
            None
        };

        let mut path_mappings = resolve_path_mappings(
            &config.path_mappings,
            user_id,
            &storage_root,
            &config.app_data_dir,
            &config.redirect_target,
        );
        sort_path_mappings_shortest_request_first(&mut path_mappings);

        let rule_prefixes =
            build_rule_prefixes(&config, &path_mappings, user_id, &storage_root, &mount_root);

        // FUSE 与 namespace fallback 保持一致：通配只读规则收敛到具体父目录，
        // 确保通配根目录下未命中具体子目录的访问仍不会绕过只读边界。
        let normalized_read_only_paths = super::normalize_rule_list(
            crate::fuse_redirect::config::expand_namespace_fallback_rules(
                config.uid,
                &config.read_only_paths,
            ),
            user_id,
        );
        let (read_only_paths, read_only_excluded_paths) =
            paths::split_exclusion_rules(&normalized_read_only_paths);
        let read_only_excluded_paths =
            paths::overlapping_exclusion_rules(&read_only_paths, &read_only_excluded_paths);

        let allowed_real_paths = super::normalize_rule_list(config.allowed_real_paths, user_id);
        let excluded_real_paths = super::normalize_rule_list(config.excluded_real_paths, user_id);
        let (sandboxed_includes, sandboxed_excludes) =
            paths::split_exclusion_rules(&config.sandboxed_paths);
        let sandboxed_paths = super::normalize_rule_list(sandboxed_includes, user_id);
        let sandboxed_excluded_paths = super::normalize_rule_list(sandboxed_excludes, user_id);
        let allowed_real_index = RuleIndex::new(&allowed_real_paths);
        let excluded_real_index = RuleIndex::new(&excluded_real_paths);
        let sandboxed_index = RuleIndex::new(&sandboxed_paths);
        let sandboxed_excluded_index = RuleIndex::new(&sandboxed_excluded_paths);
        let read_only_index = RuleIndex::new(&read_only_paths);
        let read_only_excluded_index = RuleIndex::new(&read_only_excluded_paths);

        Some(Self {
            package_name: config.package_name,
            uid: config.uid,
            user_id,
            storage_root,
            mount_rel,
            real_root,
            private_real_root: PathBuf::from(paths::data_media_user_root_for_user(user_id)),
            media_sqlite_real_root: media_sqlite_view_root_for_user(user_id),
            redirect_root: PathBuf::from(redirect_root_string),
            redirect_view_rel,
            sandbox_backend_root,
            rule_prefixes,
            allowed_real_paths,
            excluded_real_paths,
            read_only_paths,
            allowed_real_index,
            excluded_real_index,
            sandboxed_paths,
            sandboxed_index,
            sandboxed_excluded_index,
            read_only_index,
            read_only_excluded_index,
            path_mappings,
            is_mapping_mode_only: config.is_mapping_mode_only,
            is_monitor_only: config.is_monitor_only,
            is_file_monitor_enabled: config.is_file_monitor_enabled,
            deny_all: false,
            is_system_fallback: false,
            fingerprint,
        })
    }

    /// 复制一份"一律拒绝"策略，用作宿主会话未登记 uid 的回退。
    ///
    /// 只复制规则数据并翻转拒绝标志，不重复执行构造期副作用（目录准备、元数据修复），
    /// 因此不会因为"多一份策略"而在存储上留下任何痕迹。
    fn copied_as_deny_all(&self) -> Self {
        Self {
            deny_all: true,
            ..self.clone()
        }
    }

    /// 系统 uid 直通回退策略：解除 deny_all 并打上系统回退标记。
    ///
    /// 生成物仅用于 [`PolicyRegistry::system_fallback`]；重定向规则、监视开关等
    /// 会话级语义与宿主会话一致，确保系统请求的落点/记录与宿主视图等价。
    fn copied_as_system_fallback(&self) -> Self {
        Self {
            deny_all: false,
            is_system_fallback: true,
            ..self.clone()
        }
    }

    pub(super) fn is_deny_all(&self) -> bool {
        self.deny_all
    }

    pub(super) fn backend_for_relative(
        &self,
        rel: &str,
        operation: OperationKind,
    ) -> Option<BackendPath> {
        // 拒绝策略先于任何路径解析生效：所有调用点（lookup/getattr/create/write/readdir/…）
        // 都把 `None` 当作失败处理，因此未登记调用方既读不到真实内容，也写不进任何位置。
        if self.deny_all {
            return None;
        }
        let rel = sanitize_relative(rel)?;
        // 自嵌套折叠：应用视图对重定向根自身的再进入（如 sdcard 形态重定向下的
        // Android/data/<pkg>/sdcard/<rest>）与根下 <rest> 指向同一后端，但决策路径不同，
        // 镜像子树会被反复再映射、逐层自嵌套（真机实测 70+ 层直到 ENAMETOOLONG，
        // 文件监视树修复预算反复打满）。剥掉重定向根的视图前缀后按剩余路径解析，
        // 嵌套层与根层从此等价，物理层不再生长。
        let full_rel = self.full_storage_rel(&rel);
        let rel = match self.strip_redirect_view_prefix(&full_rel) {
            Some(stripped) => self.rel_for_full_storage_rel(&stripped),
            None => rel,
        };
        let storage_path = self.storage_path_for_rel(&rel);
        let resolved_storage_path = paths::normalize(&storage_path);
        let decision = self.backend_decision(&resolved_storage_path, operation);
        let own_mapping_target = self.resolve_mapping(&resolved_storage_path);
        if own_mapping_target.is_some() {
            // 本应用经自身映射访问了该视图路径：登记交接信号。应用随后通过
            // FileProvider 等机制把该路径交给其它应用时，对方会话可以据此在
            // 窗口内完成跨应用只读解析（见下方跨应用别名兜底）。
            record_own_mapped_handoff(&resolved_storage_path);
        }
        // 应用 uid 的 Real 直写门控：宿主会话数据面直连 f2fs 后端，原生
        // MediaProvider 对「应用写自有私有目录/公共媒体集合之外」的拒绝判定被
        // 绕过，应用借此拿到 legacy 式的根目录写能力（2026-10-27 真机实证：
        // 支付宝在存储根创建 `alipay/`、微信创建 `tencent/`，原生下这些 mkdir
        // 会被 MP 拒绝）。这里按原生语义收紧：应用会话（非系统回退、非系统
        // 代写）的写操作只有落在自有私有目录、自身映射目标、配置放行目录、
        // 公共媒体集合、`Android/media` 共享子树或同 UID 组兄弟包名的私有目录
        // 时才允许 Real 直写，其余一律拒绝——与「完全不使用模块」时的原生行为
        // 对齐。读操作与系统会话不受影响。
        if decision.kind == BackendKind::Real
            && matches!(operation, OperationKind::Write)
            && !self.is_system_fallback
            && !is_system_write_delegate_session(&self.package_name)
            && own_mapping_target.is_none()
            && !self.is_own_private_storage_path(&resolved_storage_path)
            && !self.matches_any(&self.allowed_real_index, &resolved_storage_path)
            && !is_native_public_media_write_path(&resolved_storage_path)
            && !is_shared_media_subtree_write_path(&resolved_storage_path)
            && !is_same_uid_private_write_path(&resolved_storage_path, self.uid)
        {
            log::warn!(
                "fuse host write leak blocked pkg={} uid={} path={}",
                self.package_name,
                self.uid,
                resolved_storage_path
            );
            return None;
        }
        let mut backend = match decision.kind {
            BackendKind::Real => match &own_mapping_target {
                Some(mapped_target) => {
                    self.backend_path_for_storage(mapped_target, BackendKind::Real)?
                }
                None => self.real_backend_for_rel(&rel),
            },
            BackendKind::Redirect => {
                if decision.is_sandbox
                    && let Some(sandbox_root) = &self.sandbox_backend_root
                {
                    sandbox_root.join(&rel)
                } else {
                    self.redirect_backend_for_rel(&rel)
                }
            }
        };

        // 跨应用只读别名兜底：应用 A 把自身映射视图形态的文件路径交给应用 B 时
        // （fd 的 d_path 保留 A 会话里的视图名），B 的会话没有 A 的映射规则，按该
        // 路径的 stat/open 全部落 ENOENT（真机：夸克把 QuarkDownloads 下的 APK 交给
        // R-安装，安装器读不到元数据，安装面板信息空白但安装本身走 fd 流不受影响）。
        // 这里在 B 的 Read 未命中真实后端时，以「A 最近经自身映射访问过同一路径或
        // 其祖先」为门控信号，按 A 的映射规则换算出真实后端路径完成只读解析：
        // - 写操作、目录列举候选集、自身映射命中与真实存在的路径都不受影响；
        // - 门控不命中时本分支零动作，无关应用拼出的视图路径依旧 ENOENT，与改动
        //   前完全一致；即便信号在窗口内，能读到的也只是真实后端上本就可见的内容。
        if matches!(operation, OperationKind::Read)
            && decision.kind == BackendKind::Real
            && own_mapping_target.is_none()
            && has_recent_cross_app_handoff(&resolved_storage_path)
            && !backend.exists()
            && let Some(alias_target) = self.resolve_cross_app_read_alias(&resolved_storage_path)
            && let Some(alias_backend) =
                self.backend_path_for_storage(&alias_target, BackendKind::Real)
            && alias_backend.exists()
        {
            log::debug!(
                "fuse cross-app alias read pkg={} uid={} path={} alias={} fp={:016x}",
                self.package_name,
                self.uid,
                resolved_storage_path,
                alias_backend.display(),
                self.fingerprint
            );
            backend = alias_backend;
        }

        // 分享应用传递的文件路径时，目标应用的 scoped FUSE 仍会套用自身的
        // sandboxed_paths。若发送方文件尚未在目标应用沙箱中建立同名副本，目标
        // 应用会得到一个存在的重定向目录，却无法打开实际文件，表现为 QQ/微信
        // 分享面板没有文件或发送失败。对只读的具体文件保留沙箱优先；仅当重定向
        // 后端不存在、真实文件存在且路径命中沙箱规则时，回退到公共真实后端，
        // 让 Android 已授予的 content/file URI 能在目标 namespace 中完成读取。
        if matches!(operation, OperationKind::Read)
            && decision.kind == BackendKind::Redirect
            && self.matches_any(&self.sandboxed_index, &resolved_storage_path)
            && !self.matches_any(&self.sandboxed_excluded_index, &resolved_storage_path)
        {
            let real_backend = self.real_backend_for_rel(&rel);
            if !backend.exists() && real_backend.is_file() {
                log::debug!(
                    "fuse share read fallback pkg={} path={} backend={}",
                    self.package_name,
                    resolved_storage_path,
                    real_backend.display()
                );
                backend = real_backend;
            }
        }

        let is_shared_public_backend = self.is_shared_public_backend_path(&backend);
        Some(BackendPath {
            rel,
            path: backend,
            is_read_only: decision.is_read_only,
            is_shared_public_backend,
        })
    }

    pub(super) fn resolve_mapping(&self, storage_path: &str) -> Option<String> {
        for mapping in self.path_mappings.iter().rev() {
            if paths::matches(&mapping.request_path, storage_path, true) {
                let Some(suffix) = paths::child_suffix(storage_path, &mapping.request_path) else {
                    return Some(mapping.final_path.clone());
                };
                return Some(format!(
                    "{}{}",
                    mapping.final_path.trim_end_matches('/'),
                    suffix
                ));
            }
        }
        None
    }

    /// 在其它已登记应用策略的映射里解析跨应用只读别名目标。
    ///
    /// 遍历宿主进程按 uid 策略表中**其它应用**的 `path_mappings`，取请求侧命中的
    /// 最长规则换算目标路径；无匹配返回 `None`。调用方必须已确认 Read 兜底前提
    /// （自身映射未命中、真实后端不存在、交接信号在窗口内）。
    fn resolve_cross_app_read_alias(&self, storage_path: &str) -> Option<String> {
        let guard = CROSS_SESSION_POLICY_TABLE.lock().ok()?;
        let table = guard.as_ref()?;
        let by_uid = table.by_uid.read().ok()?;
        let mut matched: Option<(&PathMapping, usize)> = None;
        for (uid, policy) in by_uid.iter() {
            if *uid == self.uid as u32 {
                continue;
            }
            for mapping in &policy.path_mappings {
                if !paths::matches(&mapping.request_path, storage_path, true) {
                    continue;
                }
                let request_len = mapping.request_path.len();
                let is_longer = match &matched {
                    Some((_, best_len)) => request_len > *best_len,
                    None => true,
                };
                if is_longer {
                    matched = Some((mapping, request_len));
                }
            }
        }
        let (mapping, _) = matched?;
        let Some(suffix) = paths::child_suffix(storage_path, &mapping.request_path) else {
            return Some(mapping.final_path.clone());
        };
        Some(format!(
            "{}{}",
            mapping.final_path.trim_end_matches('/'),
            suffix
        ))
    }

    fn is_read_only(&self, storage_path: &str) -> bool {
        if let Some(mapped_target) = self.resolve_mapping(storage_path) {
            if self.matches_any(&self.excluded_real_index, &mapped_target) {
                return false;
            }
            if self.matches_any(&self.read_only_excluded_index, &mapped_target) {
                return false;
            }
            return self.matches_any(&self.read_only_index, &mapped_target);
        }
        if self.matches_any(&self.excluded_real_index, storage_path) {
            return false;
        }
        if self.matches_any(&self.read_only_excluded_index, storage_path) {
            return false;
        }
        if self.matches_any(&self.read_only_index, storage_path) {
            return true;
        }
        // 通配只读规则在 FUSE 中也以其具体父目录作为安全边界；这样未命中
        // 通配子目录的访问不会因后端切换而落入私有重定向路径。
        self.read_only_paths.iter().any(|rule| {
            paths::contains_wildcards(rule)
                && paths::wildcard_policy_fallback_parent(rule, &self.storage_root)
                    .is_some_and(|parent| paths::is_same_or_child(storage_path, &parent))
        })
    }

    fn backend_decision(&self, storage_path: &str, operation: OperationKind) -> BackendDecision {
        let is_read_only = self.is_read_only(storage_path);
        let mapping_hit = self.resolve_mapping(storage_path).is_some();
        // sandboxed_paths 优先于监视/直通会话短路：系统代写组件（共享 media uid
        // 等监视会话）对配置沙盒路径的访问也必须重定向进私有沙盒，否则 native
        // 层创建会绕过沙盒直落真实路径。命中与排除的判定口径与下方
        // mapping_mode_only 分支保持一致。映射优先于沙盒（场景 15 契约）：命中
        // 映射规则的路径由 Real 分支按映射目标解析，沙盒只接管未命中映射的路径
        // ——此前沙盒判定整体前置曾把「映射+沙盒双命中」劫进私有沙盒根。
        let is_sandbox = self.is_mapping_mode_only
            && !mapping_hit
            && self.matches_any(&self.sandboxed_index, storage_path)
            && !self.matches_any(&self.sandboxed_excluded_index, storage_path);
        // 系统代写代理对其它应用私有目录的访问按真实落点放行（见
        // is_foreign_private_storage_path 注释）；放在 else-if 链的 Real 分支
        // 里统一收口，避免出现同体分支。
        let delegate_foreign_private = !is_sandbox
            && is_system_write_delegate_session(&self.package_name)
            && self.is_foreign_private_storage_path(storage_path);
        let kind = if is_sandbox {
            BackendKind::Redirect
        } else if self.is_monitor_only
            || mapping_hit
            || self.is_own_private_storage_path(storage_path)
        {
            BackendKind::Real
        } else if self.is_mapping_mode_only {
            if self.is_foreign_private_storage_path(storage_path)
                && !is_system_write_delegate_session(&self.package_name)
            {
                BackendKind::Redirect
            } else {
                BackendKind::Real
            }
        } else if self.matches_any(&self.excluded_real_index, storage_path) {
            BackendKind::Redirect
        } else if self.matches_any(&self.read_only_excluded_index, storage_path)
            || self.matches_any(&self.allowed_real_index, storage_path)
            || delegate_foreign_private
            || matches!(operation, OperationKind::Read)
                && (is_read_only || self.has_real_child_rule(storage_path))
        {
            BackendKind::Real
        } else {
            BackendKind::Redirect
        };
        BackendDecision {
            kind,
            is_read_only,
            is_sandbox,
        }
    }

    fn matches_any(&self, index: &RuleIndex, storage_path: &str) -> bool {
        let pending_display_path = paths::media_store_pending_display_path(storage_path);
        index.matches(storage_path)
            || pending_display_path
                .as_deref()
                .is_some_and(|display_path| index.matches(display_path))
    }

    // 应用自己包名的私有目录（Android/data|media|obb/<pkg>）属于 app-specific
    // 外部存储，由系统 MediaProvider 直接映射到 /data/media/<user>，本就不该被
    // 重定向。若把它们当公共路径重定向，会落到 redirect_target 下再套一层
    // sdcard/，应用保存或下载到自己私有目录的文件会丢失可见性。这里在决策层
    // 直接判 Real，后续 real_backend_root_for_storage_rel 会自然选 private_real_root
    // 直连 f2fs，无需新增挂载点。
    fn is_own_private_storage_path(&self, storage_path: &str) -> bool {
        let Some(relative) = paths::relative_child_path(storage_path, &self.storage_root) else {
            return false;
        };
        let private_roots = [
            format!("Android/data/{}", self.package_name),
            format!("Android/media/{}", self.package_name),
            format!("Android/obb/{}", self.package_name),
        ];
        private_roots
            .iter()
            .any(|root| paths::matches(root, relative, true))
    }

    // mapping_mode_only 仍然可能使用覆盖整个共享存储根的 scoped FUSE。
    // 此时若把所有未命中沙盒规则的路径都直接落到 real_root，会绕过系统
    // MediaProvider 对其它应用 Android/data|media|obb/<pkg> 的可见性隔离。
    // 应用自有私有目录由上层的 is_own_private_storage_path 保持真实落点；
    // 其它应用的私有目录必须继续走 redirect 后端，使其在目录枚举和显式访问
    // 中都不会暴露真实文件。
    //
    // 例外：系统代写代理（MediaProvider/DownloadProvider/MTP/外部存储 provider）
    // 本来就是平台写其它应用私有目录的受信通道——HyperOS 商店下载即依赖
    // DownloadProvider 把安装介质代写到 `Android/data/com.xiaomi.market/` 下
    // （真机 2026-10-23：该写被判 Redirect 吸进下载提供者沙盒，商店校验侧
    // 读真实路径永远落空，`error=3` 无限重下）。代写会话的外来私有路径按
    // 真实落点解析，保持与原生 MediaProvider FUSE 一致的语义。
    fn is_foreign_private_storage_path(&self, storage_path: &str) -> bool {
        let Some(relative) = paths::relative_child_path(storage_path, &self.storage_root) else {
            return false;
        };
        let mut parts = relative.split('/').filter(|part| !part.is_empty());
        if parts.next() != Some("Android") {
            return false;
        }
        let Some(category) = parts.next() else {
            return false;
        };
        if !matches!(category, "data" | "media" | "obb") {
            return false;
        }
        let Some(package_name) = parts.next() else {
            return false;
        };
        !paths::eq_ignore_case(package_name, &self.package_name)
    }

    fn has_real_child_rule(&self, storage_path: &str) -> bool {
        self.allowed_real_paths
            .iter()
            .chain(self.read_only_paths.iter())
            .chain(
                self.path_mappings
                    .iter()
                    .map(|mapping| &mapping.request_path),
            )
            .any(|rule| rule_may_match_path_or_descendant(rule, storage_path))
    }

    pub(super) fn is_virtual_dir(&self, rel: &str) -> bool {
        let storage_path = self.storage_path_for_rel(rel);
        self.rule_prefixes.iter().any(|prefix| {
            paths::eq_ignore_case(&prefix.rel, rel)
                || paths::is_child(&prefix.storage_path, &storage_path)
                || (!prefix.full_prefix
                    && rule_has_path_prefix(&storage_path, &prefix.storage_path))
        })
    }

    pub(super) fn real_backend_for_rel(&self, rel: &str) -> PathBuf {
        let rel = self.full_storage_rel(rel);
        self.real_backend_for_storage_rel(&rel)
    }

    pub(super) fn redirect_backend_for_rel(&self, rel: &str) -> PathBuf {
        let rel = self.full_storage_rel(rel);
        self.redirect_backend_for_storage_rel(&rel)
    }

    pub(super) fn real_backend_for_storage_rel(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            return backend_overlay::overlay_backend_path(&self.real_root)
                .unwrap_or_else(|| self.real_root.clone());
        }
        let joined = self.real_backend_root_for_storage_rel(rel).join(rel);
        // 覆盖挂载感知：HyperOS 等系统会在 init 命名空间把下载暂存目录 bind 到
        // `/data/media/...` 子树（如应用商店 `.siexternal`），应用命名空间里的
        // scoped 会话按普通路径解析会落到被遮蔽的真实目录，与宿主/MP 读侧分裂。
        // 命中覆盖挂载时改写到物理源路径，未命中保持原路径（CI 断言不受影响）。
        backend_overlay::overlay_backend_path(&joined).unwrap_or(joined)
    }

    // 按相对路径选择真实后端根：应用自有私有目录走 private_real_root 直连 f2fs，
    // 其余共享公共目录仍走 real_root 以保留 MediaStore 索引与 pending 语义。
    //
    // 分流边界必须覆盖 Android 这一层本身，而不是只覆盖 Android/data|media|obb 及更深。
    // readdir 会用字符串相等比较枚举项路径与子项解析出的后端路径，父子异根时该子项会被
    // 静默丢弃：若 Android 归 real_root 而 Android/media 归 private_real_root，列举
    // Android 时三个私有子目录都会消失。同理也不能额外要求 <pkg> 段。
    //
    // 分流只能放在这个唯一的物理拼接点上：real_backend_for_rel、backend_path_for_storage
    // 以及 readdir 的两处直接调用都汇聚到此，放在更上层会漏掉 readdir 一侧。
    //
    // 应用自有 `Android/data|media|obb/<pkg>/` 始终固定落到真实存储根，包含首次创建、
    // 后续读取以及 SQLite 的 `.db`、`-wal`、`-shm`、`-journal` 文件。不能按文件存在性、
    // 文件类型或 MediaProvider 挂载状态在不同根目录间切换，否则数据库可能被分裂到不同
    // inode。`/data/media/<user>` 是应用关闭重定向后仍会访问的真实路径；显式路径映射
    // 仍在上层优先处理。MediaProvider 视图只用于缓存失效/自愈，不参与普通 I/O 路由。
    fn real_backend_root_for_storage_rel(&self, rel: &str) -> &PathBuf {
        if is_android_private_storage_subtree_relative_path(rel) {
            &self.private_real_root
        } else {
            &self.real_root
        }
    }

    /// MediaProvider FUSE 视图根（未绑定或不可用时为 `None`）。
    ///
    /// 不参与普通 I/O 路由，仅供毒化 shm 自愈在删除后端文件时走 view 侧 unlink，
    /// 让 MediaProvider 显式失效自己的缓存节点。
    pub(super) fn media_sqlite_view_root(&self) -> Option<&Path> {
        (!self.media_sqlite_real_root.as_os_str().is_empty())
            .then_some(self.media_sqlite_real_root.as_path())
    }

    /// 自有 media sqlite `-shm` 是否必须走宿主用户态通道（禁止 passthrough backing）。
    ///
    /// `-shm` 的毒化防御（ftruncate 钳制）只覆盖 FUSE SETATTR 通道；passthrough
    /// backing fd 上的 ftruncate 绕过宿主直落 f2fs。应用的 WAL 能力探针恰以截小
    /// `-shm` 判定介质可用性，放行 passthrough 会让每次建连的探针重新制造毒化
    /// inode，自愈只能救到下一次打开——表现就是跨应用共享库反复掉登录。
    /// 与 [`Self::should_heal_media_sqlite_shm_rel`] 同一判定集合（纯路径形态，
    /// 不含尺寸）。
    pub(super) fn should_exclude_media_sqlite_shm_passthrough_rel(&self, rel: &str) -> bool {
        self.should_heal_media_sqlite_shm_rel(rel)
    }

    /// 是否应在共享 FUSE 中检查并自愈 SQLite `-shm` 边车路径。
    ///
    /// 只圈定 `-shm`（不含 `.db`/`-wal`）：`-shm` 的合法形态只有「0 字节」与
    /// 「≥32KiB」两种（由 SQLite 维护），其余尺寸都是毒化态；`.db` 小于 32KiB 是
    /// 正常现象，`-wal` 过短时 SQLite 自己会按空日志重建，都不需要外部干预。
    pub(super) fn should_heal_media_sqlite_shm_rel(&self, rel: &str) -> bool {
        let is_own_media_sqlite = is_own_media_sqlite_relative_path(rel, &self.package_name);
        let is_monitor_only_private_sqlite = self.is_monitor_only
            && is_android_private_sqlite_relative_path(rel)
            && matches!(
                self.backend_decision(&self.storage_path_for_rel(rel), OperationKind::Read)
                    .kind,
                BackendKind::Real
            );
        if !is_own_media_sqlite && !is_monitor_only_private_sqlite {
            return false;
        }
        let file_name = rel.rsplit('/').next().unwrap_or(rel);
        file_name.len() > "-shm".len() && file_name.to_ascii_lowercase().ends_with("-shm")
    }

    // 判断某个枚举得到的后端路径是否是该 rel 在真实侧的合法后端。
    // 真实后端根按私有子树分流后，同一父目录下的子项可能落在 real_root 或
    // private_real_root，readdir 不能再用单一路径做相等比较：列举存储根时枚举源是
    // real_root，而子项 Android 解析到 private_real_root，纯相等判断会把它丢弃。
    pub(super) fn is_real_backend_path_for_storage_rel(&self, rel: &str, path: &Path) -> bool {
        let storage_path = if rel.is_empty() {
            self.storage_root.clone()
        } else {
            paths::join(&self.storage_root, rel)
        };
        if self
            .backend_decision(&storage_path, OperationKind::Read)
            .kind
            != BackendKind::Real
            || self.resolve_mapping(&storage_path).is_some()
        {
            return false;
        }
        let expected = self.real_backend_for_storage_rel(rel);
        if path == expected {
            return true;
        }
        // Android 私有子树（Android 及其 data/media/obb 子层）在 real_root 与
        // private_real_root 下各有同名实体：real_root 是 real_storage 锚点（叠在系统
        // MediaProvider FUSE 之上），private_real_root 直连 f2fs，二者指向同一物理目录，
        // 但 canonicalize 无法穿透 FUSE 挂载归一（真机：inode 相同、device 不同）。分流
        // 边界保证 rel 属 Android 私有子树时，从 real_root 枚举出的同名项必然是
        // private_real_root 那一份，按 real_root 侧路径放行即可，否则根列表缺 Android。
        if is_android_private_storage_subtree_relative_path(rel) {
            return path == self.real_root.join(rel);
        }
        // 枚举源与私有子树根可能经不同挂载别名（real_storage tmp 锚点 vs /data/media）
        // 指向同一物理目录：scoped 会话带 real_root_override 时，根列举出的 Android 项
        // 解析到 private_real_root，字符串相等判定会把它整棵丢弃（表现为根列表缺
        // Android，但按路径访问正常）。回退到规范路径比较，bind 到同一物理目录的别名
        // 会被 canonicalize 归一为同一路径。仅在字符串不等时触发，稳态零开销。
        let Ok(expected_canonical) = std::fs::canonicalize(&expected) else {
            return false;
        };
        std::fs::canonicalize(path).is_ok_and(|canonical| canonical == expected_canonical)
    }

    pub(super) fn redirect_backend_for_storage_rel(&self, rel: &str) -> PathBuf {
        if rel.is_empty() {
            return backend_overlay::overlay_backend_path(&self.redirect_root)
                .unwrap_or_else(|| self.redirect_root.clone());
        }
        let joined = self.redirect_root.join(rel);
        // 与 real 后端同口径：重定向根同样落在 `/data/media/...` 物理树上，
        // 需要经过同一套覆盖挂载感知解析（见 real_backend_for_storage_rel 注释）。
        backend_overlay::overlay_backend_path(&joined).unwrap_or(joined)
    }

    /// 折叠视图对重定向根自身的再进入：`<redirect_view_rel>/<rest>` → `<rest>`。
    ///
    /// 入参/出参都是全存储相对路径（含 mount_rel 前缀）。重定向根视图形态为空
    /// （直通/监视会话）时不折叠。
    fn strip_redirect_view_prefix(&self, full_rel: &str) -> Option<String> {
        let prefix = self.redirect_view_rel.as_str();
        if prefix.is_empty() {
            return None;
        }
        if full_rel == prefix {
            return Some(String::new());
        }
        let rest = full_rel.strip_prefix(prefix)?.strip_prefix('/')?;
        Some(rest.to_string())
    }

    /// 把全存储相对路径还原为 FUSE 根相对路径（mount_rel 非空时剥掉其前缀）。
    fn rel_for_full_storage_rel(&self, full_rel: &str) -> String {
        if self.mount_rel.is_empty() {
            return full_rel.to_string();
        }
        if full_rel.is_empty() {
            return String::new();
        }
        paths::relative_child_path(
            &paths::join(&self.storage_root, full_rel),
            &paths::join(&self.storage_root, &self.mount_rel),
        )
        .map(str::to_string)
        .unwrap_or_else(|| full_rel.to_string())
    }

    pub(super) fn backend_path_for_storage(
        &self,
        storage_path: &str,
        kind: BackendKind,
    ) -> Option<PathBuf> {
        let rel = paths::relative_child_path(storage_path, &self.storage_root)?;
        Some(match kind {
            BackendKind::Real => self.real_backend_for_storage_rel(rel),
            BackendKind::Redirect => self.redirect_backend_for_storage_rel(rel),
        })
    }

    pub(super) fn storage_path_for_rel(&self, rel: &str) -> String {
        let rel = self.full_storage_rel(rel);
        if rel.is_empty() {
            self.storage_root.clone()
        } else {
            paths::join(&self.storage_root, &rel)
        }
    }

    fn monitor_identify_method(&self) -> &'static str {
        if self.is_monitor_only {
            "fuse_monitor_only"
        } else {
            "fuse_redirect"
        }
    }

    fn monitor_source_for_backend(&self, backend: &BackendPath) -> &'static str {
        if self.is_monitor_only {
            return "fuse_monitor_only";
        }
        let display_path = self.storage_path_for_rel(&backend.rel);
        if self.resolve_mapping(&display_path).is_some() {
            "path_mapping"
        } else if display_path.contains("/Android/data/") && display_path.contains("/sdcard") {
            "sandbox_path"
        } else if backend.path.starts_with(&self.redirect_root) {
            "redirect_root"
        } else {
            "fuse_redirect"
        }
    }

    pub(super) fn emit_monitor_create(
        &self,
        backend: &BackendPath,
        caller: Option<&RequesterIdentity>,
    ) {
        if !self.is_file_monitor_enabled {
            return;
        }
        let (caller_pkg, identify_method) = monitor_caller_fields(self, caller);
        let display_path = self.storage_path_for_rel(&backend.rel);
        if display_path.is_empty()
            || crate::config::SettingsHub::instance()
                .should_filter_monitor_record(&display_path, "fuse_create")
        {
            return;
        }
        let backend_path = backend.path.to_string_lossy();
        if should_skip_duplicate_monitor_create(caller_pkg, &display_path, backend_path.as_ref()) {
            return;
        }
        let operation = if self.is_monitor_only {
            "fuse_monitor_create"
        } else {
            "fuse_create"
        };
        let source = self.monitor_source_for_backend(backend);
        log::info!(
            target: FILE_MONITOR_LOG_TAG,
            "{}|{}|{}|CREATE|{}|ret=0|errno=0|identify_method={}|identify_reliability=high|op={}|source={}|backend={}",
            build_monitor_timestamp(),
            caller_pkg,
            caller_pkg,
            display_path,
            identify_method,
            operation,
            source,
            backend_path
        );
    }

    /// 监视专用 FUSE 的成功写入记录。
    ///
    /// 这里记录的是 FUSE 请求已经完成之后的结果，路径由当前请求直接解析得到；不使用
    /// 父目录回填，也不把 inotify 的观察事件伪装成调用方事件。
    pub(super) fn emit_monitor_mutation(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        from_backend: Option<&BackendPath>,
        caller: Option<&RequesterIdentity>,
    ) {
        if !self.is_file_monitor_enabled {
            return;
        }
        let (caller_pkg, identify_method) = monitor_caller_fields(self, caller);
        let display_path = self.storage_path_for_rel(&backend.rel);
        if display_path.is_empty()
            || crate::config::SettingsHub::instance()
                .should_filter_monitor_record(&display_path, operation_name)
        {
            return;
        }
        let backend_path = backend.path.to_string_lossy();
        let mut line = format!(
            "{}|{}|{}|{}|{}|ret=0|errno=0|identify_method={}|identify_reliability=high|event_role=request|op={}|source={}|backend={}",
            build_monitor_timestamp(),
            caller_pkg,
            caller_pkg,
            monitor_event_kind_for_operation(operation_name),
            display_path,
            identify_method,
            operation_name,
            self.monitor_source_for_backend(backend),
            backend_path
        );
        if let Some(from_backend) = from_backend {
            let from_path = self.storage_path_for_rel(&from_backend.rel);
            if !from_path.is_empty() && from_path != display_path {
                line.push_str("|from=");
                line.push_str(&from_path);
            }
        }
        log::info!(target: FILE_MONITOR_LOG_TAG, "{}", line);
    }

    pub(super) fn emit_monitor_read_only_deny(&self, operation_name: &str, backend: &BackendPath) {
        self.emit_monitor_read_only_deny_with_from(operation_name, backend, None, libc::EROFS);
    }

    pub(super) fn emit_monitor_read_only_deny_with_errno(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        error_no: i32,
    ) {
        self.emit_monitor_read_only_deny_with_from(operation_name, backend, None, error_no);
    }

    pub(super) fn emit_monitor_read_only_deny_with_from(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        from_backend: Option<&BackendPath>,
        error_no: i32,
    ) {
        let Some(line) =
            self.monitor_read_only_deny_line(operation_name, backend, from_backend, error_no)
        else {
            return;
        };
        log::info!(target: FILE_MONITOR_LOG_TAG, "{}", line);
    }

    pub(super) fn monitor_read_only_deny_line(
        &self,
        operation_name: &str,
        backend: &BackendPath,
        from_backend: Option<&BackendPath>,
        error_no: i32,
    ) -> Option<String> {
        if !self.is_file_monitor_enabled {
            return None;
        }
        let display_path = self.storage_path_for_rel(&backend.rel);
        if display_path.is_empty()
            || crate::config::SettingsHub::instance()
                .should_filter_monitor_record(&display_path, "")
        {
            return None;
        }
        let backend_path = backend.path.to_string_lossy();
        let event_kind = monitor_event_kind_for_operation(operation_name);
        let mut line = format!(
            "{}|{}|{}|{}|{}|ret=-1|errno={}|identify_method=fuse_redirect|identify_reliability=high|op={}|source=fuse_redirect|backend={}|{}",
            build_monitor_timestamp(),
            self.package_name,
            self.package_name,
            event_kind,
            display_path,
            error_no,
            operation_name,
            backend_path,
            READ_ONLY_DENY_EXTRA
        );
        if let Some(from_backend) = from_backend {
            let from_path = self.storage_path_for_rel(&from_backend.rel);
            if !from_path.is_empty() && from_path != display_path {
                line.push_str("|from=");
                line.push_str(&from_path);
            }
        }
        Some(line)
    }

    pub(super) fn full_storage_rel(&self, rel: &str) -> String {
        let rel = rel.trim_matches('/');
        if self.mount_rel.is_empty() {
            rel.to_string()
        } else if rel.is_empty() {
            self.mount_rel.clone()
        } else {
            paths::join(&self.mount_rel, rel)
        }
    }

    pub(super) fn is_shared_public_backend_path(&self, path: &Path) -> bool {
        let path = path.to_string_lossy();
        let relative =
            paths::relative_child_path(&path, &paths::data_media_user_root_for_user(self.user_id))
                .or_else(|| {
                    paths::relative_child_path(&path, &real_storage_anchor_for_user(self.user_id))
                });
        let Some(relative) = relative else {
            return false;
        };
        !is_android_app_private_relative_path(relative)
    }
}

/// 登记本应用经自身映射访问过的视图路径，供跨应用只读别名兜底做交接门控。
///
/// 应用 A 通过 FileProvider 把 fd 交给应用 B 之前，必须先在自己的 FUSE 会话里以
/// 映射视图路径打开同一个文件；这条访问记录就是「B 正被 A 调用」的唯一可观测
/// 等价信号（FUSE 层看不到 intent/binder 调用链）。记录按精确路径写入，A 侧的
/// 组件级解析会自然把祖先目录一并登记；消费侧（[`has_recent_cross_app_handoff`]）
/// 再沿祖先链回溯匹配。
fn record_own_mapped_handoff(storage_path: &str) {
    let key = paths::match_key(storage_path);
    let Ok(mut handoffs) = RECENT_CROSS_APP_HANDOFFS.lock() else {
        return;
    };
    let now = paths::monotonic_ms();
    handoffs.insert(key, now);
    if handoffs.len() <= CROSS_APP_HANDOFF_CAPACITY {
        return;
    }
    handoffs.retain(|_, at| now - *at < CROSS_APP_HANDOFF_TTL_MS);
    if handoffs.len() <= CROSS_APP_HANDOFF_CAPACITY {
        return;
    }
    // 容量兜底：全部未过期时驱逐最旧一条，避免长会话进程内无界增长。
    if let Some(oldest) = handoffs
        .iter()
        .min_by_key(|(_, at)| **at)
        .map(|(key, _)| key.clone())
    {
        handoffs.remove(&oldest);
    }
}

/// 是否存在近窗口内其它应用经自身映射访问过该路径或其祖先的交接信号。
///
/// 沿祖先链回溯是必须的：FUSE LOOKUP 按组件逐级解析，A 交给 B 的是完整文件路径，
/// 但 B 的会话要先解析 `QuarkDownloads`、`CloudDrive` 等中间目录才能走到文件。
fn has_recent_cross_app_handoff(storage_path: &str) -> bool {
    let Ok(handoffs) = RECENT_CROSS_APP_HANDOFFS.lock() else {
        return false;
    };
    // 空表快速路径：设备上没有任何应用经自身映射产生交接记录时，热路径上
    // 不为每次 Read 请求构造 match_key（含一次分配）。
    if handoffs.is_empty() {
        return false;
    }
    let now = paths::monotonic_ms();
    let mut current = storage_path.trim_end_matches('/');
    loop {
        if handoffs
            .get(&paths::match_key(current))
            .is_some_and(|at| now - *at < CROSS_APP_HANDOFF_TTL_MS)
        {
            return true;
        }
        match current.rfind('/') {
            // 只在还有父目录时继续回溯；根路径（rfind == 0）与无分隔符路径终止。
            Some(pos) if pos > 0 => current = &current[..pos],
            _ => return false,
        }
    }
}

pub(super) fn real_backend_root_for_config(config: &FuseRedirectConfig, user_id: i32) -> PathBuf {
    if let Some(root) = config.real_root_override.as_deref() {
        let normalized = paths::normalize(root);
        if paths::eq_ignore_case(&normalized, &real_storage_anchor_for_user(user_id)) {
            return PathBuf::from(normalized);
        }
        log::warn!("fuse real root override ignored: {}", root);
    }
    PathBuf::from(paths::data_media_user_root_for_user(user_id))
}

pub(super) fn real_storage_anchor_for_user(user_id: i32) -> String {
    paths::join(module_paths::REAL_STORAGE_TMP_DIR, &user_id.to_string())
}

/// 宿主命名空间内 MediaProvider FUSE 视图根；仅当视图真实可用（用户子树能枚举到
/// Android 层，说明绑定已建立且不是空目录）时返回非空路径，否则返回空 `PathBuf`
/// 表示保持直连 f2fs 的既有行为。
///
/// 探测 `Android` 子目录而不是目录本身：宿主命名空间里 `tmp/fuse_host_media/<user>`
/// 在绑定建立前就是存在的空目录，只判目录存在会把未绑定误判为可用。
fn media_sqlite_view_root_for_user(user_id: i32) -> PathBuf {
    let root = PathBuf::from(module_paths::FUSE_HOST_MEDIA_VIEW_DIR).join(user_id.to_string());
    if fs::is_directory(&paths::join(&root.to_string_lossy(), "Android")) {
        root
    } else {
        PathBuf::new()
    }
}

/// 监视记录的调用方字段解析：系统请求者归因命中时以归因身份与
/// `identify_method=caller` 呈现（与 hook 侧归因链的记录语义一致），
/// 否则维持会话默认（宿主包名 + fuse_redirect/fuse_monitor_only）。
fn monitor_caller_fields<'a>(
    policy: &'a RedirectPolicy,
    caller: Option<&'a RequesterIdentity>,
) -> (&'a str, &'a str) {
    match caller {
        Some(identity) => (identity.package.as_str(), "caller"),
        None => (
            policy.package_name.as_str(),
            policy.monitor_identify_method(),
        ),
    }
}

fn build_monitor_timestamp() -> String {
    let mut now: libc::time_t = 0;
    // SAFETY: now 是栈上的合法可写指针，libc::time 只写入一个 time_t 值。
    unsafe { libc::time(&mut now as *mut _) };

    // SAFETY: libc::tm 是 C ABI 结构体，全零初始化是合法的起始状态，后续由 localtime_r 填充。
    let mut tm_value: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: now 和 tm_value 均为有效的本地变量，指针在调用期间保持有效。
    let tm_ptr = unsafe { libc::localtime_r(&now as *const _, &mut tm_value as *mut _) };
    if tm_ptr.is_null() {
        return String::new();
    }

    let mut buffer = [0u8; 32];
    let format = b"%Y-%m-%d %H:%M:%S\0";
    // SAFETY: buffer 是有效的可写缓冲区，format 是以 NUL 结尾的合法格式字符串，tm_value 已由 localtime_r 填充，三个指针在调用期间均保持有效。
    let written = unsafe {
        libc::strftime(
            buffer.as_mut_ptr() as *mut _,
            buffer.len(),
            format.as_ptr() as *const _,
            &tm_value as *const _,
        )
    };
    if written == 0 {
        return String::new();
    }
    String::from_utf8_lossy(&buffer[..written]).to_string()
}

fn should_skip_duplicate_monitor_create(
    package_name: &str,
    display_path: &str,
    backend_path: &str,
) -> bool {
    if package_name.is_empty() || display_path.is_empty() || backend_path.is_empty() {
        return false;
    }
    let key = format!("{}|{}|{}", package_name, display_path, backend_path);
    let now_ms = paths::monotonic_ms();
    // 该函数可能在 fork 出来的 FUSE 子进程里执行，阻塞获取全局锁会因为父进程线程持锁而永久卡死；
    // 去重只是优化项，拿不到锁时按未去重处理即可。
    let Ok(mut recent) = RECENT_MONITOR_CREATES.try_lock() else {
        return false;
    };
    if let Some(last_ms) = recent.get_mut(&key) {
        if now_ms.saturating_sub(*last_ms) < DUPLICATE_MONITOR_CREATE_WINDOW_MS {
            *last_ms = now_ms;
            return true;
        }
        *last_ms = now_ms;
        return false;
    }
    if recent.len() >= MAX_RECENT_MONITOR_CREATES {
        recent.retain(|_, last_ms| {
            now_ms.saturating_sub(*last_ms) < DUPLICATE_MONITOR_CREATE_WINDOW_MS
        });
    }
    recent.insert(key, now_ms);
    false
}

pub(super) fn build_rule_prefixes(
    config: &FuseRedirectConfig,
    path_mappings: &[PathMapping],
    user_id: i32,
    storage_root: &str,
    mount_root: &str,
) -> Vec<RulePrefix> {
    let mut prefixes = Vec::new();
    for rule in config
        .allowed_real_paths
        .iter()
        .chain(config.excluded_real_paths.iter())
        .chain(config.sandboxed_paths.iter())
        .chain(config.read_only_paths.iter())
    {
        if let Some(prefix) = visible_rule_prefix(rule, user_id, storage_root, mount_root) {
            prefixes.push(prefix);
        }
    }
    for mapping in path_mappings {
        if let Some(prefix) =
            visible_rule_prefix(&mapping.request_path, user_id, storage_root, mount_root)
        {
            prefixes.push(prefix);
        }
    }
    prefixes.sort_by(|left, right| left.rel.cmp(&right.rel));
    prefixes.dedup_by(|left, right| paths::eq_ignore_case(&left.rel, &right.rel));
    prefixes
}

fn visible_rule_prefix(
    rule: &str,
    user_id: i32,
    storage_root: &str,
    mount_root: &str,
) -> Option<RulePrefix> {
    let rule = rule.trim_start();
    let rule = rule.strip_prefix('!').unwrap_or(rule).trim_start();
    let full_prefix = !paths::contains_wildcards(rule);
    let rule_prefix = concrete_rule_prefix(rule)?;
    let mut resolved = paths::resolve_user_path(&paths::normalize(&rule_prefix), user_id);
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return None;
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if !paths::is_child(&resolved, storage_root) && !paths::eq_ignore_case(&resolved, storage_root)
    {
        return None;
    }
    if !paths::eq_ignore_case(&resolved, mount_root) && !paths::is_child(&resolved, mount_root) {
        return None;
    }
    let rel = paths::relative_child_path(&resolved, mount_root)
        .unwrap_or("")
        .trim_matches('/')
        .to_string();
    if rel.is_empty() {
        return None;
    }
    // resolved 已是该规则前缀对应的存储路径，等价于策略侧的 storage_path_for_rel(rel)。
    Some(RulePrefix {
        rel,
        storage_path: resolved,
        full_prefix,
    })
}

fn concrete_rule_prefix(rule: &str) -> Option<String> {
    let normalized = paths::normalize(rule);
    if !paths::contains_wildcards(&normalized) {
        return Some(normalized);
    }
    let parts: Vec<&str> = normalized.split('/').collect();
    let mut kept = Vec::new();
    for part in parts {
        if part.contains('*') || part.contains('?') {
            break;
        }
        kept.push(part);
    }
    let prefix = kept.join("/");
    if prefix.is_empty() || prefix == "/" {
        None
    } else {
        Some(prefix)
    }
}

pub(super) fn visible_prefix_child(parent_rel: &str, prefix: &RulePrefix) -> Option<String> {
    let prefix_rel = prefix.rel.trim_matches('/');
    if prefix_rel.is_empty() {
        return None;
    }
    if parent_rel.is_empty() {
        return prefix_rel.split('/').next().map(ToString::to_string);
    }
    let parent = parent_rel.trim_matches('/');
    if prefix_rel.eq_ignore_ascii_case(parent) {
        return None;
    }
    let parent_prefix = format!("{}/", parent);
    if !prefix_rel
        .get(..parent_prefix.len())
        .is_some_and(|value| value.eq_ignore_ascii_case(&parent_prefix))
    {
        return None;
    }
    let rest = &prefix_rel[parent_prefix.len()..];
    if rest.is_empty() {
        return None;
    }
    Some(rest.split('/').next().unwrap_or(rest).to_string())
}

fn rule_has_path_prefix(rule: &str, path: &str) -> bool {
    if rule.is_empty() || path.is_empty() {
        return false;
    }
    if paths::matches(rule, path, true) {
        return true;
    }
    let rule_norm = paths::normalize(rule);
    let path_norm = paths::normalize(path);
    if paths::contains_wildcards(&rule_norm) {
        return false;
    }
    paths::is_child(&path_norm, &rule_norm) || paths::eq_ignore_case(&path_norm, &rule_norm)
}

fn rule_may_match_path_or_descendant(rule: &str, path: &str) -> bool {
    if rule.is_empty() || path.is_empty() {
        return false;
    }
    if paths::matches(rule, path, true) {
        return true;
    }

    let rule_segments: Vec<&str> = rule.split('/').filter(|part| !part.is_empty()).collect();
    let path_segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if path_segments.is_empty() || path_segments.len() > rule_segments.len() {
        return false;
    }

    let rule_prefix = format!("/{}", rule_segments[..path_segments.len()].join("/"));
    let path_prefix = format!("/{}", path_segments.join("/"));
    paths::matches(&rule_prefix, &path_prefix, false)
}

fn resolve_path_mappings(
    mappings: &[PathMapping],
    user_id: i32,
    storage_root: &str,
    app_data_dir: &str,
    redirect_target: &str,
) -> Vec<PathMapping> {
    let mut resolved = Vec::with_capacity(mappings.len());
    for mapping in mappings {
        let Some(current) = resolve_storage_rule(
            &mapping.request_path,
            user_id,
            storage_root,
            app_data_dir,
            redirect_target,
        ) else {
            continue;
        };
        let Some(target) = resolve_storage_rule(
            &mapping.final_path,
            user_id,
            storage_root,
            app_data_dir,
            redirect_target,
        ) else {
            continue;
        };
        if paths::eq_ignore_case(&current, &target) {
            continue;
        }
        resolved.push(PathMapping::new(current, target));
    }
    resolved
}

fn resolve_storage_rule(
    path: &str,
    user_id: i32,
    storage_root: &str,
    app_data_dir: &str,
    redirect_target: &str,
) -> Option<String> {
    let mut resolved = paths::resolve_user_path(
        &paths::resolve_placeholders(&paths::normalize(path), app_data_dir, redirect_target),
        user_id,
    );
    if resolved.is_empty() || paths::has_unsafe_segments(&resolved) {
        return None;
    }
    if !paths::is_absolute(&resolved) {
        resolved = paths::normalize(&paths::join(storage_root, &resolved));
    }
    if paths::is_child(&resolved, storage_root) {
        Some(resolved)
    } else {
        None
    }
}

pub(super) fn sanitize_relative(rel: &str) -> Option<String> {
    let trimmed = rel.trim_matches('/');
    if trimmed.is_empty() {
        return Some(String::new());
    }
    if trimmed.contains('\0') || paths::has_unsafe_segments(trimmed) || paths::is_absolute(trimmed)
    {
        return None;
    }
    Some(paths::normalize(trimmed).trim_matches('/').to_string())
}

pub(super) fn monitor_event_kind_for_operation(operation_name: &str) -> &'static str {
    match operation_name.split(':').next().unwrap_or(operation_name) {
        "open" => "OPEN",
        "write" => "WRITE",
        "create" => "CREATE",
        "mkdir" => "MKDIR",
        "rename" => "RENAME",
        "unlink" => "UNLINK",
        "rmdir" => "RMDIR",
        "truncate" => "TRUNCATE",
        "chmod" | "chown" | "utimens" | "setattr" => "ATTRIB",
        "access" => "ACCESS",
        _ => "WRITE",
    }
}

pub(super) fn fix_mapped_dir_metadata(path: &str, owner_uid: i32) {
    if !crate::metadata_repair::enabled() {
        return;
    }
    if let Ok(c_path) = CString::new(path) {
        // Android 私有外部存储由 MediaProvider 管理，已有目录的 owner/mode 不能被
        // FUSE 初始化改写；否则应用会被迫额外配置 allowed/read-only 才能访问。新建
        // 目标仍由后续创建流程初始化，已有目录只保留系统元数据。
        if let Some(user_id) = paths::extract_user_id_from_data_media_path(path)
            && is_android_private_backend_path(path, user_id)
            && !is_android_private_package_root(path, user_id)
            && fs::is_directory(path)
        {
            log::debug!("fuse keep existing Android private metadata path={}", path);
            return;
        }
        // 私有树（含包根）对齐原生 02770；此前包根会被打成 MAPPED_DIR_MODE
        // 0o2773（others 可写可穿越），与 MediaProvider 原生语义相悖——
        // 2026-10-21 用户反馈「微信私有目录 2773」即此来源。
        let mode = if let Some(user_id) = paths::extract_user_id_from_data_media_path(path)
            && is_android_private_backend_path(path, user_id)
        {
            0o2770
        } else {
            super::MAPPED_DIR_MODE
        };
        // SAFETY: c_path 以 NUL 结尾，并在 chown/chmod 调用期间保持有效。
        let _ = unsafe { libc::chown(c_path.as_ptr(), owner_uid as u32, super::MEDIA_RW_GID) };
        // SAFETY: c_path 以 NUL 结尾，并在 chmod 调用期间保持有效。
        let _ = unsafe { libc::chmod(c_path.as_ptr(), mode) };
    }
}

fn is_android_private_backend_path(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    parts.next() == Some("Android") && matches!(parts.next(), Some("data" | "media" | "obb"))
}

fn is_android_private_package_root(path: &str, user_id: i32) -> bool {
    let root = paths::data_media_user_root_for_user(user_id);
    let Some(relative) = paths::relative_child_path(path, &root) else {
        return false;
    };
    relative.split('/').filter(|part| !part.is_empty()).count() == 3
        && is_android_private_backend_path(path, user_id)
}

fn is_android_app_private_relative_path(relative: &str) -> bool {
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    matches!(parts.next(), Some("data" | "media" | "obb"))
}

// 判断相对路径是否属于走 private_real_root 的私有子树，包含 Android 目录节点自身。
// 覆盖 Android 这一层是为了让它与 Android/data|media|obb 同根，避免 readdir 因父子
// 异根而丢弃这三个子目录；Android 下的其余条目同源同分区，改根不影响可见性。
fn is_android_private_storage_subtree_relative_path(relative: &str) -> bool {
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    match parts.next() {
        None => true,
        Some(second) => matches!(second, "data" | "media" | "obb"),
    }
}

/// 判断相对路径是否是会话自有 `Android/media/<pkg>/` 下的 sqlite 数据库或边车文件。
///
/// 只有这类文件需要经 MediaProvider FUSE 视图访问：SQLite 的 WAL 依赖跨进程共享的
/// `-shm` mmap 与文件锁，宿主直连 f2fs 与其它调用方经 MediaProvider FUSE 的内核缓存
/// 是两个互不通知的缓存层，数据库文件删除重建后两层各持一个代际，读取撕裂表现为
/// `SQLITE_CORRUPT`。大小写判定与 `is_android_private_storage_subtree_relative_path`
/// 保持同一约定（rel 中目录段为规范小写/大小写）。
fn is_own_media_sqlite_relative_path(relative: &str, package_name: &str) -> bool {
    if package_name.is_empty() || !paths::is_sqlite_database_or_sidecar_path(relative) {
        return false;
    }
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    if parts.next() != Some("media") {
        return false;
    }
    match parts.next() {
        Some(package) => paths::eq_ignore_case(package, package_name),
        None => false,
    }
}

/// 判断相对路径是否位于任一应用私有外部存储目录下的 SQLite 文件。
///
/// 监视专用会话可能代表应用读取其它应用拥有的数据库；这类跨应用访问不满足
/// `is_own_media_sqlite_relative_path`，但损坏的 `-shm` 仍必须删除旧 inode 后重建。
fn is_android_private_sqlite_relative_path(relative: &str) -> bool {
    if !paths::is_sqlite_database_or_sidecar_path(relative) {
        return false;
    }
    let mut parts = relative.split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("Android") {
        return false;
    }
    matches!(parts.next(), Some("data" | "media" | "obb")) && parts.next().is_some()
}
