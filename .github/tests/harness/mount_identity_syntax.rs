// 挂载身份账本语法边界 harness 模板（由 Python 注入真实函数后 rustc 独立编译运行）。
//
// 与 should_passthrough_provider_allowed_parent_mkdir.rs 同一思路：这里不直接包含被测函数的实现，
// 而是从 src 用大括号计数抽出真实 fn 注入到下方占位符，从而编译并执行源码里实际的函数，
// 而不是一份抄写副本（抄写副本无法证明原函数有没有被改坏）。
//
// 桩只暴露被抽取函数用到的签名/全局状态，绝不复制"匹配实现"：
// - `__harness_fs::read_to_string` 返回 Python 在内存里设定的固定 mountinfo，代替真实 /proc 读取；
// - `platform::mountinfo` 与 `platform::paths::normalize_syntax` 是**真实**抽取的函数；
// - 其余依赖（module_mount_source / module_paths::normalize_mount_targets / clear_previous_mounts
//   内部的 umount 链路 / log / last_errno）用最小桩顶替。
//
// 被测修复点（来自主代理对 src 的修改）：
//   1. live_mounts_at / capture_mount_identity / classify_mount 改用 normalize_syntax（保留存储别名、
//      大小写敏感），不再把 /data/media 折叠成 /storage/emulated；
//   2. encode 写 schema=2，decode 拒绝旧 schema；
//   3. clear_previous_mounts 改为按"已降序的目标列表"正向遍历（不再 .rev()）。
//
// 反向验证：Python 在内存里把抽取函数里的 `paths::normalize_syntax` 替换成别名折叠桩
// `__alias_fold`，重新编译运行——此时"只有后端挂载时主入口不可命中"等用例应从通过变失败，
// 证明 harness 真在验证别名保留行为，而非空壳。

#![allow(dead_code, unused_imports, unused_variables, unused_macros)]

use std::sync::Mutex;

// ---- 真实 mountinfo 解析（仅类型与 parse_entry/unescape_field 被抽取注入） ----
mod platform {
    pub mod mountinfo {
        // 数据类型的副本（非匹配实现）：字段与 src/platform/mountinfo.rs 一致。
        pub struct MountInfoEntry<'a> {
            pub mount_id: u64,
            pub root: &'a str,
            pub target: &'a str,
            pub fs_type: &'a str,
            pub source: &'a str,
        }

        // __INJECT_MOUNTINFO__
    }

    pub mod paths {
        // __INJECT_PATHS__
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
    pub fn is_module_redirect_mount(source: &str, _root: &str, _target: &str, _pkg: &str) -> bool {
        source.starts_with("srx_fuse")
    }
}

// ---- clear_previous_mounts 的 mount_identity::load 桩：无账本。 ----
// topmost_live_mount 桩返回 None：本 harness 只验证身份/语法口径与摘除顺序，
// "热重载保留重定向根"的判定由 reload_keep_redirect_root harness 单独执行真实实现，
// 这里让保留分支恒不触发，避免两处判据互相掩盖。
mod mount_identity {
    pub fn load(_pkg: &str, _pid: i32) -> Option<crate::MountLedger> {
        None
    }

    pub fn topmost_live_mount(_pid: i32, _target: &str) -> Option<crate::LiveMount> {
        None
    }
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

// ---- 类型（数据定义副本，非匹配实现） ----
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NamespaceIdentity {
    pub dev: u64,
    pub ino: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveMount {
    pub mount_id: u64,
    pub source: String,
    pub fs_type: String,
    pub root: String,
}

#[derive(Clone, Debug)]
pub struct MountIdentity {
    pub mount_point: String,
    pub mount_id: u64,
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
    pub redirect_target: String,
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

fn clear_mount_target_stack_verified(target: &str, _ledger: Option<&MountLedger>) -> bool {
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
    match capture_mount_identity(0, "/data/media/0/Download/SrtProbe", "pkg") {
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
    match capture_mount_identity(0, "/storage/emulated/0/Download/SrtProbe", "pkg") {
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
        source: "srx_fuse".to_string(),
    }]);
    let encoded = encode(&ledger);
    if !encoded.contains("schema=2") {
        println!("FAIL schema_v2_written: encode 应写 schema=2");
        failures.push("schema_v2_written");
    } else if decode(&encoded).is_none() {
        println!("FAIL schema_v2_accepted: 当前 schema 账本应被接受");
        failures.push("schema_v2_accepted");
    } else {
        println!("PASS schema_v2: encode 写 schema=2 且 decode 接受");
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
        redirect_target: String::new(),
    };
    let plan = MountForkPlan {
        state_path: "/x".to_string(),
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

    if failures.is_empty() {
        println!("ALL MOUNT IDENTITY SYNTAX CASES PASSED");
    } else {
        println!("MOUNT IDENTITY SYNTAX CASE(S) FAILED: {:?}", failures);
        std::process::exit(1);
    }
}
