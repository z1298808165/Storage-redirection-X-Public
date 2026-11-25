// 挂载身份边界 harness；Python 抽取生产函数，桩只替换外部观测与清理动作。
// 验证存储别名保留、身份格式往返、旧进程拒绝清理、清理顺序与父树可达性。
// 故障变体分别去掉别名保留与可达父链校验，验证边界用例确实能检出回归。

#![allow(dead_code, unused_imports, unused_variables, unused_macros)]

use std::sync::Mutex;

// ---- 真实 mountinfo 解析（仅类型与 parse_entry/unescape_field 被抽取注入） ----
mod platform {
    pub mod mountinfo {
        // 数据类型的副本（非匹配实现）：字段与 src/platform/mountinfo.rs 一致。
        pub struct MountInfoEntry<'a> {
            pub mount_id: u64,
            pub parent_mount_id: u64,
            pub dev: &'a str,
            pub root: &'a str,
            pub target: &'a str,
            pub fs_type: &'a str,
            pub source: &'a str,
        }

        // __INJECT_MOUNTINFO__
    }

    pub mod paths {
        // __INJECT_PATHS__

        pub fn storage_alias_roots_for_user(_user_id: i32) -> Vec<String> {
            vec!["/storage/emulated/0".to_string()]
        }
    }

    pub fn user_id_from_uid(uid: i32) -> i32 {
        if uid >= 0 { uid / 100000 } else { 0 }
    }

    pub mod module_paths {
        // 最小桩：decode 只关心"目标是否安全"，这里一律放行。
        pub fn is_safe_mount_target(_p: &str) -> bool {
            true
        }

        // 最小桩：clear_previous_mounts 依赖"目标已按深度降序排列"。
        // 仅做长度降序去重以模拟真实 normalize_mount_targets 的排序方向，
        // 不复制任何挂载点匹配逻辑。
        pub fn normalize_mount_targets(targets: &[String]) -> Vec<String> {
            let mut v: Vec<String> = targets.to_vec();
            v.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| b.cmp(a)));
            v.dedup();
            v
        }
    }
}

// ---- 归属判定桩：capture 只需"最顶层是不是本模块挂载"。 ----
// 真实实现看挂载源与 root 沙箱路径；这里仅按抽取用例里使用的挂载源前缀近似。
mod module_mount_source {
    pub struct ModuleMount {
        pub mount_id: u64,
        pub source: String,
        pub mount_point: String,
        pub is_sandbox_root: bool,
    }

    pub fn is_module_redirect_mount(source: &str, _root: &str, _target: &str, _pkg: &str) -> bool {
        source.starts_with("srx_fuse")
    }

    // 每次只暴露可达栈顶；摘除后下一层才会出现。
    pub fn app_redirect_mounts_in(_pid: i32, _package_name: &str) -> Option<Vec<ModuleMount>> {
        if crate::UNRECORDED_READ_FAILED.load(std::sync::atomic::Ordering::Relaxed) { return None; }
        let remaining = crate::UNRECORDED_LAYERS.load(std::sync::atomic::Ordering::Relaxed);
        Some(if remaining == 0 { Vec::new() } else {
            vec![ModuleMount { mount_id: remaining as u64, source: "srx_fuse".into(), mount_point: "/storage/emulated/0".into(), is_sandbox_root: true }]
        })
    }
}

// ---- clear_previous_mounts 的 mount_identity::load 桩：无账本。 ----
// topmost_live_mount 桩返回 None：本 harness 只验证身份/语法口径与摘除顺序，
// "热重载保留重定向根"的判定由 reload_keep_redirect_root harness 单独执行真实实现，
// 这里让保留分支恒不触发，避免两处判据互相掩盖。
mod mount_identity {
    pub fn namespace_identity(_: i32) -> Option<crate::NamespaceIdentity> { Some(crate::NamespaceIdentity { dev: 1, ino: 1 }) }
    pub fn load(_pkg: &str, _pid: i32) -> Option<crate::MountLedger> {
        crate::CLEANUP_LEDGER.lock().unwrap().clone()
    }

    pub fn topmost_live_mount(_pid: i32, _target: &str) -> Option<crate::LiveMount> {
        None
    }

    // 清理路径会连带移除同 pid 的遗留登记；桩里不需要实际状态，只提供签名。
    pub fn remove(_pkg: &str, _pid: i32) {}
}

// ---- fs 桩：返回内存里固定的 mountinfo，代替真实 /proc/<pid>/mountinfo。 ----
mod __harness_fs {
    use super::MOUNTINFO;
    pub fn read_to_string(_p: impl AsRef<std::path::Path>) -> Result<String, std::io::Error> {
        Ok(MOUNTINFO.lock().unwrap().clone())
    }
}

// ---- log 宏桩：clear_previous_mounts 用 warn!/info! 记录，这里丢弃。 ----
// 抽取函数里原本是 `log::warn!` / `log::info!`，由 Python 在注入前替换成裸宏调用。
#[macro_use]
mod log {
    macro_rules! warn {
        ($($arg:tt)*) => {{
            let _ = format_args!($($arg)*);
        }};
    }

    macro_rules! info {
        ($($arg:tt)*) => {{
            let _ = format_args!($($arg)*);
        }};
    }
}

// ---- 内存状态 ----
static MOUNTINFO: Mutex<String> = Mutex::new(String::new());
static CLEARED_TARGETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static READ_TARGETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static CLEANUP_LEDGER: Mutex<Option<MountLedger>> = Mutex::new(None);
static TARGET_CURRENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static UNRECORDED_LAYERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static UNRECORDED_READ_FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// ---- 类型（数据定义副本，非匹配实现） ----
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamespaceIdentity {
    pub dev: u64,
    pub ino: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMount {
    pub mount_id: u64,
    pub parent_mount_id: u64,
    pub dev: String,
    pub source: String,
    pub fs_type: String,
    pub root: String,
}

#[derive(Clone, Debug)]
pub struct MountIdentity {
    pub mount_point: String,
    pub mount_id: u64,
    pub dev: String,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct MountLedger {
    pub package_name: String,
    pub target_pid: i32,
    pub target_start_time: u64,
    pub namespace: NamespaceIdentity,
    pub generation: u64,
    pub mounts: Vec<MountIdentity>,
    pub detach_attempts: u32,
}

impl MountLedger {
    pub fn target_is_current(&self) -> bool { TARGET_CURRENT.load(std::sync::atomic::Ordering::Relaxed) }
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

    pub fn record_mounts(&mut self, mounts: Vec<MountIdentity>) {
        self.generation = self.generation.saturating_add(1);
        self.mounts = mounts;
        self.detach_attempts = 0;
    }

    pub fn clear_mounts(&mut self) {
        self.mounts.clear();
        self.detach_attempts = 0;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountVerdict {
    Owned(LiveMount),
    Detached,
    Superseded(LiveMount),
    StaleNamespace,
}

const IDENTITY_SCHEMA_VERSION: u32 = 2;

// ---- clear_previous_mounts 用的桩与类型 ----
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MountOperation {
    Reload,
    Apply,
    Disable,
}

pub struct MountRequest {
    pub operation: MountOperation,
    pub package_name: String,
    pub pid: i32,
    pub uid: i32,
    pub redirect_target: String,
    pub path_mappings: Vec<String>,
}

pub struct MountForkPlan {
    pub state_path: String,
    pub overlay_targets: Vec<String>,
    pub scoped_fuse_roots: Vec<String>,
}

// 保留判定的桩：真实实现见 reload_keep_redirect_root harness；这里恒 false，
// 让 clear_previous_mounts 在本 harness 里始终走"摘除"分支。
fn should_keep_reload_redirect_root(
    _target: &str,
    _request: &MountRequest,
    _scoped_fuse_roots: &[String],
    _live_layer: Option<(&str, &str)>,
) -> bool {
    false
}

#[derive(PartialEq, Debug)]
pub enum ClearOutcome {
    Cleared,
    Unverified,
}

impl ClearOutcome {
    fn is_cleared(&self) -> bool {
        matches!(self, Self::Cleared)
    }
}

fn read_fuse_children(_state_path: &str) -> Vec<String> {
    Vec::new()
}

fn read_mount_targets(_state_path: &str) -> Vec<String> {
    READ_TARGETS.lock().unwrap().clone()
}

fn clear_mount_target_stack_verified(target: &str, _ledger: Option<&MountLedger>, _uid: i32) -> bool {
    // 只记录被要求清理的目标及其顺序，不真正 umount。
    CLEARED_TARGETS.lock().unwrap().push(target.to_string());
    true
}

fn terminate_recorded_fuse_child(_child: &str) -> bool {
    true
}

fn last_errno() -> i32 {
    0
}

fn errno_text(_errno: i32) -> &'static str {
    "harness"
}

// ---- 空账本兜底清理的 umount 链路桩：harness 不真正摘挂载，只验证路径与错误分类。 ----
use std::ffi::CString;

const MNT_DETACH: i32 = 2;

mod libc {
    pub const EINVAL: i32 = 22;
    pub const ENOENT: i32 = 2;
}

unsafe fn umount2(_target: *const std::ffi::c_char, _flags: i32) -> i32 {
    let _ = UNRECORDED_LAYERS.fetch_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed, |count| count.checked_sub(1));
    0
}

// 反向验证用：别名折叠桩（把 /data/media 折成 /storage/emulated），不区分大小写以外的任何语义。
// 仅用于 Python 把抽取函数里的 paths::normalize_syntax 替换成本函数后，证明 harness 会失败。
fn __alias_fold(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("/data/media/") {
        return format!("/storage/emulated/{rest}");
    }
    path.to_string()
}

// ---- 让抽取函数能解析到桩模块 ----
use crate::platform::mountinfo;
use crate::platform::paths;
use crate::platform::module_paths;
use crate::module_mount_source::is_module_redirect_mount;
use crate::__harness_fs as fs;

// ---- 抽取的真实函数（mount_ledger / mount_identity / daemon_mount 三处） ----
// __INJECT_ALL__

// ===================== 测试驱动 =====================

fn set_mountinfo(s: &str) {
    // quality-allow(chinese-language): 解引用赋值是测试桩代码，不是英文自然语言。
    *MOUNTINFO.lock().unwrap() = s.to_string();
}

fn mk_live(mount_id: u64, source: &str, root: &str) -> LiveMount {
    LiveMount {
        mount_id,
        parent_mount_id: 0,
        dev: "0:1".to_string(),
        source: source.to_string(),
        fs_type: "fuse.srx".to_string(),
        root: root.to_string(),
    }
}

fn main() {
    let mut failures: Vec<&'static str> = Vec::new();

    // --- 用例 1：只有后端 /data/media 挂载时，主入口 /storage/emulated 不可命中 ---
    set_mountinfo(
        "100 99 0:1 /Download/SrtProbe /data/media/0/Download/SrtProbe - fuse.srx srx_fuse_probe\n",
    );
    let storage_hits = live_mounts_at(0, "/storage/emulated/0/Download/SrtProbe");
    if !storage_hits.is_empty() {
        println!(
            "FAIL storage_not_hit: 主入口不应命中后端挂载，实际命中 {} 条",
            storage_hits.len()
        );
        failures.push("storage_not_hit");
    } else {
        println!("PASS storage_not_hit: 仅后端挂载时主入口未命中");
    }
    let backend_hits = live_mounts_at(0, "/data/media/0/Download/SrtProbe");
    if backend_hits.len() != 1 || backend_hits[0].mount_id != 100 {
        println!("FAIL backend_hit: 后端挂载应被自身别名命中");
        failures.push("backend_hit");
    } else {
        println!("PASS backend_hit: 后端挂载按自身别名命中 id=100");
    }

    // --- 用例 2：混合别名各取各自 ID，不互相串 ---
    set_mountinfo(
        "100 99 0:1 /Download/QQ /data/media/0/Download/QQ - fuse.srx srx_fuse_qq\n\
         101 99 0:1 /Download/QQ /storage/emulated/0/Download/QQ - fuse.srx srx_fuse_qq\n",
    );
    let data_qq = live_mounts_at(0, "/data/media/0/Download/QQ");
    let storage_qq = live_mounts_at(0, "/storage/emulated/0/Download/QQ");
    if data_qq.len() != 1 || data_qq[0].mount_id != 100 {
        println!("FAIL mixed_alias_data: 期望仅命中 data/media id=100，实际 {:?}", data_qq);
        failures.push("mixed_alias_data");
    } else {
        println!("PASS mixed_alias_data: data/media 命中各自 id=100");
    }
    if storage_qq.len() != 1 || storage_qq[0].mount_id != 101 {
        println!(
            "FAIL mixed_alias_storage: 期望仅命中 storage/emulated id=101，实际 {:?}",
            storage_qq
        );
        failures.push("mixed_alias_storage");
    } else {
        println!("PASS mixed_alias_storage: storage/emulated 命中各自 id=101");
    }

    // --- 用例 3：大小写不混同 ---
    set_mountinfo(
        "210 99 0:1 /Download/SrtProbe /data/media/0/Download/SrtProbe - fuse.srx srx_fuse_sp\n\
         211 99 0:1 /Download/srtprobe /data/media/0/Download/srtprobe - fuse.srx srx_fuse_sp\n",
    );
    let upper = live_mounts_at(0, "/data/media/0/Download/SrtProbe");
    let lower = live_mounts_at(0, "/data/media/0/Download/srtprobe");
    if upper.len() != 1 || upper[0].mount_id != 210 {
        println!("FAIL case_upper: 大写 S 应只命中 id=210，实际 {:?}", upper);
        failures.push("case_upper");
    } else {
        println!("PASS case_upper: 大写 S 命中 id=210");
    }
    if lower.len() != 1 || lower[0].mount_id != 211 {
        println!("FAIL case_lower: 小写 s 应只命中 id=211，实际 {:?}", lower);
        failures.push("case_lower");
    } else {
        println!("PASS case_lower: 小写 s 命中 id=211");
    }
    // 主入口形式（大写 S）不能命中大小写不同的后端挂载
    let storage_case = live_mounts_at(0, "/storage/emulated/0/Download/SrtProbe");
    if !storage_case.is_empty() {
        println!("FAIL case_storage_not_hit: 主入口不应命中大小写不同的后端挂载");
        failures.push("case_storage_not_hit");
    } else {
        println!("PASS case_storage_not_hit: 主入口未命中大小写不同的后端挂载");
    }

    // --- 用例 4：capture 保留存储别名（不折叠成 /storage/emulated） ---
    set_mountinfo(
        "300 99 0:1 /Download/SrtProbe /data/media/0/Download/SrtProbe - fuse.srx srx_fuse_probe\n",
    );
    match capture_explicit_mount_identity(0, "/data/media/0/Download/SrtProbe") {
        Some(identity) if identity.mount_point == "/data/media/0/Download/SrtProbe"
            && identity.mount_id == 300 =>
        {
            println!(
                "PASS capture_preserves_alias: 落盘 mount_point={} 保留后端别名",
                identity.mount_point
            );
        }
        other => {
            println!("FAIL capture_preserves_alias: 实际 {:?}", other);
            failures.push("capture_preserves_alias");
        }
    }
    set_mountinfo(
        "301 99 0:1 /Download/SrtProbe /storage/emulated/0/Download/SrtProbe - fuse.srx srx_fuse_probe\n",
    );
    match capture_explicit_mount_identity(0, "/storage/emulated/0/Download/SrtProbe") {
        Some(identity) if identity.mount_point == "/storage/emulated/0/Download/SrtProbe" => {
            println!(
                "PASS capture_preserves_storage_alias: 落盘 mount_point={}",
                identity.mount_point
            );
        }
        other => {
            println!("FAIL capture_preserves_storage_alias: 实际 {:?}", other);
            failures.push("capture_preserves_storage_alias");
        }
    }

    // --- 用例 5：classify 用 normalize_syntax（别名保留 + 大小写敏感） ---
    let ns = NamespaceIdentity { dev: 1, ino: 1 };
    let ledger_backend_record = MountLedger {
        package_name: "pkg".to_string(),
        target_pid: 1,
        target_start_time: 1,
        namespace: ns,
        generation: 1,
        mounts: vec![MountIdentity {
            mount_point: "/data/media/0/Download/SrtProbe".to_string(),
            mount_id: 30,
            dev: "0:1".to_string(),
            source: "srx_fuse".to_string(),
        }],
        detach_attempts: 0,
    };
    // 主入口查询：账本记的是后端别名，live 是后端挂载；修复后必须 Detached（不可误 Own 后端）。
    let verdict = classify_mount(
        &ledger_backend_record,
        "/storage/emulated/0/Download/SrtProbe",
        Some(&mk_live(30, "srx_fuse", "/Download/SrtProbe")),
        Some(ns),
        true,
    );
    if verdict != MountVerdict::Detached {
        println!("FAIL classify_main_not_own_backend: 实际 {:?}", verdict);
        failures.push("classify_main_not_own_backend");
    } else {
        println!("PASS classify_main_not_own_backend: 主入口查询返回 Detached");
    }
    // 正向 Owned：账本与主入口、live 完全一致。
    let ledger_main_record = MountLedger {
        package_name: "pkg".to_string(),
        target_pid: 1,
        target_start_time: 1,
        namespace: ns,
        generation: 1,
        mounts: vec![MountIdentity {
            mount_point: "/storage/emulated/0/Download/SrtProbe".to_string(),
            mount_id: 5,
            dev: "0:1".to_string(),
            source: "srx_fuse".to_string(),
        }],
        detach_attempts: 0,
    };
    let verdict_owned = classify_mount(
        &ledger_main_record,
        "/storage/emulated/0/Download/SrtProbe",
        Some(&mk_live(5, "srx_fuse", "/Download/SrtProbe")),
        Some(ns),
        true,
    );
    if verdict_owned != MountVerdict::Owned(mk_live(5, "srx_fuse", "/Download/SrtProbe")) {
        println!("FAIL classify_owned: 实际 {:?}", verdict_owned);
        failures.push("classify_owned");
    } else {
        println!("PASS classify_owned: 主入口命中返回 Owned");
    }
    // 大小写不混同：账本记大写 S，live 是小写 s（不同 mount_id）→ Superseded。
    let verdict_case = classify_mount(
        &ledger_backend_record,
        "/data/media/0/Download/SrtProbe",
        Some(&mk_live(31, "srx_fuse", "/Download/srtprobe")),
        Some(ns),
        true,
    );
    if verdict_case != MountVerdict::Superseded(mk_live(31, "srx_fuse", "/Download/srtprobe")) {
        println!("FAIL classify_case_sensitive: 实际 {:?}", verdict_case);
        failures.push("classify_case_sensitive");
    } else {
        println!("PASS classify_case_sensitive: 大小写不同视为不同挂载 Superseded");
    }

    // --- 用例 6：schema 旧版拒绝 + 当前版本可用 ---
    let v1 = "schema=1\npackage=p\ntarget_pid=1\ntarget_start_time=1\nnamespace=1:1\ngeneration=0\ndetach_attempts=0\nmount=5\tsrx_fuse\t/data/media/0/Download/SrtProbe\n";
    if decode(v1).is_some() {
        println!("FAIL schema_v1_rejected: 旧 schema 账本不应被接受");
        failures.push("schema_v1_rejected");
    } else {
        println!("PASS schema_v1_rejected: schema=1 被拒绝");
    }
    let mut ledger = MountLedger::new("pkg", 1, 1, ns);
    ledger.record_mounts(vec![MountIdentity {
        mount_point: "/data/media/0/Download/SrtProbe".to_string(),
        mount_id: 30,
        dev: "0:1".to_string(),
        source: "srx_fuse".to_string(),
    }]);
    let encoded = encode(&ledger);
    if !encoded.contains("schema=3") {
        println!("FAIL schema_v3_written: encode 应写 schema=3");
        failures.push("schema_v3_written");
    } else if let Some(decoded) = decode(&encoded) {
        // 往返必须逐字段相等。encode/decode 字段顺序漂移时 decode 会因缺字段把整条
        // mount 行丢弃，账本"存在但 mounts 为空"——归属判定与 detach 记账同时失效，
        // 真机表现为重挂请求回 completed 而运行中应用一层未换。只断言 is_some() 抓不到。
        let roundtrip_ok = decoded.mounts.len() == 1
            && decoded.mounts[0].mount_id == 30
            && decoded.mounts[0].dev == "0:1"
            && decoded.mounts[0].source == "srx_fuse"
            && decoded.mounts[0].mount_point == "/data/media/0/Download/SrtProbe";
        if !roundtrip_ok {
            println!("FAIL ledger_roundtrip: 实际 {:?}", decoded.mounts);
            failures.push("ledger_roundtrip");
        } else {
            println!(
                "PASS schema_v3_roundtrip: encode 写 schema=3 且 id/dev/source/挂载点往返一致"
            );
        }
    } else {
        println!("FAIL schema_v3_accepted: 当前 schema 账本应被接受");
        failures.push("schema_v3_accepted");
    }

    // --- 用例 7：cleanup 按已降序列表正向遍历（排序不反转） ---
    // quality-allow(chinese-language): 解引用赋值用于准备清理目标测试数据。
    *READ_TARGETS.lock().unwrap() = vec![
        "/storage/emulated/0/Download/A".to_string(),
        "/storage/emulated/0/Download/A/Sub".to_string(),
    ];
    CLEARED_TARGETS.lock().unwrap().clear();
    let request = MountRequest {
        operation: MountOperation::Reload,
        package_name: "pkg".to_string(),
        pid: 1,
        uid: 10000,
        redirect_target: String::new(),
        path_mappings: Vec::new(),
    };
    let plan = MountForkPlan {
        state_path: std::env::var("SRX_TEST_STATE_PATH").expect("需要隔离的状态文件路径"),
        overlay_targets: Vec::new(),
        scoped_fuse_roots: Vec::new(),
    };
    let outcome = clear_previous_mounts(&request, &plan);
    let cleared = CLEARED_TARGETS.lock().unwrap().clone();
    // normalize_mount_targets 按长度降序 => [Sub, A]；正向遍历应保持 [Sub, A]。
    // 若修复被回退成 .rev()，会变成 [A, Sub]。
    if outcome != ClearOutcome::Cleared {
        println!("FAIL cleanup_cleared: 期望 Cleared，实际 {:?}", outcome);
        failures.push("cleanup_cleared");
    } else if cleared != vec![
        "/storage/emulated/0/Download/A/Sub".to_string(),
        "/storage/emulated/0/Download/A".to_string(),
    ] {
        println!("FAIL cleanup_order: 期望最深优先 [Sub, A]，实际 {:?}", cleared);
        failures.push("cleanup_order");
    } else {
        println!("PASS cleanup_order: 正向遍历保持降序（最深优先）");
    }

    READ_TARGETS.lock().unwrap().clear();
    for (current, same_namespace) in [(false, true), (true, false), (true, true)] {
        let mut recorded = ledger_main_record.clone();
        if !same_namespace { recorded.namespace.ino = 2; }
        *CLEANUP_LEDGER.lock().unwrap() = Some(recorded);
        TARGET_CURRENT.store(current, std::sync::atomic::Ordering::Relaxed);
        CLEARED_TARGETS.lock().unwrap().clear();
        clear_previous_mounts(&request, &plan);
        assert_eq!(!CLEARED_TARGETS.lock().unwrap().is_empty(), current && same_namespace, "旧进程或旧 namespace 的账本不能授权清理");
    }
    *CLEANUP_LEDGER.lock().unwrap() = None;
    UNRECORDED_LAYERS.store(2, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(clear_previous_mounts(&request, &plan), ClearOutcome::Unverified, "摘掉可达层后新露出的残留不能被误报为已清空");
    assert_eq!(UNRECORDED_LAYERS.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(clear_previous_mounts(&request, &plan), ClearOutcome::Cleared);
    assert_eq!(UNRECORDED_LAYERS.load(std::sync::atomic::Ordering::Relaxed), 0);
    UNRECORDED_LAYERS.store(1, std::sync::atomic::Ordering::Relaxed);
    UNRECORDED_READ_FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(clear_previous_mounts(&request, &plan), ClearOutcome::Unverified, "读表失败不能授权清理或注入");
    assert_eq!(UNRECORDED_LAYERS.load(std::sync::atomic::Ordering::Relaxed), 1);
    UNRECORDED_READ_FAILED.store(false, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(clear_previous_mounts(&request, &plan), ClearOutcome::Cleared);

    // 父入口的新系统层会遮住旧树的子挂载，但 mountinfo 仍保留旧子挂载的记录。
    set_mountinfo(
        "1 0 254:1 / / rw - ext4 rootfs rw\n\
         10 1 0:84 / /storage/emulated rw - fuse /dev/fuse rw\n\
         11 10 254:1 /media/0/Android/data/test/sdcard /storage/emulated/0 rw - ext4 rootfs rw\n\
         12 10 0:85 / /storage/emulated rw - fuse /dev/fuse rw\n",
    );
    assert!(topmost_live_mount(0, "/storage/emulated/0").is_none(), "被祖先覆盖的子挂载不能证明当前视图有效");
    let (covering_point, covering) = try_visible_mount_at(0, "/storage/emulated/0").unwrap().unwrap();
    assert_eq!(covering_point, "/storage/emulated");
    assert_eq!((covering.mount_id, covering.dev.as_str(), covering.root.as_str()), (12, "0:85", "/0"));
    assert_eq!(live_mounts_at(0, "/storage/emulated").len(), 2, "同路径叠层仍须全部保留供清理");

    // 未知父树与可见的新子层都按实际父链解析，不能重新认领旧树的子挂载。
    set_mountinfo(
        "1 0 254:1 / / rw - ext4 rootfs rw\n\
         10 1 0:84 / /storage/emulated rw - fuse /dev/fuse rw\n\
         11 10 254:1 /sandbox /storage/emulated/0 rw - ext4 rootfs rw\n\
         12 10 0:85 / /storage/emulated rw - tmpfs foreign rw\n\
         3 12 0:86 /live /storage/emulated/0 rw - fuse /dev/fuse rw\n",
    );
    assert_eq!(topmost_live_mount(0, "/storage/emulated/0").unwrap().mount_id, 3);
    assert_eq!(live_mounts_at(0, "/storage/emulated/0").len(), 1);
    let (covering_point, covering) = try_visible_mount_at(0, "/storage/emulated/1/Download").unwrap().unwrap();
    assert_eq!(covering_point, "/storage/emulated");
    assert_eq!((covering.mount_id, covering.fs_type.as_str(), covering.root.as_str()), (12, "tmpfs", "/1/Download"));

    let entries = "1 0 254:1 / / rw - ext4 rootfs rw\n\
        7 1 0:7 / /storage rw - tmpfs storage rw\n\
        8 7 0:8 / /storage/emulated rw - fuse /dev/fuse rw\n\
        9 8 0:9 / /storage/emulated/0 rw - ext4 rootfs rw\n\
        2 7 0:2 / /storage rw - tmpfs cover rw\n\
        3 2 0:3 / /storage/with\\040space rw - tmpfs escaped rw\n"
        .lines().filter_map(platform::mountinfo::parse_entry).collect::<Vec<_>>();
    for (path, expected) in [
        ("/", Some(1)),
        ("/storage/emulated/0/Download", Some(2)),
        ("/storage/with space/file", Some(3)),
        ("/storage/with spaces/file", Some(2)),
        ("/storages/emulated/0", Some(1)),
        ("storage/emulated/0", None),
        ("", None),
    ] {
        assert_eq!(platform::mountinfo::visible_mount_at(&entries, path).map(|entry| entry.mount_id), expected, "可达挂载路径边界: {path}");
    }
    assert!(platform::mountinfo::mount_is_descendant(&entries, 3, 1));
    assert!(!platform::mountinfo::mount_is_descendant(&entries, 99, 1));
    assert!(!platform::mountinfo::mount_is_descendant(&entries, 1, 99));
    let cycle = "4 5 0:4 / /a rw - tmpfs a rw\n5 4 0:5 / /b rw - tmpfs b rw"
        .lines().filter_map(platform::mountinfo::parse_entry).collect::<Vec<_>>();
    assert!(!platform::mountinfo::mount_is_descendant(&cycle, 4, 1), "环形父链必须有界退出");
    set_mountinfo("8 7 0:8 / / rw - tmpfs lower rw\n3 8 0:3 / / rw - tmpfs upper rw\n");
    assert_eq!(live_mounts_at(0, "/").len(), 2);
    let (visible_point, visible) = try_visible_mount_at(0, "/child").unwrap().unwrap();
    assert_eq!(visible_point, "/");
    assert_eq!(visible.root, "/child");

    if failures.is_empty() {
        println!("ALL MOUNT IDENTITY SYNTAX CASES PASSED");
    } else {
        println!("MOUNT IDENTITY SYNTAX CASE(S) FAILED: {:?}", failures);
        std::process::exit(1);
    }
}
