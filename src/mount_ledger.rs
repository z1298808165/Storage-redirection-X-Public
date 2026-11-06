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

use crate::fuse_redirect::config::NATIVE_FUSE_SOURCE;
use crate::platform::module_paths;
use crate::platform::mountinfo;
use crate::platform::paths;
use std::fs;
use std::path::PathBuf;

// quality-allow(lint-suppression): 消费方 daemon_mount 仅由 bin #[path] 再编译，
// lib 视角本 re-export 未使用（两侧编译差异）。
#[allow(unused_imports)]
pub use crate::module_mount_source::is_module_redirect_mount;

/// 账本 schema 版本。字段增减时必须同步递增，旧版本账本按不可用处理。
///
/// v2 → v3：`LiveMount`/`MountIdentity` 增加 `dev`（超级块设备号）。挂载源原生化之后
/// 源不再带模块标识，"这一层是不是本模块的"只能靠超级块身份 + 挂载 ID，账本必须落盘
/// `dev` 才能在同一次开机内反查归属。旧版本账本按不可用处理，由下一次成功挂载重新登记。
///
/// v1 → v2：归属判据由"别名归一化路径"改为"保留存储别名的语法路径"。旧账本把
/// `/data/media/<user>/...` 归一化成 `/storage/emulated/<user>/...` 存储，与真实主入口
/// 目标重合，会被新逻辑误判为同一挂载点的归属身份、进而误用于摘除后端挂载。为保守起见，
/// 直接让 v1 账本 decode 失败（按不可用处理），由下一次成功挂载用新口径重新登记。
const IDENTITY_SCHEMA_VERSION: u32 = 3;

/// 持久 dev 历史最多保留的代数。只需覆盖"刚换代的旧层还挂在活应用命名空间里"这一段
/// 窗口，几代足够；无限增长会让每次归属判定都多读一堆早已消失的 dev。
const KNOWN_DEVS_LIMIT: usize = 64;

/// 目标进程 mount namespace 的身份。
///
/// nsfs 的 `st_dev` + `st_ino` 在同一台设备上唯一标识一个 mount namespace 实例；
/// 应用重启后 `setns` 目标会换成新的 inode，据此可以区分"同一个命名空间里的挂载残留"
/// 与"旧命名空间已经随进程销毁"。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    /// 父挂载 ID：同挂载点叠放时即下面一层的挂载 ID，用于栈顶判定。
    pub parent_mount_id: u64,
    /// 超级块设备号（`major:minor`）。同一宿主/会话的所有层共享它（bind 继承），
    /// 系统 MediaProvider 的 FUSE 是另一个值——挂载源原生化之后，它是唯一能区分
    /// "我们的层"与"系统的层"的内核事实。
    pub dev: String,
    pub source: String,
    pub fs_type: String,
    pub root: String,
}

/// 落盘的挂载身份。
#[derive(Clone, Debug)]
pub struct MountIdentity {
    pub mount_point: String,
    pub mount_id: u64,
    /// 超级块设备号：与 `mount_id` 一起构成"这一层是谁的"的完整身份。
    pub dev: String,
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

/// 读取指定命名空间内挂载点上的全部挂载记录，保持 mountinfo 原始文件序。
///
/// 直接读 `/proc/<pid>/mountinfo` 即可获得目标命名空间的视图，不需要切换调用方自身的
/// 命名空间，也不会修改任何目录元数据。`pid <= 0` 表示读取调用方自身所在的命名空间，
/// 用于挂载子进程在 `setns` 之后登记自己刚建立的挂载。
///
/// 顺序契约：必须保持文件序——栈顶兜底判定（[`mountinfo::topmost_by_stack`] 的并列
/// 形态）与既有 /proc/mounts 约定都依赖「后挂的层在文件里靠后」；按挂载 ID 排序会
/// 破坏该约定（detached tree 搬运的层 ID 与挂载顺序倒挂，真机 2026-10-15）。
pub fn live_mounts_at(pid: i32, mount_point: &str) -> Vec<LiveMount> {
    let path = if pid > 0 {
        format!("/proc/{pid}/mountinfo")
    } else {
        "/proc/self/mountinfo".to_string()
    };
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    // 语法层匹配：保留存储别名，不把 /data/media 折叠成 /storage/emulated，
    // 且大小写敏感。否则不同挂载别名会被归一化到同一字符串，使缺失的主入口
    // 误匹配到后端的 /data/media 挂载。见模块级文档中的场景 29 说明。
    let normalized = paths::normalize_syntax(mount_point);
    content
        .lines()
        .filter_map(|line| {
            let entry = mountinfo::parse_entry(line)?;
            if paths::normalize_syntax(&mountinfo::unescape_field(entry.target)) != normalized {
                return None;
            }
            Some(LiveMount {
                mount_id: entry.mount_id,
                parent_mount_id: entry.parent_mount_id,
                dev: entry.dev.to_string(),
                source: mountinfo::unescape_field(entry.source),
                fs_type: entry.fs_type.to_string(),
                root: mountinfo::unescape_field(entry.root),
            })
        })
        .collect()
}

/// 挂载点上最顶层的一条记录（按父挂载链判定叠放顺序）。
///
/// 挂载 ID 在 detached tree + `move_mount` 搬运形态下不随叠放顺序递增，按最大 ID
/// 选取会把栈底层（如屏障 tmpfs）当成栈顶。叠层顺序只能按 `parent_mount_id`
/// 父链判定，详见 [`mountinfo::topmost_by_stack`]。
pub fn topmost_live_mount(pid: i32, mount_point: &str) -> Option<LiveMount> {
    let mounts = live_mounts_at(pid, mount_point);
    mountinfo::topmost_by_stack(
        &mounts,
        |mount: &LiveMount| mount.mount_id,
        |mount: &LiveMount| mount.parent_mount_id,
    )
    .cloned()
}

/// 收集本模块已知的 FUSE 超级块设备号（账本里登记过的 + 持久历史 + 当前宿主会话）。
///
/// 挂载源原生化之后，挂载表里不再有任何模块标识，"这一层是不是我们的"只能靠超级块身份。
/// 判断依据是**我们自己登记过的东西**：每一个成功挂载轮次都会把 `dev` 写进账本
/// （见 [`MountIdentity::dev`]），主机宿主会话的 `dev` 则在快照里。系统 MediaProvider 的
/// FUSE 超级块不在其中，因此这个集合天然把"我们的层"和"系统的层"分开。
///
/// 也包含**已失效会话**的 dev：应用进程还活着、宿主换代时，旧层仍在它的命名空间里，
/// 清理流程必须先能认出它才能摘掉（认不出就会一直 ENOTCONN）。
///
/// 因此除了活账本，还要并上 [`remember_module_fuse_devs`] 落盘的持久历史：账本是按
/// 命名空间覆盖写的，宿主换代后同一批 pid 的新一轮挂载会用**新 dev** 重写账本，
/// 旧 dev 只剩挂载表里的那一层还带着——只读账本必然认不出它（2026-11-02 真机：
/// 宿主换代后每轮都 `unmount skipped foreign ... dev=<旧会话> ledger_devs=0`
/// → `cleanup incomplete` → 死层永远叠在栈底）。
pub fn known_module_fuse_devs() -> std::collections::HashSet<String> {
    let mut devs = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(ledger_dir()) {
        for entry in entries.flatten() {
            let Ok(content) = fs::read_to_string(entry.path()) else {
                continue;
            };
            // 复用 decode 而不是再写一遍文本解析：账本格式只在一处定义，两处各写一份
            // 必然在字段顺序调整时漂移。schema 不匹配（旧版本账本）直接跳过。
            let Some(ledger) = decode(&content) else {
                continue;
            };
            for mount in ledger.mounts {
                // 只收 FUSE 超级块：`alias bind` 这类层继承的是后端真实文件系统（`/data`
                // 的 f2fs）超级块，收进来会让同盘的任何系统 bind 挂载都被判成本模块所有。
                if !mount.dev.is_empty() && mount.source == NATIVE_FUSE_SOURCE {
                    devs.insert(mount.dev);
                }
            }
        }
    }
    if let Ok(content) = fs::read_to_string(known_devs_path()) {
        for line in content.lines() {
            let dev = line.trim();
            if !dev.is_empty() {
                devs.insert(dev.to_string());
            }
        }
    }
    if let Some(dev) = crate::fuse_host::current_host_dev() {
        devs.insert(dev);
    }
    devs
}

/// 持久记录"这一代会话的超级块属于本模块"，跨进程/跨宿主换代保留。
///
/// 只写 `major:minor` 文本，不含任何路径或进程信息；同一 dev 幂等，超出
/// [`KNOWN_DEVS_LIMIT`] 时按先进先出淘汰最老的记录——保护的是"刚换代、旧层还挂在
/// 活着的应用命名空间里"这个窗口，不需要无限历史。
pub fn remember_module_fuse_devs<'a>(devs: impl IntoIterator<Item = &'a str>) {
    let mut lines: Vec<String> = fs::read_to_string(known_devs_path())
        .map(|content| {
            content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut changed = false;
    for dev in devs {
        if dev.is_empty() || lines.iter().any(|line| line == dev) {
            continue;
        }
        lines.push(dev.to_string());
        changed = true;
    }
    if !changed {
        return;
    }
    if lines.len() > KNOWN_DEVS_LIMIT {
        let drop = lines.len() - KNOWN_DEVS_LIMIT;
        lines.drain(0..drop);
    }
    if fs::create_dir_all(module_paths::MOUNT_STATE_DIR).is_err() {
        return;
    }
    let path = known_devs_path();
    let mut content = lines.join("\n");
    content.push('\n');
    let temp = format!("{}.tmp", path.display());
    if fs::write(&temp, content.as_bytes()).is_ok() && fs::rename(&temp, &path).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

/// 持久 dev 历史文件路径（与账本目录同级，避免被 [`known_module_fuse_devs`] 当账本解析）。
fn known_devs_path() -> PathBuf {
    PathBuf::from(module_paths::MOUNT_STATE_DIR).join("known_devs")
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
    // 字段顺序必须与 decode 严格一致（id、dev、source、挂载点）。少写 dev 会让 decode
    // 把 source 当 dev、把挂载点当 source，最后因缺字段整行被丢弃——账本看起来"存在但
    // 没有挂载记录"，归属判定与 detach 记账会同时失效（2026-11-02 真机：重挂报
    // completed 但运行中应用一层未换）。
    for mount in &ledger.mounts {
        content.push_str(&format!(
            "mount={}\t{}\t{}\t{}\n",
            mount.mount_id, mount.dev, mount.source, mount.mount_point
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
            let dev = parts.next()?.to_string();
            let source = parts.next()?;
            let mount_point = parts.next()?;
            if !module_paths::is_safe_mount_target(mount_point) {
                return None;
            }
            Some(MountIdentity {
                mount_point: mount_point.to_string(),
                mount_id,
                dev,
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

/// 删除挂载账本；文件已不存在也视为成功。
pub fn remove(package_name: &str, pid: i32) -> bool {
    let path = ledger_path(package_name, pid);
    fs::remove_file(&path).is_ok() || fs::metadata(&path).is_err()
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
    // 账本会被同名命名空间的下一轮挂载整体覆盖，落盘的同时把这批 dev 写进持久历史，
    // 宿主换代后旧层才还能被认成本模块的。
    // 只登记**FUSE 超级块**的 dev：私有目录 alias bind 之类的层继承的是后端真实文件系统
    // 的超级块（`/data` 的 f2fs），把它当"本模块的层"会让任何同盘 bind 挂载（系统自己的
    // `/data/media` 视图等）都被判成本模块所有，从而被误摘。挂载源原生化之后，我们的层
    // 一律是 `/dev/fuse`，这一条就是"只认自己的超级块"的最窄判据。
    remember_module_fuse_devs(
        ledger
            .mounts
            .iter()
            .filter(|mount| mount.source == NATIVE_FUSE_SOURCE)
            .map(|mount| mount.dev.as_str()),
    );
    true
}

/// 按调用方给定的显式挂载目标登记身份：不经归属谓词，直接取该目标的 topmost 事实。
///
/// 调用方刚在 `mount_point` 上完成挂载（或确认沿用既有挂载），target 上的 topmost 挂载
/// 就是本模块的那一层。**不能**改回 [`is_module_redirect_mount`] 归属谓词判定：
/// private-dir alias bind 的 `root` 形态与系统挂载完全同形（都止于
/// `Android/{data,media,obb}/<pkg>`），谓词会把它当系统挂载漏掉，账本缺了这层，
/// cleanup 判 foreign 就永远摘不净，重挂循环不停（真机实测 wetype 等应用每次
/// reload 都触发 cleanup incomplete）。
pub fn capture_explicit_mount_identity(pid: i32, mount_point: &str) -> Option<MountIdentity> {
    let live = topmost_live_mount(pid, mount_point)?;
    Some(MountIdentity {
        mount_point: paths::normalize_syntax(mount_point),
        mount_id: live.mount_id,
        dev: live.dev,
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
///
/// 不返回结果：账本落盘失败**不影响本次挂载已生效的事实**，调用方也不得据此改变挂载结论；
/// 但失败会让后续恢复与 `doctor` 退化成「按挂载源判定」的弱归属，所以必须留痕。告警收敛
/// 在本函数内（带 `path` 标签），两条挂载路径不可能再各自漏掉一份——此前 `daemon` 侧丢弃
/// 返回值静默失败、`companion` 侧另写一条告警，同一件事两种口径。
pub fn record_mount_identity(
    log_tag: &str,
    package_name: &str,
    target_pid: i32,
    targets: &[String],
) {
    let Some(namespace) = namespace_identity(0) else {
        log::warn!(
            "mount identity namespace unavailable path={} pid={}",
            log_tag,
            target_pid
        );
        return;
    };
    let target_start_time =
        crate::platform::process_start_time_ticks(target_pid).unwrap_or_default();
    let mut mounts = Vec::new();
    for target in module_paths::normalize_mount_targets(targets) {
        // 显式目标是调用方本次挂载创建（或确认沿用）的路径，直接按 topmost 事实登记，
        // 不走归属谓词（见 capture_explicit_mount_identity 的文档）。
        if let Some(identity) = capture_explicit_mount_identity(0, &target) {
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
    if !save(&ledger) {
        // 落盘失败的单一口径：调用方无法再漏报，也不必分别处理。
        log::warn!(
            "mount identity save failed path={} pid={} pkg={} mounts={} targets={}",
            log_tag,
            target_pid,
            package_name,
            ledger.mounts.len(),
            targets.len()
        );
        return;
    }
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
