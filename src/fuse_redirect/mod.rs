mod attrs;
mod backend_overlay;
mod callbacks;
pub(crate) mod config;
mod helpers;
mod inode;
mod media_view;
mod metadata;
mod perf;
mod policy;
mod rules;
pub(crate) mod scoped_mount;
pub(crate) mod slow_probe;
pub(super) use rules::normalize_rule_list;

pub(crate) use policy::SharedPolicyTable;

// 公开这些配置类型供 daemon/测试流复用；部分构建目标只使用其中的函数。
pub use config::{
    FuseRedirectConfig, MountRequestFields, request_policy_fingerprint,
    scoped_fuse_mount_roots_for_request,
};
pub use scoped_mount::{
    ScopedMountAttempt, ScopedMountReport, conclude_scoped_mount, log_scoped_mount_roots,
};

use crate::platform::{fs, paths};
use attrs::{file_attr_from_metadata, synthetic_dir_attr};
use fuser::{
    Errno, FileAttr, FileType, FopenFlags, Generation, INodeNo, ReplyEmpty, ReplyEntry, Request,
};
use inode::{remove_inode_path, remove_unreferenced_inode};
use metadata::{
    apply_create_metadata, create_metadata, cstring_path, errno_from_code, errno_from_io,
    last_errno,
};
use perf::FusePerfStats;
use policy::{
    ANDROID_APP_UID_START, BackendPath, OperationKind, PolicyRegistry, RedirectPolicy,
    RequesterIdentity,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use once_cell::sync::Lazy;

/// attr 有效期：与原生 MP FuseDaemon 对齐（10s）。writeback 回写模式下内核在
/// 本地维护 inode 的 i_size，应用轮询 stat 命中未过期的内核缓存时拿到的是
/// 真实尺寸；TTL 过短（旧值 250ms）会让每次轮询穿透到后端 lstat，读到回刷
/// 滞后的旧尺寸——QQ 相机录制即因此读到 size=0/截断值而判「录制异常」。
const ATTR_TTL: Duration = Duration::from_secs(10);
/// entry 有效期：与原生一致取 0（不缓存查找结果，负查询亦不缓存）。
const ENTRY_TTL: Duration = Duration::ZERO;
const ROOT_INO: u64 = 1;
const MAX_READ_SIZE: usize = 256 * 1024;

/// 系统 uid 请求者身份缓存 TTL：pid 的 /proc 身份极少变化（进程退出即失效，
/// 重用窗口内 cmdline+uid 双校验兜底），5 秒足够吸收 write 等高频路径的重复解析。
const SYSTEM_REQUESTER_CACHE_TTL_MS: i64 = 5_000;
/// 缓存容量上限：系统 uid 进程数量级远小于该值，超限整表清空（简单防膨胀）。
const SYSTEM_REQUESTER_CACHE_CAP: usize = 256;
/// 系统 uid 请求者身份缓存条目：(解析时刻单调毫秒，身份；None 表示 pid 当时
/// 不可归因——缓存失败结果避免 miss 风暴)。
type SystemRequesterCacheEntry = (i64, Option<RequesterIdentity>);
static SYSTEM_REQUESTER_CACHE: Lazy<Mutex<HashMap<i32, SystemRequesterCacheEntry>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
/// 目录候选缓存的存活时间。
///
/// 命中前会逐源校验目录 mtime（`DirectorySourceSignature`），文件增删都会改变父目录
/// mtime 从而立即失效，因此这里的 TTL 只是兜底上限，不承担新鲜度判定。此前取 250ms
/// 时，相册类应用逐个相册目录 opendir 的间隔远大于有效期，几乎每次都重新枚举：
/// 真机实测（A16，1 万张照片 / 20 个相册目录）模块视图的树枚举 198–229ms，原生
/// 36–70ms；单目录 500 条枚举模块 32ms、原生 1ms，会话侧 `avg_dir_scan_us≈26000`。
/// 按目录数线性放大后（上百个相册目录）正好是用户反馈的秒级加载。放宽到 2s 后
/// 同一目录的重复 opendir 走缓存，新增/删除仍由 mtime 校验即时失效。
const DIR_CANDIDATE_CACHE_TTL: Duration = Duration::from_secs(2);
static INODE_POLICY_FALLBACKS: AtomicU64 = AtomicU64::new(0);
/// 会话级 ENOENT 诊断限流：前 4 条带策略指纹落日志，用于定位「未登记 uid / 旧映射残留」。
static OPEN_ENOENT_DIAG_COUNT: AtomicU64 = AtomicU64::new(0);
/// create 入口的 ENOENT 诊断限流（与 open 分开计数）。
static CREATE_ENOENT_DIAG_COUNT: AtomicU64 = AtomicU64::new(0);
const INITIAL_DIR_CANDIDATE_CACHE_ENTRIES: usize = 64;
const MEDIA_RW_UID: u32 = 1023;
pub(super) const MEDIA_RW_GID: u32 = 1023;
// 目录模式只在 `platform::fs` 定义一次，避免多处副本各自漂移。
pub(super) use crate::platform::fs::MAPPED_DIR_MODE;
const SHARED_PUBLIC_DIR_MODE: u32 = crate::platform::fs::SHARED_PUBLIC_DIR_MODE;
const INITIAL_DIR_CANDIDATE_CACHE_BYTES: usize = 256 * 1024;

thread_local! {
    // FUSE 读回调通常在固定工作线程上重复执行，复用 256 KiB 内的缓冲区减少分配。
    static FUSE_READ_BUFFER: RefCell<Vec<u8>> = RefCell::new(Vec::new());
}

struct FuseRedirectFs {
    /// 策略注册表；每个回调按调用方 uid 取一次，未命中时回退到会话默认。
    policy: PolicyRegistry,
    /// FUSE 请求由多个内核线程并发派发，读多写少：
    /// 使用读写锁让 read/readdir/fsync 等只读路径可以并行取句柄，避免互斥锁把并发读串行化。
    state: RwLock<FuseState>,
    perf: FusePerfStats,
    passthrough_enabled: AtomicBool,
    /// 会话是否协商了 FUSE_WRITEBACK_CACHE：写进内核页缓存、延迟回刷后端。
    /// 决定文件 open/create 应答是否必须携带 FOPEN_KEEP_CACHE——缺失时内核在
    /// 每次 open 都失效该 inode 页缓存，第二个 fd 读正在写的文件会经 GETATTR
    /// 取到回刷滞后的后端 size（截断视图），读方（如录制 I 帧提取）据此判坏。
    writeback_enabled: AtomicBool,
}

struct FuseState {
    next_ino: u64,
    next_fh: u64,
    /// 路径 → inode。键是 [`FuseState::inode_key`] 规整后的形式：后端存储根开启
    /// casefold 时按 ASCII 小写折叠，与后端「大小写不同即同一文件」的语义一致。
    inodes: HashMap<String, u64>,
    /// 后端是否按大小写不敏感解析路径（会话建立时探测一次存储根）。
    casefold: bool,
    paths_by_inode: HashMap<u64, String>,
    /// 最近一次按应用策略解析每个 FUSE 路径，用于 root 子进程跨 namespace 接续访问。
    path_policy_uids: HashMap<String, u32>,
    inode_policy_uids: HashMap<u64, u32>,
    inode_path_versions: HashMap<u64, u64>,
    /// inode 级已解析后端（对齐原生 FuseDaemon 的 `node->GetIoPath()` 模型）：
    /// lookup 解析过一次的条目把后端留在 inode 上，随后的 getattr/readlink/xattr
    /// 直接复用，不再整条重跑策略路由。键是 inode 本身，生命周期与 inode 表一致
    /// （rename 改写 `paths_by_inode` 后 rel 比对必然 miss，重新解析即可），因此
    /// 不引入"路径缓存"那类陈旧语义。
    inode_backends: HashMap<u64, InodeBackendCacheEntry>,
    lookup_counts: HashMap<u64, u64>,
    dir_entry_refs: HashMap<u64, u64>,
    files: HashMap<u64, OpenFile>,
    backings: HashMap<(u64, u64), RegisteredBacking>,
    dirs: HashMap<u64, Arc<[DirEntry]>>,
    dir_candidate_cache: HashMap<String, CachedDirCandidates>,
    dir_candidate_cache_bytes: usize,
    dir_candidate_cache_byte_budget: usize,
    dir_candidate_cache_capacity: usize,
    dir_candidate_cache_max_capacity: usize,
}

/// inode 后端缓存条目：属主 uid + 解析结果。
///
/// 复用条件是**请求策略与登记时的属主一致**：跨进程 fd fstat（GETATTR 以他人
/// 身份到达）的视图差异必须保留原语义，不能拿属主的后端顶替。
#[derive(Clone)]
struct InodeBackendCacheEntry {
    owner_uid: u32,
    backend: Arc<BackendPath>,
}

/// 条目上限：`ls -lR` 一万条目的遍历会把 inode 全部带进来，每条目两段路径字符串
/// 约 300B，8192 上限即 2MB 量级；超限整体清空（与系统 uid 身份缓存同一兜底模式）。
const INODE_BACKEND_CACHE_CAP: usize = 8192;
/// 按路径属主表上限：每条为一段相对路径加 u32，16384 条约 2MB 量级。
const PATH_POLICY_UID_CAP: usize = 16384;
static INODE_BACKEND_CACHE_HITS: AtomicU64 = AtomicU64::new(0);

impl FuseState {
    fn next_handle(&mut self) -> u64 {
        let fh = self.next_fh;
        self.next_fh = self.next_fh.saturating_add(1).max(1);
        fh
    }

    fn clear_dir_candidate_cache(&mut self) {
        self.dir_candidate_cache.clear();
        self.dir_candidate_cache_bytes = 0;
    }

    /// inode 表的路径键。
    ///
    /// Android 的 /data/media 默认开启 casefold：`Tencent/x` 与 `tencent/x` 在后端是
    /// 同一个文件。若按原样字符串分配 inode，两种拼写会得到两个 nodeid，各自有独立的
    /// 页缓存与 i_size——writeback 下一侧写入后另一侧 fstat 的大小永远不变、read 立即
    /// 返回 EOF，轮询日志增长的线程就会空转（真机：微信命名空间内 `Download/` 写 201
    /// 字节，`download/` 读方停在 1 字节）。后端未开 casefold 时保持精确匹配，避免把
    /// 两个真实不同的文件合并成一个 inode。
    fn inode_key(&self, rel: &str) -> String {
        if self.casefold {
            rel.to_ascii_lowercase()
        } else {
            rel.to_string()
        }
    }
}

/// 探测后端根目录是否开启 casefold（ext4/f2fs 的 `FS_CASEFOLD_FL`）。
///
/// 只看存储根：Android 在建立 /data/media 时整棵树一次性设置，子目录继承。探测失败
/// 时按不折叠处理，等价于改造前的精确匹配语义。
fn backend_root_is_casefold(root: &Path) -> bool {
    const FS_CASEFOLD_FL: libc::c_long = 0x4000_0000;
    let Ok(dir) = File::open(root) else {
        return false;
    };
    let mut flags: libc::c_long = 0;
    // SAFETY: dir 是刚打开的有效目录 fd；FS_IOC_GETFLAGS 只向 flags 写入一个 long。
    let ret = unsafe { libc::ioctl(dir.as_raw_fd(), libc::FS_IOC_GETFLAGS, &mut flags) };
    ret == 0 && flags & FS_CASEFOLD_FL != 0
}

fn dir_candidate_cache_capacity_limits() -> (usize, usize, usize) {
    let total_kib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let mut fields = line.split_whitespace();
                (fields.next() == Some("MemTotal:")).then(|| fields.next()?.parse::<u64>().ok())?
            })
        })
        .unwrap_or(4 * 1024 * 1024);
    // 字节预算按设备内存分档：单个相册目录动辄上千条，原先 1MiB 预算下大目录
    // 直接判超限（真机会话快照 `dir_cache_oversize=3`、命中率 2%），缓存等于没装。
    // 宿主会话每设备只有一个，几 MB 的目录候选缓存换掉每目录 26ms 的重复枚举是
    // 划算的；上限仍按条目数（256）与字节预算双重约束，不会无界增长。
    let (max_capacity, byte_budget) = if total_kib < 4 * 1024 * 1024 {
        (64, 2 * 1024 * 1024)
    } else if total_kib < 8 * 1024 * 1024 {
        (128, 4 * 1024 * 1024)
    } else {
        (256, 8 * 1024 * 1024)
    };
    (
        INITIAL_DIR_CANDIDATE_CACHE_ENTRIES.min(max_capacity),
        max_capacity,
        byte_budget,
    )
}

#[derive(Clone)]
struct OpenFile {
    ino: INodeNo,
    public_uid: Option<u32>,
    backend: Arc<BackendPath>,
    file: Arc<File>,
    backing_key: Option<(u64, u64)>,
}

impl OpenFile {
    fn attr(&self) -> Result<FileAttr, Errno> {
        let mut attr =
            file_attr_from_metadata(self.ino, self.file.metadata().map_err(errno_from_io)?);
        if let Some(uid) = self.public_uid {
            attr.uid = uid;
            attr.gid = MEDIA_RW_GID;
        }
        Ok(attr)
    }
}

struct RegisteredBacking {
    id: Arc<fuser::BackingId>,
    handles: usize,
}

#[derive(Clone)]
struct DirEntry {
    ino: INodeNo,
    kind: FileType,
    name: String,
    rel: String,
}

#[derive(Clone)]
pub(super) struct DirEntryCandidate {
    rel: String,
    kind: FileType,
    name: String,
}

#[derive(Clone)]
pub(super) struct DirectorySourceSignature {
    path: PathBuf,
    modified: Option<SystemTime>,
    is_dir: Option<bool>,
}

impl DirectorySourceSignature {
    fn capture(path: &Path) -> Self {
        let _slow = slow_probe::SlowFsGuard::start("readdir_source_capture", path);
        let metadata = std::fs::metadata(path).ok();
        Self {
            path: path.to_path_buf(),
            modified: metadata.as_ref().and_then(|value| value.modified().ok()),
            is_dir: metadata.as_ref().map(std::fs::Metadata::is_dir),
        }
    }

    fn state(&self) -> DirectorySourceState {
        let _slow = slow_probe::SlowFsGuard::start("readdir_source_state", &self.path);
        let Some(metadata) = std::fs::metadata(&self.path).ok() else {
            return if self.modified.is_none() && self.is_dir.is_none() {
                DirectorySourceState::Current
            } else {
                DirectorySourceState::Missing
            };
        };
        let modified = metadata.modified().ok();
        let is_dir = Some(metadata.is_dir());
        if self.modified == modified && self.is_dir == is_dir {
            DirectorySourceState::Current
        } else {
            DirectorySourceState::Changed
        }
    }
}

enum DirectorySourceState {
    Current,
    Changed,
    Missing,
}

struct CachedDirCandidates {
    created_at: Instant,
    estimated_bytes: usize,
    candidates: Vec<DirEntryCandidate>,
    sources: Vec<DirectorySourceSignature>,
}

pub(super) fn estimate_cached_dir_candidates_bytes(
    rel: &str,
    candidates: &[DirEntryCandidate],
    sources: &[DirectorySourceSignature],
) -> usize {
    let candidate_bytes = candidates.iter().fold(0usize, |total, candidate| {
        total
            .saturating_add(candidate.rel.len())
            .saturating_add(candidate.name.len())
            .saturating_add(std::mem::size_of::<DirEntryCandidate>())
    });
    let source_bytes = sources.iter().fold(0usize, |total, source| {
        total
            .saturating_add(source.path.to_string_lossy().len())
            .saturating_add(std::mem::size_of::<DirectorySourceSignature>())
    });
    rel.len()
        .saturating_add(candidate_bytes)
        .saturating_add(source_bytes)
        .saturating_add(INITIAL_DIR_CANDIDATE_CACHE_BYTES / 64)
}

/// 新建条目（create/mknod/mkdir）共用的前置解析结果。
struct NewEntryRoute {
    policy: Arc<RedirectPolicy>,
    rel: String,
    backend: BackendPath,
}

/// 宿主共享会话的「系统读取者」白名单：MediaProvider 应用 uid 与 media/media_rw
/// 系统身份。这些调用方按路径访问应用重定向树是系统扫描/元数据协商的正常路径，
/// 不属于 scoped storage 越权；其余应用 uid 一律维持 deny_all。
fn is_system_reader_uid(uid: u32) -> bool {
    uid == 1013
        || uid == 1023
        || [
            "com.android.providers.media.module",
            "com.google.android.providers.media.module",
            "com.android.providers.media",
        ]
        .iter()
        .any(|package| {
            policy::package_uid(package)
                .is_some_and(|app_id| app_id >= 10000 && uid % 100000 == app_id as u32 % 100000)
        })
}

impl FuseRedirectFs {
    /// 系统读取者的路径解析（miss 重试语义）：先用请求者自身策略解析——MP 在宿主
    /// 共享会话有已登记策略（公共视图），命中即维持原语义；解析失败且请求者为
    /// 白名单 uid（MP/媒体身份）时，改用目标应用的属主策略重试——文件在被重定向
    /// 的后端落点而公共视图不存在，正是本轮 QQ 相机 fd 协商 ENOENT 的路径侧同源
    /// 场景。仅 Read 类，仅 `Android/data/<pkg>/` 前缀且该应用已登记；写路径与
    /// 其它应用 uid 一律维持原解析结果（宿主拒绝未登记调用方写入的沙盒设计）。
    fn backend_for_relative_with_system_reader(
        &self,
        uid: u32,
        policy: &Arc<RedirectPolicy>,
        rel: &str,
    ) -> Option<(Arc<RedirectPolicy>, BackendPath)> {
        if let Some(backend) = policy.backend_for_relative(rel, OperationKind::Read) {
            return Some((Arc::clone(policy), backend));
        }
        let owner_policy = self.system_reader_policy_on_miss(uid, policy, rel)?;
        let backend = owner_policy.backend_for_relative(rel, OperationKind::Read)?;
        Some((owner_policy, backend))
    }

    /// 请求者策略解析 miss 时的系统读取者重试：命中则返回属主策略，调用方以它
    /// 重新执行原解析并沿同一视角完成 attr 塑形与应答；未命中返回 None 维持原结果。
    fn system_reader_policy_on_miss(
        &self,
        uid: u32,
        _policy: &Arc<RedirectPolicy>,
        rel: &str,
    ) -> Option<Arc<RedirectPolicy>> {
        if !is_system_reader_uid(uid) {
            return None;
        }
        let package_name = rel
            .strip_prefix("Android/data/")
            .and_then(|rest| rest.split('/').next())
            .filter(|name| !name.is_empty())?;
        let owner_policy = self.policy.find_owner_policy_by_package(package_name)?;
        owner_policy.backend_for_relative(rel, OperationKind::Read)?;
        static SYSTEM_READER_COUNT: AtomicU64 = AtomicU64::new(0);
        let seen = SYSTEM_READER_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if seen == 1 || seen.is_multiple_of(256) {
            log::debug!(
                "fuse system-reader resolve uid={} pkg={} rel={} count={}",
                uid,
                package_name,
                rel,
                seen
            );
        }
        Some(owner_policy)
    }
}

impl FuseRedirectFs {
    /// 取出按 uid 策略表句柄。
    ///
    /// 宿主共享会话在 `spawn_mount2` 交出 `FuseRedirectFs` 之后，仍需要按 uid 登记应用策略，
    /// 因此必须在交出所有权之前把句柄取出来，交给控制通道。
    pub(crate) fn policy_table(&self) -> SharedPolicyTable {
        self.policy.shared_table()
    }

    fn new(config: FuseRedirectConfig) -> Option<Self> {
        let package_name = config.package_name.clone();
        // 宿主共享会话服务的是完整存储视图：未登记 uid 必须一律拒绝，否则它会回退到直通策略，
        // 直接读写真实存储——沙盒失效，且越权产生的落点无法靠事后清理恢复。
        let deny_unregistered = config.is_passthrough_host;
        let policy = PolicyRegistry::new(RedirectPolicy::new(config)?, deny_unregistered);
        let (dir_cache_capacity, dir_cache_max_capacity, dir_cache_byte_budget) =
            dir_candidate_cache_capacity_limits();
        let casefold = backend_root_is_casefold(&policy.session().real_root);
        log::info!(
            "fuse inode keying pkg={} casefold={} real_root={}",
            package_name,
            casefold,
            policy.session().real_root.display()
        );
        let mut inodes = HashMap::new();
        let mut paths_by_inode = HashMap::new();
        inodes.insert(String::new(), ROOT_INO);
        paths_by_inode.insert(ROOT_INO, String::new());

        let perf = FusePerfStats::new(package_name);
        perf.record_dir_cache_capacity(dir_cache_capacity, dir_cache_max_capacity);
        perf.record_dir_cache_budget(0, dir_cache_byte_budget);
        perf.log_dir_cache_config();

        Some(Self {
            policy,
            perf,
            passthrough_enabled: AtomicBool::new(false),
            writeback_enabled: AtomicBool::new(false),
            state: RwLock::new(FuseState {
                next_ino: ROOT_INO + 1,
                next_fh: 1,
                inodes,
                casefold,
                paths_by_inode,
                path_policy_uids: HashMap::new(),
                inode_policy_uids: HashMap::new(),
                inode_path_versions: HashMap::from([(ROOT_INO, 0)]),
                inode_backends: HashMap::new(),
                lookup_counts: HashMap::new(),
                dir_entry_refs: HashMap::new(),
                files: HashMap::new(),
                backings: HashMap::new(),
                dirs: HashMap::new(),
                dir_candidate_cache: HashMap::new(),
                dir_candidate_cache_bytes: 0,
                dir_candidate_cache_byte_budget: dir_cache_byte_budget,
                dir_candidate_cache_capacity: dir_cache_capacity,
                dir_candidate_cache_max_capacity: dir_cache_max_capacity,
            }),
        })
    }

    /// 文件 open/create 应答的 FOPEN 标志：writeback 回写模式下必须携带
    /// FOPEN_KEEP_CACHE。缺失时内核每次 open 都失效该 inode 的页缓存，第二个
    /// fd 读「正在写的文件」会经 GETATTR 取到回刷滞后的后端 size（截断视图），
    /// 例如录制中 I 帧提取管线打开 growing mp4 直接判坏。与 passthrough open
    /// 应答（恒带 KEEP_CACHE）保持一致。
    pub(super) fn file_open_flags(&self) -> FopenFlags {
        if self.writeback_enabled.load(Ordering::Relaxed) {
            FopenFlags::FOPEN_KEEP_CACHE
        } else {
            FopenFlags::empty()
        }
    }

    // 同一后端 inode 的并存句柄复用注册，直到最后一个 release 才注销。
    fn backing_for_open_file(
        &self,
        fh: u64,
        file: &File,
        register: impl FnOnce() -> std::io::Result<fuser::BackingId>,
    ) -> std::io::Result<Arc<fuser::BackingId>> {
        let metadata = file.metadata()?;
        let key = (metadata.dev(), metadata.ino());
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        if !state.files.contains_key(&fh) {
            return Err(std::io::Error::from_raw_os_error(libc::EBADF));
        }
        let backing = match state.backings.entry(key) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => entry.insert(RegisteredBacking {
                id: Arc::new(register()?),
                handles: 0,
            }),
        };
        backing.handles += 1;
        let id = Arc::clone(&backing.id);
        if let Some(open_file) = state.files.get_mut(&fh) {
            open_file.backing_key = Some(key);
        }
        Ok(id)
    }

    // rename 覆盖或 unlink 后路径已失效，fd 属性仍从打开的后端文件读取。
    fn open_file_for_inode(&self, ino: INodeNo, fh: Option<fuser::FileHandle>) -> Option<OpenFile> {
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        if let Some(fh) = fh {
            state.files.get(&fh.0).filter(|file| file.ino == ino)
        } else {
            state.files.values().find(|file| file.ino == ino)
        }
        .cloned()
    }

    fn policy_for_request(&self, req: &Request) -> Arc<RedirectPolicy> {
        self.policy.for_request(req.uid(), req.pid())
    }

    /// 系统 uid 请求者的监视归因（仅监视开启且走系统回退策略时解析）。
    ///
    /// daemon 以 root 运行，直接读 `/proc/<pid>` 反查发起者（MP 域的 SELinux 限制
    /// 不存在），身份可信度由 status 的 Uid 行裁决；pid 是发起任务的强信号，短
    /// TTL 缓存吸收 write 等高频路径的重复解析。非系统 uid 或未走系统回退策略
    /// （应用 uid 命中自身策略）时返回 None，记录维持会话默认形态。
    fn requester_attribution(
        &self,
        req: &Request,
        policy: &RedirectPolicy,
    ) -> Option<RequesterIdentity> {
        if !policy.is_file_monitor_enabled || !policy.is_system_fallback {
            return None;
        }
        let uid = req.uid() as i32;
        if uid <= 0 || uid >= ANDROID_APP_UID_START {
            return None;
        }
        let pid = req.pid() as i32;
        if pid <= 0 {
            return None;
        }
        let now_ms = crate::platform::paths::monotonic_ms();
        if let Ok(cache) = SYSTEM_REQUESTER_CACHE.lock()
            && let Some((at_ms, identity)) = cache.get(&pid)
            && now_ms.saturating_sub(*at_ms) <= SYSTEM_REQUESTER_CACHE_TTL_MS
        {
            return match identity {
                Some(cached) if cached.uid == uid => Some(cached.clone()),
                _ => None,
            };
        }
        let identity = crate::platform::proc_identity::read_proc_identity(pid)
            .filter(|(proc_uid, _)| *proc_uid == uid)
            .map(|(proc_uid, package)| RequesterIdentity {
                package,
                uid: proc_uid,
            });
        if let Ok(mut cache) = SYSTEM_REQUESTER_CACHE.lock() {
            if cache.len() >= SYSTEM_REQUESTER_CACHE_CAP {
                cache.clear();
            }
            cache.insert(pid, (now_ms, identity.clone()));
        }
        identity
    }

    fn policy_for_read_request(
        &self,
        req: &Request,
        ino: Option<INodeNo>,
        rel: Option<&str>,
    ) -> Arc<RedirectPolicy> {
        let policy = self.policy_for_request(req);
        if req.uid() != 0 || !policy.is_deny_all() {
            return policy;
        }
        let cached_uid = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            ino.and_then(|value| state.inode_policy_uids.get(&value.0).copied())
                .or_else(|| rel.and_then(|value| state.path_policy_uids.get(value).copied()))
        };
        if let Some(cached_uid) = cached_uid
            && let Some(cached_policy) = self.policy.for_uid_exact(cached_uid)
        {
            let count = INODE_POLICY_FALLBACKS.fetch_add(1, Ordering::Relaxed) + 1;
            if count == 1 || count.is_multiple_of(256) {
                log::info!(
                    "fuse policy inode fallback uid=0 pid={} ino={} rel={} app_uid={} pkg={} count={}",
                    req.pid(),
                    ino.map(|value| value.0).unwrap_or(0),
                    rel.unwrap_or(""),
                    cached_uid,
                    cached_policy.package_name,
                    count
                );
            }
            return cached_policy;
        }
        policy
    }

    /// fstat 已打开 fd（GETATTR 带 fh）的属主策略回退：fd 可经 binder 跨进程复制
    /// （如 QQ 录制结束后把视频 fd 交给 MediaProvider 做原始格式协商，MP 进程对
    /// 该 fd fstat 时 FUSE GETATTR 以 MP 的 uid 到达 daemon）。fd 的文件身份在
    /// open 时刻已固定，不能按请求者 uid 重新路由 rel 路径——非属主解析必然
    /// ENOENT。属主与请求者一致或属主策略已注销时返回 None，沿用调用方策略。
    fn inode_owner_policy_for_fd_stat(
        &self,
        ino: INodeNo,
        requester_uid: u32,
    ) -> Option<Arc<RedirectPolicy>> {
        let owner_uid = {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            state.inode_policy_uids.get(&ino.0).copied()?
        };
        let owner_policy = self.policy.for_uid_exact(owner_uid)?;
        if owner_policy.uid as u32 == requester_uid {
            return None;
        }
        log::debug!(
            "fuse getattr fd-owner fallback ino={:x} requester_uid={} owner_uid={}",
            ino.0,
            requester_uid,
            owner_uid
        );
        Some(owner_policy)
    }

    fn remember_policy_for_inode_locked(
        state: &mut FuseState,
        ino: INodeNo,
        rel: &str,
        policy: &RedirectPolicy,
    ) {
        // 跨进程 fstat 回退以「应用属主」为唯一可信登记：系统身份（MP/media/root）
        // 的解析晚到、失败（attr ENOENT 时 remember 已先行执行）或视图不同，都不得
        // 覆盖已登记的应用属主；应用属主之间先到者保留。
        let uid = policy.uid as u32;
        let is_app_uid = uid >= 10000;
        if let Some(&existing) = state.inode_policy_uids.get(&ino.0)
            && (!is_app_uid || (existing >= 10000 && existing != uid))
        {
            return;
        }
        if let Some(&existing) = state.path_policy_uids.get(rel)
            && (!is_app_uid || (existing >= 10000 && existing != uid))
        {
            return;
        }
        if state.path_policy_uids.len() >= PATH_POLICY_UID_CAP
            && !state.path_policy_uids.contains_key(rel)
        {
            // 按路径的属主表只在 inode 已被 forget 后兜底 root 接续访问；宿主会话常驻，
            // 不设上限会随访问过的路径数无界增长。超限整体清空，与 inode 后端缓存同一模式，
            // 仍存活的 inode 由 inode_policy_uids 继续承接。
            state.path_policy_uids.clear();
        }
        state.path_policy_uids.insert(rel.to_string(), uid);
        state.inode_policy_uids.insert(ino.0, uid);
    }

    fn ino_for_path_locked(state: &mut FuseState, rel: &str) -> INodeNo {
        let key = state.inode_key(rel);
        if let Some(ino) = state.inodes.get(&key).copied() {
            return INodeNo(ino);
        }
        let ino = state.next_ino;
        state.next_ino = state.next_ino.saturating_add(1).max(ROOT_INO + 1);
        state.inodes.insert(key, ino);
        state.paths_by_inode.insert(ino, rel.to_string());
        state.inode_path_versions.insert(ino, 0);
        INodeNo(ino)
    }

    /// 登记本 inode 最近一次解析出的后端（`reply_entry_for_rel` 在 lookup 锁内调用）。
    fn remember_backend_for_inode_locked(
        state: &mut FuseState,
        ino: INodeNo,
        policy: &RedirectPolicy,
        backend: Arc<BackendPath>,
    ) {
        if ino.0 == ROOT_INO || state.inode_backends.len() >= INODE_BACKEND_CACHE_CAP {
            // 超限整体清空：条目只是加速件，清空后下一轮 lookup 自然重建。
            state.inode_backends.clear();
        }
        state.inode_backends.insert(
            ino.0,
            InodeBackendCacheEntry {
                owner_uid: policy.uid as u32,
                backend,
            },
        );
    }

    fn add_lookup_locked(state: &mut FuseState, ino: INodeNo) {
        if ino.0 != ROOT_INO {
            let count = state.lookup_counts.entry(ino.0).or_default();
            *count = count.saturating_add(1);
        }
    }

    fn remove_lookup_locked(state: &mut FuseState, ino: INodeNo, count: u64) {
        if let Some(current) = state.lookup_counts.get_mut(&ino.0) {
            *current = current.saturating_sub(count);
            if *current == 0 {
                state.lookup_counts.remove(&ino.0);
            }
        }
        remove_unreferenced_inode(state, ino.0);
    }

    fn path_for_ino(&self, ino: INodeNo) -> Option<String> {
        let state = self.state.read().unwrap_or_else(|err| err.into_inner());
        state.paths_by_inode.get(&ino.0).cloned()
    }

    /// 按已解析后端回答 inode 元数据类请求。命中条件：登记属主与请求策略一致
    /// 且 `paths_by_inode` 的 rel 未变（rename/unlink 会改写或移除该表，命中前
    /// 必然先比对），否则退回完整路由并更新缓存。rel 比对让陈旧条目只会造成
    /// 重复解析，永远不会给出错误后端。
    fn backend_for_ino(
        &self,
        policy: &RedirectPolicy,
        ino: INodeNo,
    ) -> Result<Arc<BackendPath>, Errno> {
        let rel = self.path_for_ino(ino).ok_or(Errno::ENOENT)?;
        {
            let state = self.state.read().unwrap_or_else(|err| err.into_inner());
            if let Some(entry) = state.inode_backends.get(&ino.0)
                && entry.owner_uid == policy.uid as u32
                && entry.backend.rel == rel
            {
                let count = INODE_BACKEND_CACHE_HITS.fetch_add(1, Ordering::Relaxed) + 1;
                if count == 1 || count.is_multiple_of(8192) {
                    // 会话侧 info 不进 running.log（宿主只转发 warn 以上），命中率
                    // 取证用 warn 限频落盘。
                    log::warn!(
                        "fuse inode backend cache hit uid={} ino={} rel={} count={}",
                        policy.uid,
                        ino.0,
                        rel,
                        count
                    );
                }
                return Ok(Arc::clone(&entry.backend));
            }
        }
        policy
            .backend_for_relative(&rel, OperationKind::Read)
            .map(Arc::new)
            .ok_or(Errno::ENOENT)
    }

    fn backend_for_relative(
        &self,
        policy: &RedirectPolicy,
        rel: &str,
        operation: OperationKind,
    ) -> Result<BackendPath, Errno> {
        policy
            .backend_for_relative(rel, operation)
            .ok_or(Errno::ENOENT)
    }

    /// xattr 系 op 的公共后端路由：错误统一走应答错误观测，成功后以
    /// backend 的 NUL 结尾路径执行动作。
    fn with_backend_for_xattr(
        &self,
        policy: &RedirectPolicy,
        ino: INodeNo,
        op: &'static str,
        action: impl FnOnce(&std::ffi::CString) -> Result<callbacks::XattrOutcome, Errno>,
    ) -> Result<callbacks::XattrOutcome, Errno> {
        let backend = self.backend_for_ino(policy, ino).inspect_err(|&errno| {
            let rel = self.path_for_ino(ino);
            self.log_reply_error(op, policy.uid as u32, rel.as_deref(), errno);
        })?;
        let c_path = metadata::cstring_path(&backend.path)?;
        action(&c_path)
    }

    fn child_rel(parent_rel: &str, name: &OsStr) -> Result<String, Errno> {
        let name_bytes = name.as_bytes();
        if name_bytes.is_empty()
            || name_bytes.contains(&0)
            || name_bytes == b"."
            || name_bytes == b".."
            || name_bytes.contains(&b'/')
        {
            return Err(Errno::EINVAL);
        }
        let name_text = String::from_utf8_lossy(name_bytes).to_string();
        if parent_rel.is_empty() {
            Ok(name_text)
        } else {
            Ok(paths::join(parent_rel, &name_text))
        }
    }

    /// 新建条目的公共前置解析：父 inode → 子相对路径 → 写后端 → 只读拒绝 →
    /// 确保父目录。任一步失败返回 `Errno`，由调用方以各自的 Reply 类型应答；
    /// 只读拒绝在返回前按操作名发出监视事件，三个回调的告警语义与抽取前一致。
    fn route_new_entry(
        &self,
        req: &Request,
        parent: INodeNo,
        name: &OsStr,
        operation_name: &'static str,
    ) -> Result<NewEntryRoute, Errno> {
        let Some(parent_rel) = self.path_for_ino(parent) else {
            return Err(Errno::ENOENT);
        };
        let rel = Self::child_rel(&parent_rel, name)?;
        let policy = self.policy_for_request(req);
        let mut backend = self.backend_for_relative(&policy, &rel, OperationKind::Write)?;
        // HyperOS 下载 bind 在任务入队瞬间创建，快照 TTL 可能刚好错过；新建入口
        // 强制重读 init mountinfo，避免首次 CREATE 与 bind 创建竞态时落到被遮蔽的
        // 真实目录（真机 2026-10-19：商店增量下载校验 error=3 的直接成因）。
        if let Some(resolved) = backend_overlay::overlay_backend_path_fresh(&backend.path) {
            backend.path = resolved;
        }
        if backend.is_read_only {
            policy.emit_monitor_read_only_deny(operation_name, &backend);
            return Err(Errno::EROFS);
        }
        self.ensure_parent_for_backend(&policy, &backend)?;
        Ok(NewEntryRoute {
            policy,
            rel,
            backend,
        })
    }

    /// 视图根节点（挂载点本身）的展示属性对齐。
    ///
    /// 视图根上提后，根节点在基线里就是 `/storage/emulated`（`0550 media_rw:media_rw`），
    /// 而它的后端是「用户存储根」（`2770`）。应用只要 `stat("/storage/emulated")` 就能看出
    /// mode 差异，所以根的展示属性直接取系统视图根的真实值。
    ///
    /// 与 `attr_for_backend` 的边界：那里一律透传后端事实（**不得**把既有目录归一成模块
    /// 常量，否则公共目录的 02773/02771 会被压平）；这里只处理"根节点本身"，且取值来自
    /// 系统视图而非模块常量。`dev`/`ino` 是内核事实，保持我方值。
    fn apply_view_root_attr(&self, policy: &RedirectPolicy, ino: INodeNo, attr: &mut FileAttr) {
        if !policy.view_lifted || ino.0 != 1 {
            return;
        }
        let Some(system_root) = self.lifted_root_attr(policy) else {
            // 读不到系统视图根时保持后端属性：宁可少一次对齐，也不能让 getattr 失败。
            return;
        };
        attr.perm = system_root.perm;
        attr.uid = system_root.uid;
        attr.gid = system_root.gid;
        attr.nlink = system_root.nlink;
        attr.size = system_root.size;
        attr.blocks = system_root.blocks;
        attr.blksize = system_root.blksize;
    }

    /// 系统视图根（`/storage/emulated`）的真实属性，用于视图根节点的展示对齐。
    ///
    /// 读取发生在会话进程自己的命名空间：那里 `/storage/emulated` 仍是系统
    /// MediaProvider 视图，不会回到本会话（本会话挂在 `FUSE_HOST_MOUNT_POINT`）。
    /// 读不到时返回 `None`，调用方保持后端属性——宁可少一次对齐，也不能让 getattr 失败。
    fn lifted_root_attr(&self, policy: &RedirectPolicy) -> Option<FileAttr> {
        let system_root = crate::platform::paths::parent(
            &crate::platform::paths::storage_user_root_for_user(policy.user_id),
        );
        let metadata = std::fs::symlink_metadata(&system_root).ok()?;
        Some(file_attr_from_metadata(INodeNo(1), metadata))
    }

    fn attr_for_backend(
        &self,
        policy: &RedirectPolicy,
        ino: INodeNo,
        backend: &BackendPath,
    ) -> Result<FileAttr, Errno> {
        let _slow = slow_probe::SlowFsGuard::start("attr_for_backend", &backend.path);
        let metadata = std::fs::symlink_metadata(&backend.path).map_err(errno_from_io)?;
        let mut attr = file_attr_from_metadata(ino, metadata);
        if backend.is_shared_public_backend {
            attr.uid = policy.uid as u32;
            attr.gid = MEDIA_RW_GID;
            // **不改写目录 mode**：这里曾经无条件把目录权限压成 `SHARED_PUBLIC_DIR_MODE`
            // (02770)，但该常量是「我们新建目录时落盘的权限」，用它覆盖**已存在**节点的
            // 展示值会让原生形态暴露。真机 2026-11-02 应用域实测（应用进程内 stat）：
            // `/storage/emulated/0/DCIM` 原生 `1023:1023 02773`、`Android` 原生 `02771`，
            // 改写后统一变成 `02770` —— 只要 stat 一次公共目录就能看出来。
            //
            // 展示层只负责「属主归一」（见下方注释），mode/nlink/size 一律透传后端事实：
            // 本模块新建目录落盘就是 02770，与原生新建一致；系统既有目录保持 02773/02771。
        }
        Ok(attr)
    }

    fn visible_attr_for_backend(
        &self,
        policy: &RedirectPolicy,
        ino: INodeNo,
        backend: &BackendPath,
    ) -> Result<FileAttr, Errno> {
        match self.attr_for_backend(policy, ino, backend) {
            Ok(mut attr) => {
                self.apply_view_root_attr(policy, ino, &mut attr);
                Ok(attr)
            }
            Err(errno) if errno.code() == libc::ENOENT && policy.is_virtual_dir(&backend.rel) => {
                Ok(synthetic_dir_attr(ino, policy.uid as u32, MEDIA_RW_GID))
            }
            Err(errno) => Err(errno),
        }
    }

    fn reply_entry_for_rel(
        &self,
        uid: u32,
        policy: &Arc<RedirectPolicy>,
        rel: &str,
        reply: ReplyEntry,
    ) {
        // lookup 是系统读取者路径反解的主入口：MP 对重定向树的路径访问先经此建立 dentry。
        let Some((policy, backend)) =
            self.backend_for_relative_with_system_reader(uid, policy, rel)
        else {
            // 双重 miss（请求者视图 + 系统读取者重试）：ENOENT 归因由
            // log_reply_error 承担，log_new_entry_errno 的 create 诊断不适用。
            self.log_reply_error("lookup", uid, Some(rel), Errno::ENOENT);
            reply.error(Errno::ENOENT);
            return;
        };
        let policy = &policy;
        let backend = Arc::new(backend);
        let ino = {
            let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
            let ino = Self::ino_for_path_locked(&mut state, rel);
            Self::remember_policy_for_inode_locked(&mut state, ino, rel, policy);
            Self::remember_backend_for_inode_locked(&mut state, ino, policy, Arc::clone(&backend));
            Self::add_lookup_locked(&mut state, ino);
            ino
        };
        match self.visible_attr_for_backend(policy, ino, &backend) {
            Ok(attr) => reply.entry(&ENTRY_TTL, &attr, Generation(0)),
            Err(errno) => {
                self.log_reply_error("lookup", policy.uid as u32, Some(rel), errno);
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                Self::remove_lookup_locked(&mut state, ino, 1);
                reply.error(errno);
            }
        }
    }

    fn ensure_parent_for_backend(
        &self,
        policy: &RedirectPolicy,
        backend: &BackendPath,
    ) -> Result<(), Errno> {
        let Some(parent) = backend.path.parent() else {
            return Ok(());
        };
        let parent = parent.to_string_lossy();
        if parent.is_empty() {
            return Ok(());
        }
        let owner_uid = if !crate::metadata_repair::enabled() {
            -1
        } else if backend.is_shared_public_backend {
            MEDIA_RW_UID as i32
        } else {
            policy.uid
        };
        let parent_path = Path::new(parent.as_ref());
        let existed = fs::is_directory(&parent);
        if existed || fs::create_directory(&parent, owner_uid) {
            // 覆盖挂载目标（如 HyperOS apk-tmp 下载目录）由系统按下载方身份自管
            // 属主与权限，既有层级不得改写；仅本会话新建的层级按调用方身份修复。
            // 已存在的父目录必须整体跳过：既有目录由系统原生语义和独立迁移流程负责，
            // 不在 FUSE 新建入口重复改写其 owner/group/mode。此前这里把 Android/data
            // 私有树的 ext_data_rw（1078）覆盖成 media_rw（1023），造成 MicroMsg
            // 每次启动后 gid 漂移。
            // 私有树既有层级的属主归位由 owner_migration 与 runtime 代写 hook 负责，
            // 这里只修「本会话刚创建」的层级。
            if !existed {
                let metadata = create_metadata(
                    policy.uid as u32,
                    MEDIA_RW_GID,
                    policy.uid as u32,
                    MAPPED_DIR_MODE,
                    backend.is_shared_public_backend,
                    backend.is_app_private_backend,
                    true,
                );
                apply_create_metadata(parent_path, metadata)?;
            }
            Ok(())
        } else {
            Err(Errno::EIO)
        }
    }

    fn invalidate_dir_candidate_cache(&self) {
        let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
        state.clear_dir_candidate_cache();
    }

    /// open 失败回包留痕：EIO 这类「不该出现」的 errno 是宿主数据面故障的唯一直接
    /// 痕迹（实测反馈设备 QQ 打开 chatpic 缩略图收到 EIO，且无其它任何日志）。
    /// ENOENT 是常规探测、EACCES/EROFS 已有监视台账单独记录，均不重复留痕。
    /// W 级日志不受 verbose 门禁，宿主进程必然落入 running.log。
    /// create/mkdir 等新建入口的异常 errno 诊断：写路径 O_CREAT 走 create 回调而非 open，
    /// 不补这个点则「未登记 uid / 旧映射残留」的 ENOENT 与旧只读策略的 EROFS 全部无痕。
    /// xattr 系 op 的策略解析：与 getattr 一致的属主回退——跨进程 fd 的
    /// getxattr/setxattr 同样不能按请求者 uid 重路由 rel 路径。
    fn policy_for_fd_stat_or_request(&self, req: &Request, ino: INodeNo) -> Arc<RedirectPolicy> {
        let policy = self.policy_for_read_request(req, Some(ino), None);
        if let Some(owner_policy) = self.inode_owner_policy_for_fd_stat(ino, req.uid()) {
            return owner_policy;
        }
        policy
    }

    /// 统一应答错误观测。ENOENT/EACCES 是热路径正常语义，仅 debug 日志开启时
    /// 记录——此前 QQ 相机失败链中 MP 侧 getattr ENOENT 因宿主应答完全无观测
    /// 成为盲区，这次补上。其余错误罕见且排查价值高，warn 级采样（前 16 次全
    /// 记，之后每 256 次记一次）兼顾证据留存与模块日志 20s 轮转的限流约束。
    fn log_reply_error(&self, op: &'static str, uid: u32, rel: Option<&str>, errno: Errno) {
        let errno_value = i32::from(errno);
        if matches!(errno_value, libc::ENOENT | libc::EACCES) {
            if crate::logging::is_debug_logging_enabled() {
                log::debug!("fuse {op} error uid={uid} rel={rel:?} errno={errno_value}");
            }
            return;
        }
        static UNEXPECTED_COUNT: AtomicU64 = AtomicU64::new(0);
        let seen = UNEXPECTED_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
        if seen > 16 && !seen.is_multiple_of(256) {
            return;
        }
        log::warn!("fuse {op} error uid={uid} rel={rel:?} errno={errno_value} seen={seen}");
    }

    fn log_new_entry_errno(policy: &RedirectPolicy, name: &OsStr, errno: Errno) {
        let errno_value = i32::from(errno);
        if !matches!(errno_value, libc::ENOENT | libc::EACCES | libc::EROFS) {
            return;
        }
        if errno_value == libc::EROFS {
            log::warn!(
                "fuse create read-only deny pkg={} name={:?} errno={} policy_fp={:016x} monitor_only={}",
                policy.package_name,
                name,
                errno_value,
                policy.fingerprint,
                policy.is_monitor_only
            );
            return;
        }
        let seen = CREATE_ENOENT_DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
        if seen >= 4 {
            return;
        }
        log::warn!(
            "fuse create enoent diag pkg={} name={:?} errno={} policy_fp={:016x} deny_all={} monitor_only={} seen={}",
            policy.package_name,
            name,
            errno_value,
            policy.fingerprint,
            policy.is_deny_all(),
            policy.is_monitor_only,
            seen + 1
        );
    }

    fn log_unexpected_open_errno(policy: &RedirectPolicy, rel: &str, errno: Errno) {
        let errno_value = i32::from(errno);
        if errno_value == libc::EROFS {
            // 只读拒绝在正常场景只来自 read_only_paths；若出现的路径不在新配置的只读表里，
            // 这个指纹就是「宿主仍持旧策略」的直接证据（全量场景跑的确定性 EROFS 排查锚点）。
            log::warn!(
                "fuse open read-only deny pkg={} rel={} errno={} policy_fp={:016x} monitor_only={}",
                policy.package_name,
                rel,
                errno_value,
                policy.fingerprint,
                policy.is_monitor_only
            );
            return;
        }
        if matches!(errno_value, libc::ENOENT | libc::EACCES) {
            // ENOENT 是 deny_all/旧映射残留的主要表象，量可能很大：会话级限 4 条，
            // 带指纹即可判定「未登记 uid」与「旧映射残留」。
            let seen = OPEN_ENOENT_DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
            if seen >= 4 {
                return;
            }
            log::warn!(
                "fuse open enoent diag pkg={} rel={} errno={} policy_fp={:016x} deny_all={} monitor_only={} seen={}",
                policy.package_name,
                rel,
                errno_value,
                policy.fingerprint,
                policy.is_deny_all(),
                policy.is_monitor_only,
                seen + 1
            );
            return;
        }
        log::warn!(
            "fuse open unexpected errno pkg={} rel={} errno={}",
            policy.package_name,
            rel,
            errno_value
        );
    }

    fn open_backend_file(path: &Path, flags: i32, mode: u32) -> Result<File, Errno> {
        let _slow = slow_probe::SlowFsGuard::start("open_backend_file", path);
        // FUSE writeback 回写模式下的部分页写，内核要先读原页（read-modify-write），
        // 该读仍由本 daemon 应答。backend 若照搬应用的 O_WRONLY 打开，pread 会返回
        // EBADF，内核 write_begin 失败后在应用侧呈现为诡异的 pwrite 短写（夸克下载
        // 引擎的 TNet 日志与下载目标都是 O_WRONLY，短写让其进入重建文件死循环）。
        // daemon 以自身权限打开 backend，写意图一律升级为 O_RDWR；O_PATH 打开保持
        // 原样（其 fd 不可读写是内核语义，升级反而破坏路径探测）。
        let mut backend_flags = flags | libc::O_CLOEXEC;
        if backend_flags & libc::O_PATH == 0 && backend_flags & libc::O_ACCMODE != libc::O_RDONLY {
            backend_flags = (backend_flags & !libc::O_ACCMODE) | libc::O_RDWR;
        }
        // 后端绝不能带 O_APPEND：FUSE WRITE 请求携带的是内核算好的绝对偏移（writeback
        // 回写时更是按页重写已有区段），daemon 以 pwrite(offset) 落盘；而 Linux 在
        // O_APPEND fd 上的 pwrite 忽略 offset、一律追加到末尾。于是每次回写都把同一段
        // 数据重复接到后端尾部（真机：视图 500 字节、后端 12750 字节，微信 xlog 视图与
        // 后端尺寸分叉）。追加语义由内核在 FUSE 层维护（O_APPEND 打开时内核按 i_size
        // 计算写偏移），后端只需按偏移写。
        backend_flags &= !libc::O_APPEND;
        let c_path = cstring_path(path)?;
        // SAFETY: c_path 是合法的 NUL 结尾字符串，backend_flags/mode 为整数参数，
        // open 只在失败时返回 -1，成功返回的 fd 由 File 接管所有权。
        let fd = unsafe { libc::open(c_path.as_ptr(), backend_flags, mode) };
        if fd < 0 {
            Err(errno_from_code(last_errno()))
        } else {
            // SAFETY: fd 刚由 open 成功返回且未被其它对象接管，转移给 File 独占管理。
            Ok(unsafe { File::from_raw_fd(fd) })
        }
    }

    fn remove_child(
        &self,
        policy: &RedirectPolicy,
        parent: INodeNo,
        name: &OsStr,
        is_dir: bool,
        caller: Option<&RequesterIdentity>,
        reply: ReplyEmpty,
    ) {
        let Some(parent_rel) = self.path_for_ino(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let rel = match Self::child_rel(&parent_rel, name) {
            Ok(rel) => rel,
            Err(errno) => {
                reply.error(errno);
                return;
            }
        };
        let Some(backend) = policy.backend_for_relative(&rel, OperationKind::Read) else {
            reply.error(Errno::ENOENT);
            return;
        };
        if backend.is_read_only {
            policy.emit_monitor_read_only_deny(if is_dir { "rmdir" } else { "unlink" }, &backend);
            reply.error(Errno::EROFS);
            return;
        }
        let result = if is_dir {
            std::fs::remove_dir(&backend.path)
        } else {
            std::fs::remove_file(&backend.path)
        };
        match result {
            Ok(()) => {
                let mut state = self.state.write().unwrap_or_else(|err| err.into_inner());
                remove_inode_path(&mut state, &rel);
                state.clear_dir_candidate_cache();
                policy.emit_monitor_mutation(
                    if is_dir { "rmdir" } else { "unlink" },
                    &backend,
                    None,
                    caller,
                );
                reply.ok();
            }
            Err(error) => reply.error(errno_from_io(error)),
        }
    }
}
