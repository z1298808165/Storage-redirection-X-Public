//! 后端路径的 init 命名空间覆盖挂载感知解析。
//!
//! 背景（真机 2026-10-19，HyperOS 应用商店 speed-install，`error=3`）：
//! HyperOS 会在系统命名空间把 `/data/apk-tmp/<pkg>/download` bind 到
//! `/data/media/<user>/Android/data/<pkg>/files/.siexternal`。这个 bind 位于
//! MediaProvider 与共享宿主会话（见 `fuse_host.rs` 的 `MS_SLAVE` 说明）的传播域内，
//! 但**不在应用命名空间里**。scoped FUSE 会话 worker 跟随应用命名空间服务
//! （挂载 worker 先 setns 再挂载），按普通路径解析 `/data/media/...` 后端时落
//! 到被 bind 遮蔽之前的真实目录：下载侧写入真实目录、商店校验侧经宿主/MP 读到
//! bind 目标 apk-tmp，两侧各执一份物理目录，商店读不到自己刚下载的文件。
//!
//! 修法：`/data/media/...` 后端路径解析前查 **init 命名空间**（`/proc/1/root`
//! 代表的挂载视图，即系统挂载授权方工作的地方）里是否有覆盖该路径的挂载；命中
//! 则把后端改写到覆盖挂载的物理源路径，使 scoped 会话的写侧与宿主/MP 的读侧
//! 汇聚到同一物理目录。无覆盖挂载时返回 `None`，路径保持原样——CI 场景与既有
//! 测试的后端路径断言不受任何影响。
//!
//! 改写到物理源路径（而不是 `/proc/1/root/...` 前缀）的原因：日志、监视台账与
//! 路径断言里出现的是真实落盘位置，可直接与真机 `ls`/`find` 证据互证；同时
//! `/proc/1/root` 前缀会让路径字符串随诊断上下文漂移，难以比较。
//!
//! 属主安全：覆盖目标（apk-tmp 目录树）由系统按下载方应用属主自管，本模块只做
//! **路径改写**，不改写既有节点的属主与权限；调用方（`ensure_parent_for_backend`）
//! 据此对覆盖路径跳过既有目录的元数据修复。
//!
//! 后端根的写法与覆盖表不统一（真机 2026-11-17）：宿主会话的数据面是 f2fs 锚点
//! `real_media_anchor/<user>`，它是 `/data/media/<user>` 的**非递归** bind，层级上
//! 看不到 init 事后在同一子树建的 bind；而覆盖表只登记 `/data/media/<user>` 写法。
//! 因此所有入口都先经 [`canonical_backend_form`] 换算，换算必须带 `user_id`——
//! 锚点字符串自带用户段，任何丢弃该段的换算都会让查询恒不命中，宿主视图于是退回
//! bind 遮蔽前的陈旧目录（应用商店下载完成、校验读不到包、安装不发起）。

use crate::platform::mountinfo::{parse_entry, unescape_field};
use crate::platform::paths;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// mountinfo 快照刷新间隔。HyperOS 的下载 bind 随下载任务创建与拆除，1 秒粒度
/// 足够跟上生命周期，同时把解析开销摊薄到可忽略（多线程共享同一份快照）。
const SNAPSHOT_TTL: Duration = Duration::from_secs(1);

/// 后端树的根前缀：覆盖判定只服务 `/data/media/...` 物理后端。
const BACKEND_TREE_PREFIX: &str = "/data/media";

/// 伪文件系统不承载后端数据，覆盖判定没有意义；`fuse` 前缀另行整族排除
/// （含 `fuse.srx` 本模块会话）。
const EXCLUDED_FS_TYPES: [&str; 10] = [
    "proc",
    "sysfs",
    "devtmpfs",
    "devpts",
    "selinuxfs",
    "tracefs",
    "debugfs",
    "bpf",
    "functionfs",
    "configfs",
];

#[derive(Clone)]
struct OverlayMount {
    /// 设备号（`major:minor`），用于寻找该文件系统根挂载点。
    dev: String,
    /// init 视图中的挂载点（已展开八进制转义），如
    /// `/data/media/0/Android/data/com.xiaomi.market/files/.siexternal`。
    mount_point: String,
    /// 覆盖挂载在文件系统内的根（已展开转义），如
    /// `/apk-tmp/com.xiaomi.market/download`。
    root_in_fs: String,
}

#[derive(Default)]
struct Snapshot {
    /// `/data` 子树内的覆盖挂载候选（按 mountinfo 文件序）。
    overlays: Vec<OverlayMount>,
    /// 设备号 → 该文件系统根（`root=="/"`）的挂载点，如 `254:60 → /data`。
    fs_roots: HashMap<String, String>,
}

static SNAPSHOT: Mutex<Option<(Instant, Arc<Snapshot>)>> = Mutex::new(None);

/// 覆盖表代次：每次装载新快照就前进一位。
///
/// 上层按 inode 缓存后端解析结果（`FuseState::inode_backends`），而覆盖挂载可能在会话
/// 运行中途出现或消失。缓存若不带代次，首次解析落在 [`SNAPSHOT_TTL`] 窗口内就会把
/// 「bind 遮蔽前的后端」钉死到该 inode 的生命周期结束——真机 2026-11-17 的 A16 偶发
/// 视图不一致即由此而来：应用侧先查过该目录，1 秒内系统才建 bind，之后无论重试多少
/// 次都读回旧目录。带上代次后，缓存最多陈旧 [`SNAPSHOT_TTL`]，与覆盖表自身的观测
/// 延迟同阶。
static SNAPSHOT_GENERATION: AtomicU64 = AtomicU64::new(1);

/// 快照最后一次刷新的时刻（Unix 毫秒）。供 [`snapshot_generation`] 做无锁的 TTL 判断，
/// 避免每个 FUSE 操作都去抢 [`SNAPSHOT`] 锁。
static SNAPSHOT_REFRESHED_MS: AtomicU64 = AtomicU64::new(0);

/// 快照自检日志是否已输出过（每进程一次）：`debug` 看得到「读到了多少」，`warn` 看得到
/// 「根本读不到」，避免每秒刷屏。
static SNAPSHOT_LOGGED: AtomicBool = AtomicBool::new(false);

/// 上一次记录过的覆盖表签名（覆盖挂载点集合的哈希）。只用于「变化时打一条日志」，
/// 不参与任何解析判定。
static SNAPSHOT_SIGNATURE: Mutex<Option<u64>> = Mutex::new(None);

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// 当前覆盖表代次。快照超过 TTL 时先刷新一次再返回，保证调用方拿到的是「至多滞后
/// 一个 TTL」的代次；未过期时只做一次原子读，不进锁。
pub(super) fn snapshot_generation() -> u64 {
    let refreshed = SNAPSHOT_REFRESHED_MS.load(Ordering::Relaxed);
    if now_unix_ms().saturating_sub(refreshed) >= SNAPSHOT_TTL.as_millis() as u64 {
        let _ = cached_snapshot();
    }
    SNAPSHOT_GENERATION.load(Ordering::Relaxed)
}

fn load_snapshot() -> Snapshot {
    let mut snapshot = Snapshot::default();
    let Ok(content) = std::fs::read_to_string("/proc/1/mountinfo") else {
        // 非 Android 环境（本地测试）或权限受限：视为无覆盖挂载，路径保持原样。
        // 但「读不到」和「读到了但没有覆盖挂载」在排查时是完全相反的结论：前者会让整条
        // 补偿链静默失效（2026-11-18 场景 44 在 CI 上就是这么表现的），所以这里留一条
        // 一次性 warn，别让权限问题伪装成「没有覆盖挂载」。
        if !SNAPSHOT_LOGGED.swap(true, Ordering::Relaxed) {
            log::warn!("overlay snapshot unavailable: cannot read /proc/1/mountinfo");
        }
        return snapshot;
    };
    for line in content.lines() {
        let Some(entry) = parse_entry(line) else {
            continue;
        };
        let is_virtual = EXCLUDED_FS_TYPES.contains(&entry.fs_type)
            || entry.fs_type.starts_with("fuse")
            || entry.fs_type.starts_with("sdcardfs");
        if is_virtual {
            continue;
        }
        let dev = entry.dev.to_string();
        let target = unescape_field(entry.target);
        let root_in_fs = unescape_field(entry.root);
        if root_in_fs == "/" && !snapshot.fs_roots.contains_key(&dev) {
            snapshot.fs_roots.insert(dev.clone(), target.clone());
        }
        // 只登记可能遮蔽 `/data/media` 后端的挂载点（含 `/data/media` 本身）。
        if target == "/data/media" || target.starts_with("/data/media/") {
            snapshot.overlays.push(OverlayMount {
                dev,
                mount_point: target,
                root_in_fs,
            });
        }
    }
    // 覆盖表发生变化时记一条 debug：这是判断「本进程到底有没有看见后续新建的 bind」
    // 的唯一现场。只在变化时打，稳态零输出；排查场景 44 时就是缺这条，才出现「改写一次
    // 都没发生，但不知道是没看见挂载还是没匹配上」的僵局。
    log_overlay_change(&snapshot);
    snapshot
}

/// 覆盖表变化日志：mounts 计数 + 覆盖挂载点前若干条，便于直接与现场 `mountinfo` 对照。
fn log_overlay_change(snapshot: &Snapshot) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    snapshot.overlays.len().hash(&mut hasher);
    for mount in &snapshot.overlays {
        mount.mount_point.hash(&mut hasher);
        mount.root_in_fs.hash(&mut hasher);
    }
    let signature = hasher.finish();
    {
        let mut guard = SNAPSHOT_SIGNATURE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard.as_ref() == Some(&signature) {
            return;
        }
        *guard = Some(signature);
    }
    let sample = snapshot
        .overlays
        .iter()
        .rev()
        .take(3)
        .map(|mount| mount.mount_point.as_str())
        .collect::<Vec<_>>()
        .join(",");
    log::debug!(
        "overlay snapshot changed overlays={} fs_roots={} newest=[{}]",
        snapshot.overlays.len(),
        snapshot.fs_roots.len(),
        sample
    );
}

fn cached_snapshot() -> Arc<Snapshot> {
    let mut guard = SNAPSHOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((fetched_at, snapshot)) = guard.as_ref()
        && fetched_at.elapsed() < SNAPSHOT_TTL
    {
        return snapshot.clone();
    }
    let snapshot = Arc::new(load_snapshot());
    *guard = Some((Instant::now(), snapshot.clone()));
    SNAPSHOT_GENERATION.fetch_add(1, Ordering::Relaxed);
    SNAPSHOT_REFRESHED_MS.store(now_unix_ms(), Ordering::Relaxed);
    snapshot
}

/// 组件级前缀判断：`prefix == path` 或 `path` 位于 `prefix` 目录之下。
fn is_component_prefix(prefix: &str, path: &str) -> bool {
    if prefix == path {
        return true;
    }
    let prefix = prefix.trim_end_matches('/');
    path.len() > prefix.len() && path.starts_with(prefix) && path.as_bytes()[prefix.len()] == b'/'
}

fn join_fs_path(fs_root: &str, root_in_fs: &str, rest: &str) -> String {
    let mut out = String::with_capacity(fs_root.len() + root_in_fs.len() + rest.len() + 1);
    out.push_str(fs_root.trim_end_matches('/'));
    let trimmed_root = root_in_fs.trim_end_matches('/');
    if !trimmed_root.is_empty() {
        if !trimmed_root.starts_with('/') {
            out.push('/');
        }
        out.push_str(trimmed_root);
    }
    // rest 来自后端路径去掉挂载点前缀的剩余部分：要么为空，要么以 '/' 开头。
    out.push_str(rest);
    out
}

static LAST_LOGGED_OVERLAY: Mutex<Option<String>> = Mutex::new(None);

/// 把后端路径换算成覆盖挂载表登记的规范形态 `/data/media/<user>/…`。
///
/// 后端根有三族：`/data/media/<user>`、宿主 f2fs 锚点（`real_media_anchor/<user>`，
/// `data_media_user_root_for_current_ns` 的返回值）与 MP 视图。三族在磁盘上是同一棵
/// 树的不同写法，而 init 侧登记的覆盖挂载**只用 `/data/media/<user>` 一种写法**，
/// 所以查询前必须换算。
///
/// 换算必须由调用方给出 `user_id`：锚点字符串本身带用户段（`…/real_media_anchor/0`
/// 等价于 `/data/media/0`），任何「把锚点前缀映射成不带用户的 `/data/media`」的写法
/// 都会丢掉 `<user>` 段，使查询串与挂载点永不相等、改写恒不命中（真机 2026-11-17：
/// 应用商店下载完成却无法安装，`.siexternal` 读到 bind 遮蔽前的陈旧目录）。
///
/// 返回 `None` 表示该路径不在 `/data/media` 物理树上（如 MP 视图），覆盖判定无意义。
fn canonical_backend_form<'a>(backend: &'a Path, user_id: i32) -> Option<Cow<'a, str>> {
    let backend_text = backend.to_str()?;
    // 与 `data_media_user_root_for_current_ns` 同一判据（锚点只服务主用户）：无锚点时
    // 当前根就是规范根，此路径不分配，是 daemon 与应用命名空间里的常态。
    if user_id == 0
        && let Some(anchor) = paths::media_real_anchor_root()
        && let Some(rest) = backend_text.strip_prefix(anchor)
        && (rest.is_empty() || rest.starts_with('/'))
    {
        return Some(Cow::Owned(format!("{BACKEND_TREE_PREFIX}/{user_id}{rest}")));
    }
    is_component_prefix(BACKEND_TREE_PREFIX, backend_text).then_some(Cow::Borrowed(backend_text))
}

/// 解析后端路径在 init 命名空间视图下的物理源路径。
///
/// 返回 `None` 表示无覆盖挂载或改写结果与原路径相同（恒等映射），调用方按
/// 原路径处理即可。改写仅在命中覆盖挂载时发生，稳态（无 bind）下零路径变化。
pub(super) fn overlay_backend_path(backend: &Path, user_id: i32) -> Option<PathBuf> {
    let Some(canonical) = canonical_backend_form(backend, user_id) else {
        // 换算不出规范形态（锚点族写法与当前进程不一致）时，这里静默返回 None 会让整条
        // 补偿链看起来「没命中」而查不出原因，采样留痕用于对照现场路径。
        log_overlay_query_sample("canonical-none", &backend.to_string_lossy(), None, 0);
        return None;
    };
    let snapshot = cached_snapshot();
    let resolved = resolve_with_snapshot(&canonical, &snapshot);
    // 只采样「落在某个覆盖挂载点之内」的查询：这些才是本该命中改写的路径，父级 miss
    // 是正常现象，混进来会把关键那条挤出采样窗口（2026-11-18 CI 现场就是这样）。
    let covering = snapshot
        .overlays
        .iter()
        .filter(|mount| is_component_prefix(&mount.mount_point, &canonical))
        .max_by_key(|mount| mount.mount_point.len());
    if let Some(covering) = covering {
        log_overlay_query_sample(
            if resolved.is_some() { "hit" } else { "miss" },
            &canonical,
            Some(covering.mount_point.as_str()),
            snapshot.overlays.len(),
        );
    }
    resolved
}

/// 覆盖查询采样日志：前 20 次全量、之后每 2048 次一条，只打 debug，用于回答
/// 「查询串长什么样、覆盖表里最新一条是什么、两者为什么没对上」。
fn log_overlay_query_sample(kind: &str, query: &str, newest: Option<&str>, overlays: usize) {
    static SAMPLES: AtomicU64 = AtomicU64::new(0);
    let seen = SAMPLES.fetch_add(1, Ordering::Relaxed) + 1;
    if seen > 20 && !seen.is_multiple_of(2048) {
        return;
    }
    log::debug!(
        "overlay query kind={} n={} overlays={} query={} newest={}",
        kind,
        seen,
        overlays,
        query,
        newest.unwrap_or("-")
    );
}

/// 与 [`overlay_backend_path`] 相同，但绕过快照 TTL 强制重读 init mountinfo。
///
/// 仅供新建入口（create/mkdir）使用：HyperOS 的下载 bind 在任务入队瞬间创建，
/// 1 秒快照可能刚好错过，导致首次 CREATE 落到被遮蔽的真实目录、整次下载与
/// 商店校验读写分裂。新建是低频操作，重读 mountinfo 的开销可以接受。
pub(super) fn overlay_backend_path_fresh(backend: &Path, user_id: i32) -> Option<PathBuf> {
    let canonical = canonical_backend_form(backend, user_id)?;
    let snapshot = Arc::new(load_snapshot());
    if let Some(slot) = SNAPSHOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_mut()
    {
        *slot = (Instant::now(), snapshot.clone());
    }
    SNAPSHOT_GENERATION.fetch_add(1, Ordering::Relaxed);
    SNAPSHOT_REFRESHED_MS.store(now_unix_ms(), Ordering::Relaxed);
    resolve_with_snapshot(&canonical, &snapshot)
}

fn resolve_with_snapshot(backend_text: &str, snapshot: &Snapshot) -> Option<PathBuf> {
    // 最长挂载点优先：同一路径可能被多层嵌套 bind 逐级覆盖，最上层的才是可见层。
    let mut best: Option<&OverlayMount> = None;
    for mount in &snapshot.overlays {
        if !is_component_prefix(&mount.mount_point, backend_text) {
            continue;
        }
        let better = match best {
            Some(current) => mount.mount_point.len() > current.mount_point.len(),
            None => true,
        };
        if better {
            best = Some(mount);
        }
    }
    let mount = best?;
    let Some(fs_root) = snapshot.fs_roots.get(&mount.dev) else {
        // 挂载点对上了、却取不到该设备号的 fs 根：改写只能放弃，表现为「没有任何改写发生」。
        // 这种情况在 CI 与真机之间可能不同（init 侧对同一设备号是否登记了 root=“/” 的挂载），
        // 必须留痕，否则会被误读成「覆盖表没命中挂载点」。
        log::debug!(
            "overlay fs root missing dev={} mount_point={} fs_roots={}",
            mount.dev,
            mount.mount_point,
            snapshot.fs_roots.len()
        );
        return None;
    };
    let rest = &backend_text[mount.mount_point.len()..];
    let source = join_fs_path(fs_root, &mount.root_in_fs, rest);
    if source == backend_text {
        return None;
    }
    log_rewrite_once(&mount.mount_point, &source);
    Some(PathBuf::from(source))
}

/// 改写首次发生（或覆盖点变化）时留一条 debug 痕迹；同一覆盖点只记一次，避免
/// 下载期数千次路径解析刷屏。
fn log_rewrite_once(mount_point: &str, source: &str) {
    let mut guard = LAST_LOGGED_OVERLAY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.as_deref() == Some(mount_point) {
        return;
    }
    log::debug!(
        "fuse backend overlay rewrite mount_point={} source_prefix={}",
        mount_point,
        source
    );
    *guard = Some(mount_point.to_string());
}
