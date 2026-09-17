//! 挂载身份账本：显式记录挂载 ID、挂载源、mount namespace 身份与目标进程 starttime。
//!
//! 背景：daemon 会在同一个挂载点上反复挂载（配置变更、死连接恢复、应用重启），而
//! `/proc/<pid>/mountinfo` 只能证明"该路径上有一条挂载"，无法回答"这条挂载是不是我建的"。
//! 缺少归属判据时，恢复流程只剩两个坏选择：按路径盲摘（可能摘掉其它组件或新会话的挂载）
//! 或直接再挂一层（在死连接上堆叠，越修越坏）。
//!
//! 本模块把归属判据落盘，供挂载清理与恢复流程在动手之前先校验：
//!
//! - `mount_id`：内核分配的挂载 ID。同路径重挂会产生新的 ID，可精确区分新旧会话；
//! - `source`：挂载源（`MountOption::FSName`）。本模块的挂载源带固定前缀，见
//!   [`is_module_mount_source`]；
//! - `ns_dev` / `ns_ino`：目标进程 mount namespace 的 nsfs 身份。命名空间被替换（应用重启）
//!   时挂载会随旧命名空间一起销毁，据此可以判定"无需摘除"而不是"摘除失败"；
//! - `target_start_time`：目标进程 starttime，避免 PID 复用后把新进程的挂载当成自己的；
//! - `generation`：同一命名空间内的挂载代数，单调递增，用于诊断与幂等重挂。
//!
//! 账本与 `MOUNT_STATE_DIR` 下的挂载状态文件职责不同：状态文件回答"要摘哪些路径"，
//! 账本回答"那些路径上的挂载归谁"。因此账本在挂载成功后写入，在确认摘除完成后删除，
//! 摘除未验证通过时保留并累计失败轮次，作为拒绝叠加新挂载的依据。

use crate::platform::module_paths;
use crate::platform::mountinfo;
use crate::platform::paths;
use std::fs;
use std::path::PathBuf;

/// 本模块挂载源使用的前缀。
///
/// `srx_fuse_redirect[<pid>]` 是按应用启动的 scoped FUSE 会话；`srx_fuse_host[<pid>]` 是
/// 共享 FUSE daemon 的宿主会话。两种来源都由本模块创建，恢复流程可以放心摘除；
/// 其它前缀（例如系统媒体 FUSE 的 `/dev/fuse`）一律视为外部挂载。
const MODULE_MOUNT_SOURCE_PREFIXES: [&str; 2] = ["srx_fuse_redirect", "srx_fuse_host"];

/// 账本 schema 版本。字段增减时必须同步递增，旧版本账本按不可用处理。
const IDENTITY_SCHEMA_VERSION: u32 = 1;

/// 同一命名空间连续摘除失败的收敛预算。
///
/// 达到预算后账本进入 poisoned 状态：恢复流程停止重新注入，只保留告警，直到摘除被确认
/// 完成。这样做的目的是把"一直挂不上"暴露成一个可观测的持续状态，而不是每轮都叠加一层
/// 新的挂载，让应用最终面对一个多层 ENOTCONN 的死挂载栈。
pub const MAX_DETACH_ATTEMPTS: u32 = 3;

/// 目标进程 mount namespace 的身份。
///
/// nsfs 的 `st_dev` + `st_ino` 在同一台设备上唯一标识一个 mount namespace 实例；
/// 应用重启后 `setns` 目标会换成新的 inode，据此可以区分"同一个命名空间里的挂载残留"
/// 与"旧命名空间已经随进程销毁"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamespaceIdentity {
    pub dev: u64,
    pub ino: u64,
}

/// 一条挂载记录在挂载表中的实时状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMount {
    pub mount_id: u64,
    pub source: String,
    pub fs_type: String,
}

/// 落盘的挂载身份。
#[derive(Clone, Debug)]
pub struct MountIdentity {
    pub mount_point: String,
    pub mount_id: u64,
    pub source: String,
}

/// 一个命名空间的挂载身份账本。
#[derive(Clone, Debug)]
pub struct MountLedger {
    pub package_name: String,
    pub target_pid: i32,
    pub target_start_time: u64,
    pub namespace: NamespaceIdentity,
    /// 同一命名空间内的挂载代数，每成功记录一轮挂载递增。
    pub generation: u64,
    pub mounts: Vec<MountIdentity>,
    /// 连续摘除失败轮次；达到 [`MAX_DETACH_ATTEMPTS`] 后不再允许注入。
    pub detach_attempts: u32,
}

impl MountLedger {
    pub fn new(
        package_name: &str,
        target_pid: i32,
        target_start_time: u64,
        namespace: NamespaceIdentity,
    ) -> Self {
        Self {
            package_name: package_name.to_string(),
            target_pid,
            target_start_time,
            namespace,
            generation: 0,
            mounts: Vec::new(),
            detach_attempts: 0,
        }
    }

    /// 记录一轮挂载结果并递增代数。
    pub fn record_mounts(&mut self, mounts: Vec<MountIdentity>) {
        self.generation = self.generation.saturating_add(1);
        self.mounts = mounts;
        self.detach_attempts = 0;
    }

    /// 累计一次摘除失败；返回是否已经达到收敛预算。
    pub fn record_detach_failure(&mut self) -> bool {
        self.detach_attempts = self.detach_attempts.saturating_add(1);
        self.is_poisoned()
    }

    /// 摘除被确认完成后清空失败计数与挂载记录。
    pub fn clear_mounts(&mut self) {
        self.mounts.clear();
        self.detach_attempts = 0;
    }

    /// 是否已收敛到 poisoned：继续注入只会在死连接上叠加新的挂载层。
    pub fn is_poisoned(&self) -> bool {
        self.detach_attempts >= MAX_DETACH_ATTEMPTS
    }

    /// 目标进程是否仍是账本记录的那一个实例。
    pub fn target_is_current(&self) -> bool {
        crate::platform::is_process_instance_alive(self.target_pid, self.target_start_time)
    }
}

/// 挂载归属判定结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountVerdict {
    /// 记录与实时状态一致：必须先摘除这一层，再重新注入。
    Owned(LiveMount),
    /// 目标路径上已没有挂载：无需摘除，可以直接重新注入。
    Detached,
    /// 同路径被其它身份接管（新会话或其它组件）：不得摘除，交由接管者负责。
    Superseded(LiveMount),
    /// 目标进程或命名空间已经失效：挂载随命名空间销毁，记录应清理。
    StaleNamespace,
}

/// 读取 mount namespace 的 nsfs 身份。
///
/// 进程退出后 `/proc/<pid>/ns/mnt` 会消失，返回 None 表示无法确认命名空间身份，
/// 调用方应把它当作"目标不可用"而不是"身份未变化"。`pid <= 0` 表示调用方自身所在的
/// 命名空间，用于挂载子进程在 `setns` 之后登记目标命名空间的身份。
pub fn namespace_identity(pid: i32) -> Option<NamespaceIdentity> {
    let path = if pid > 0 {
        format!("/proc/{pid}/ns/mnt")
    } else {
        "/proc/self/ns/mnt".to_string()
    };
    let Ok(c_path) = std::ffi::CString::new(path) else {
        return None;
    };
    // SAFETY: c_path 是以 NUL 结尾的合法路径且在本作用域内保持存活；buf 是栈上有效的
    // stat 结构，stat 成功时会完整写入。
    let mut buf = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe { libc::stat(c_path.as_ptr(), buf.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    // SAFETY: 上一步返回 0 表示内核已写入完整的 stat 结构。
    let stat = unsafe { buf.assume_init() };
    Some(NamespaceIdentity {
        dev: stat.st_dev,
        ino: stat.st_ino,
    })
}

/// 读取指定命名空间内挂载点上的全部挂载记录，按挂载 ID 升序返回。
///
/// 直接读 `/proc/<pid>/mountinfo` 即可获得目标命名空间的视图，不需要切换 daemon 自身的
/// 命名空间，也不会修改任何目录元数据。`pid <= 0` 表示读取调用方自身所在的命名空间，
/// 用于挂载子进程在 `setns` 之后登记自己刚建立的挂载。
pub fn live_mounts_at(pid: i32, mount_point: &str) -> Vec<LiveMount> {
    let path = if pid > 0 {
        format!("/proc/{pid}/mountinfo")
    } else {
        "/proc/self/mountinfo".to_string()
    };
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let normalized = paths::normalize(mount_point);
    let mut mounts = content
        .lines()
        .filter_map(|line| {
            let entry = mountinfo::parse_entry(line)?;
            if !paths::eq_ignore_case(
                &paths::normalize(&mountinfo::unescape_field(entry.target)),
                &normalized,
            ) {
                return None;
            }
            Some(LiveMount {
                mount_id: entry.mount_id,
                source: mountinfo::unescape_field(entry.source),
                fs_type: entry.fs_type.to_string(),
            })
        })
        .collect::<Vec<_>>();
    mounts.sort_by_key(|mount| mount.mount_id);
    mounts
}

/// 挂载点上最顶层的一条记录（挂载 ID 最大）。
pub fn topmost_live_mount(pid: i32, mount_point: &str) -> Option<LiveMount> {
    live_mounts_at(pid, mount_point)
        .into_iter()
        .max_by_key(|mount| mount.mount_id)
}

/// 判断挂载源是否由本模块创建。
///
/// 只按固定前缀判断，不按文件系统类型：内核 `mount(2)` 直挂的 scoped 挂载类型是 `fuse`，
/// 只有经 fusermount 回退时才出现 `fuse.srx`，两者都是本模块的挂载。
pub fn is_module_mount_source(source: &str) -> bool {
    MODULE_MOUNT_SOURCE_PREFIXES
        .iter()
        .any(|prefix| source.starts_with(prefix))
}

/// 按挂载身份判定当前应该执行哪种恢复动作。
///
/// 判定顺序体现"先证明归属，再动手"的原则：
///
/// 1. 目标进程已不是记录的那个实例（退出或被 PID 复用）→ [`MountVerdict::StaleNamespace`]，
///    挂载随旧命名空间销毁，账本应清理，不能对复用后的 PID 摘挂载；
/// 2. 命名空间 inode 变化 → 同样视为旧命名空间已销毁；
/// 3. 账本里没有该挂载点的记录，或目标路径上没有挂载 → [`MountVerdict::Detached`]，
///    直接重新注入；
/// 4. 最顶层挂载的 ID 与挂载源都匹配记录 → [`MountVerdict::Owned`]，摘除它再重新注入；
/// 5. 其余情况（同路径上有别的挂载，或本模块的新会话已经接管）→
///    [`MountVerdict::Superseded`]，不摘除。
pub fn classify_mount(
    ledger: &MountLedger,
    mount_point: &str,
    live: Option<&LiveMount>,
    current_namespace: Option<NamespaceIdentity>,
    target_is_current: bool,
) -> MountVerdict {
    if !target_is_current {
        return MountVerdict::StaleNamespace;
    }
    let Some(current_namespace) = current_namespace else {
        return MountVerdict::StaleNamespace;
    };
    if current_namespace != ledger.namespace {
        return MountVerdict::StaleNamespace;
    }
    let normalized = paths::normalize(mount_point);
    let recorded = ledger
        .mounts
        .iter()
        .find(|mount| paths::eq_ignore_case(&mount.mount_point, &normalized));
    let (Some(recorded), Some(live)) = (recorded, live) else {
        return MountVerdict::Detached;
    };
    if live.mount_id == recorded.mount_id && live.source == recorded.source {
        return MountVerdict::Owned(live.clone());
    }
    MountVerdict::Superseded(live.clone())
}

/// 账本文件所在目录。
fn ledger_dir() -> PathBuf {
    PathBuf::from(module_paths::MOUNT_STATE_DIR).join("identity")
}

/// 账本文件路径，按包名与 pid 区分命名空间。
pub fn ledger_path(package_name: &str, pid: i32) -> String {
    ledger_dir()
        .join(format!(
            "{}_{}.identity",
            module_paths::sanitize_name(package_name),
            pid
        ))
        .to_string_lossy()
        .into_owned()
}

/// 序列化账本内容。
///
/// `mount` 行以制表符分隔字段、挂载点放最后，读取时再按 [`module_paths::is_safe_mount_target`]
/// 过滤，避免账本被外部写入后在无关目录上执行卸载。
fn encode(ledger: &MountLedger) -> String {
    let mut content = String::new();
    content.push_str(&format!("schema={}\n", IDENTITY_SCHEMA_VERSION));
    content.push_str(&format!("package={}\n", ledger.package_name));
    content.push_str(&format!("target_pid={}\n", ledger.target_pid));
    content.push_str(&format!("target_start_time={}\n", ledger.target_start_time));
    content.push_str(&format!(
        "namespace={}:{}\n",
        ledger.namespace.dev, ledger.namespace.ino
    ));
    content.push_str(&format!("generation={}\n", ledger.generation));
    content.push_str(&format!("detach_attempts={}\n", ledger.detach_attempts));
    for mount in &ledger.mounts {
        content.push_str(&format!(
            "mount={}\t{}\t{}\n",
            mount.mount_id, mount.source, mount.mount_point
        ));
    }
    content
}

/// 从文本解析账本；schema 不匹配或关键字段缺失时返回 None。
fn decode(content: &str) -> Option<MountLedger> {
    let schema = field(content, "schema")?.parse::<u32>().ok()?;
    if schema != IDENTITY_SCHEMA_VERSION {
        return None;
    }
    let package_name = field(content, "package")?.to_string();
    let target_pid = field(content, "target_pid")?.parse::<i32>().ok()?;
    let target_start_time = field(content, "target_start_time")?.parse::<u64>().ok()?;
    let namespace_raw = field(content, "namespace")?;
    let (dev, ino) = namespace_raw.split_once(':')?;
    let namespace = NamespaceIdentity {
        dev: dev.parse().ok()?,
        ino: ino.parse().ok()?,
    };
    let generation = field(content, "generation")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let detach_attempts = field(content, "detach_attempts")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mounts = content
        .lines()
        .filter_map(|line| line.strip_prefix("mount="))
        .filter_map(|value| {
            let mut parts = value.split('\t');
            let mount_id = parts.next()?.parse::<u64>().ok()?;
            let source = parts.next()?;
            let mount_point = parts.next()?;
            if !module_paths::is_safe_mount_target(mount_point) {
                return None;
            }
            Some(MountIdentity {
                mount_point: mount_point.to_string(),
                mount_id,
                source: source.to_string(),
            })
        })
        .collect();
    Some(MountLedger {
        package_name,
        target_pid,
        target_start_time,
        namespace,
        generation,
        mounts,
        detach_attempts,
    })
}

fn field<'a>(content: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    content
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}

/// 读取账本；文件不存在、schema 不匹配或解析失败时返回 None。
pub fn load(package_name: &str, pid: i32) -> Option<MountLedger> {
    let content = fs::read_to_string(ledger_path(package_name, pid)).ok()?;
    decode(&content)
}

/// 原子写入账本。
///
/// 与挂载状态文件采用同一套写入纪律：先写临时文件并 `fsync`，再 `rename` 覆盖。
/// 这样崩溃或断电只会留下临时文件，正式账本始终是上一轮的完整内容，不会因为读到半截
/// 文件而丢失归属判据。
pub fn save(ledger: &MountLedger) -> bool {
    let dir = ledger_dir();
    if fs::create_dir_all(&dir).is_err() {
        log::warn!("mount identity mkdir failed dir={}", dir.display());
        return false;
    }
    let path = ledger_path(&ledger.package_name, ledger.target_pid);
    let temp_path = format!("{path}.tmp");
    let content = encode(ledger);
    if fs::write(&temp_path, content.as_bytes()).is_err() {
        log::warn!("mount identity write failed path={}", temp_path);
        return false;
    }
    if fs::rename(&temp_path, &path).is_err() {
        let _ = fs::remove_file(&temp_path);
        log::warn!("mount identity rename failed path={}", path);
        return false;
    }
    true
}

/// 删除账本；文件已不存在也视为成功。
pub fn remove(package_name: &str, pid: i32) -> bool {
    let path = ledger_path(package_name, pid);
    fs::remove_file(&path).is_ok() || fs::metadata(&path).is_err()
}

/// 清理目标进程已经退出的账本。
///
/// 判断以 `/proc/<pid>` 的 starttime 为准：PID 被复用后 starttime 一定不同，
/// 据此可以区分"进程真的退出了"与"PID 换了个新进程"。
pub fn prune_stale() -> usize {
    let Ok(entries) = fs::read_dir(ledger_dir()) else {
        return 0;
    };
    let mut removed = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("identity") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(ledger) = decode(&content) else {
            // 解析不了的文件既不能用于恢复也不能用于清理，直接移除避免长期累积。
            if fs::remove_file(&path).is_ok() {
                removed = removed.saturating_add(1);
            }
            continue;
        };
        if !ledger.target_is_current() && fs::remove_file(&path).is_ok() {
            removed = removed.saturating_add(1);
        }
    }
    if removed > 0 {
        log::info!("mount identity pruned stale count={}", removed);
    }
    removed
}

/// 枚举磁盘上现有的全部账本，供诊断输出使用。
///
/// 解析失败的文件不计入结果：诊断不能因为一个损坏文件而整体失败。
pub fn list_ledgers() -> Vec<MountLedger> {
    let Ok(entries) = fs::read_dir(ledger_dir()) else {
        return Vec::new();
    };
    let mut ledgers = entries
        .flatten()
        .filter(|entry| {
            entry.path().extension().and_then(|value| value.to_str()) == Some("identity")
        })
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter_map(|content| decode(&content))
        .collect::<Vec<_>>();
    ledgers.sort_by(|left, right| {
        left.package_name
            .cmp(&right.package_name)
            .then_with(|| left.target_pid.cmp(&right.target_pid))
    });
    ledgers
}

/// 构造一条当前时刻的挂载身份记录。
pub fn capture_mount_identity(pid: i32, mount_point: &str) -> Option<MountIdentity> {
    let live = topmost_live_mount(pid, mount_point)?;
    if !is_module_mount_source(&live.source) {
        // 最顶层不是本模块的挂载时不能登记身份，否则恢复流程会误以为可以摘除。
        return None;
    }
    Some(MountIdentity {
        mount_point: paths::normalize(mount_point),
        mount_id: live.mount_id,
        source: live.source,
    })
}
