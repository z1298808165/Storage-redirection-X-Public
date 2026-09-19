//! 挂载身份账本的共享内核：账本类型、落盘格式与挂载身份采集。
//!
//! 背景：模块会在同一个挂载点上反复挂载（配置变更、死连接恢复、应用重启），而
//! `/proc/<pid>/mountinfo` 只能证明"该路径上有一条挂载"，无法回答"这条挂载是不是我建的"。
//! 缺少归属判据时，恢复流程只剩两个坏选择：按路径盲摘（可能摘掉其它组件或新会话的挂载）
//! 或直接再挂一层（在死连接上堆叠，越修越坏）。
//!
//! 本模块把归属判据落盘，供挂载清理与恢复流程在动手之前先校验：
//!
//! - `mount_id`：内核分配的挂载 ID。同路径重挂会产生新的 ID，可精确区分新旧会话；
//! - `source`：挂载源（`MountOption::FSName`）。本模块的挂载源带固定前缀；
//! - `ns_dev` / `ns_ino`：目标进程 mount namespace 的 nsfs 身份。命名空间被替换（应用重启）
//!   时挂载会随旧命名空间一起销毁，据此可以判定"无需摘除"而不是"摘除失败"；
//! - `target_start_time`：目标进程 starttime，避免 PID 复用后把新进程的挂载当成自己的；
//! - `generation`：同一命名空间内的挂载代数，单调递增，用于诊断与幂等重挂。
//!
//! 账本与 `MOUNT_STATE_DIR` 下的挂载状态文件职责不同：状态文件回答"要摘哪些路径"，
//! 账本回答"那些路径上的挂载归谁"。
//!
//! **为什么拆成独立文件**：挂载有两条路径——root daemon（`daemon_mount`）与应用进程内的
//! companion（`lifecycle::companion_mount`）。两条路径都要写账本，但 `mount_identity` 原本只在
//! daemon 二进制里编译，于是 companion 路径从来不写账本，走这条路径的应用在恢复流程与 `doctor`
//! 里是盲的。把账本内核放进 lib/bin 共用的模块后，两条路径写的是同一份格式、同一套归属判据。
//! 只与恢复决策有关的部分（判定结果、清理、枚举）留在 bin 侧的 `mount_identity`。

use crate::platform::module_paths;
use crate::platform::mountinfo;
use crate::platform::paths;
use std::fs;
use std::path::PathBuf;

pub use crate::module_mount_source::is_module_redirect_mount;

/// 账本 schema 版本。字段增减时必须同步递增，旧版本账本按不可用处理。
const IDENTITY_SCHEMA_VERSION: u32 = 1;

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
///
/// `root` 是挂载点在**源文件系统内的路径**。bind 挂载会原样继承底层文件系统的挂载源
/// （真机上模块自己的 bind 源是 MediaProvider FUSE 的 `/dev/fuse`），此时 `source` 无法
/// 用于归属判定，只能靠 `root` 里的沙箱路径区分本模块与系统挂载，因此必须一并读出。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMount {
    pub mount_id: u64,
    pub source: String,
    pub fs_type: String,
    pub root: String,
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
    /// 连续摘除失败轮次；达到恢复决策层定义的收敛预算后不再允许注入。
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

    /// 摘除被确认完成后清空失败计数与挂载记录。
    pub fn clear_mounts(&mut self) {
        self.mounts.clear();
        self.detach_attempts = 0;
    }
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
/// 直接读 `/proc/<pid>/mountinfo` 即可获得目标命名空间的视图，不需要切换调用方自身的
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
                root: mountinfo::unescape_field(entry.root),
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

/// 账本文件所在目录。
///
/// 对 `mount_identity`（bin 侧）可见，因此是 `pub` 而不是 `pub(crate)`：bin 侧用
/// `pub use` 再导出本模块的条目时，`pub(crate)` 会报 E0364。
pub fn ledger_dir() -> PathBuf {
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
pub fn encode(ledger: &MountLedger) -> String {
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
pub fn decode(content: &str) -> Option<MountLedger> {
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

/// 构造一条当前时刻的挂载身份记录。
///
/// 两条挂载路径（daemon 与 companion）共用它：`pid <= 0` 读调用方自身命名空间，而两条路径
/// 在登记时都已经处于目标应用的命名空间里，因此归属判据一致。
///
/// 归属判据用 [`is_module_redirect_mount`]（挂载源 **或** `root` 沙箱路径）而不是只看挂载源：
/// bind 会继承底层文件系统的源，真机上本模块每一层的 source 都是 `/dev/fuse`，只按源判断会
/// 让账本**永远登记不到任何挂载明细**，恢复流程因此拿不到「哪一层是我的」，只能走按命名空间
/// 身份的兜底判定，永远产不出 `Owned`。
pub fn capture_mount_identity(
    pid: i32,
    mount_point: &str,
    package_name: &str,
) -> Option<MountIdentity> {
    let live = topmost_live_mount(pid, mount_point)?;
    if !is_module_redirect_mount(&live.source, &live.root, mount_point, package_name) {
        // 最顶层不是本模块的挂载时不能登记身份，否则恢复流程会误以为可以摘除。
        return None;
    }
    Some(MountIdentity {
        mount_point: paths::normalize(mount_point),
        mount_id: live.mount_id,
        source: live.source,
    })
}

/// 按本次挂载的目标集合登记账本。
///
/// 两条挂载路径共用同一套登记纪律，避免"哪条路径挂了账本"成为口径差异：
///
/// - 必须在挂载真正生效之后调用，且调用方已经处于目标命名空间（`pid` 传 0 读自身）；
/// - 读不到任何归属明确的挂载时**不写旧记录**，只保留进程与命名空间身份：宁可让账本暂时
///   没有挂载明细（后续摘除只受挂载源约束），也不要留下与实际挂载不匹配的 `mount_id`——
///   那会让下一轮恢复把本模块自己的挂载误判成"已被新会话接管"而拒绝清理。
///
/// `log_tag` 标识调用方（`daemon` / `companion`），保留在日志里便于区分是哪条路径写的。
/// 返回 false 表示账本落盘失败（调用方应记告警，但不影响本次挂载已生效的事实）。
pub fn record_mount_identity(
    log_tag: &str,
    package_name: &str,
    target_pid: i32,
    targets: &[String],
) -> bool {
    let Some(namespace) = namespace_identity(0) else {
        log::warn!(
            "mount identity namespace unavailable path={} pid={}",
            log_tag,
            target_pid
        );
        return false;
    };
    let target_start_time =
        crate::platform::process_start_time_ticks(target_pid).unwrap_or_default();
    let mut mounts = Vec::new();
    for target in module_paths::normalize_mount_targets(targets) {
        if let Some(identity) = capture_mount_identity(0, &target, package_name) {
            mounts.push(identity);
        }
    }

    let mut ledger = load(package_name, target_pid).unwrap_or_else(|| {
        MountLedger::new(package_name, target_pid, target_start_time, namespace)
    });
    ledger.target_start_time = target_start_time;
    ledger.namespace = namespace;
    if mounts.is_empty() && !targets.is_empty() {
        log::warn!(
            "mount identity no owned mount recorded path={} pid={} pkg={} targets={}",
            log_tag,
            target_pid,
            package_name,
            targets.len()
        );
        ledger.clear_mounts();
    } else {
        ledger.record_mounts(mounts);
    }
    let saved = save(&ledger);
    if saved {
        log::info!(
            "mount identity saved path={} pid={} pkg={} generation={} mounts={} ns={}:{}",
            log_tag,
            target_pid,
            package_name,
            ledger.generation,
            ledger.mounts.len(),
            ledger.namespace.dev,
            ledger.namespace.ino
        );
    }
    saved
}
