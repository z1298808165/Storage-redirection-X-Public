//! 本模块挂载源的识别，以及"重定向挂载是否已在当前命名空间生效"的判定。
//!
//! 这个文件同时被 zygisk 模块（`srx_core`）和 `srx_daemon` 两个目标编译，因此只放两个目标
//! 都需要的极小逻辑，不引入账本或状态落盘。
//!
//! 挂载源的识别必须单点定义：测试流也按同一套前缀区分 scoped 与宿主挂载，两处若各写一份，
//! 改前缀时会漏改一处，导致归属判定和用例断言给出互相矛盾的结论。
//!
//! 这里有两类用途，判据宽严不同，不要混用：
//!
//! - **判定"我的重定向是否已生效"**（[`app_redirect_mounts_in`]）要宽：FUSE 后端的源带
//!   `srx_fuse_*` 前缀，命名空间后端的 bind 记录则要靠 `root` 里的沙箱路径识别。少认一条，
//!   对应后端的应用就会在 `specialize_post` 里空等，甚至被 AMS 判进程启动超时而杀掉。
//! - **判定"这条挂载能否安全摘除"**（[`is_module_redirect_mount`]）要精确：可以按 `root` 里的
//!   沙箱路径识别，但标记必须是 `<包名>/sdcard` 而不是裸 `<包名>`——后者与系统自己的
//!   `Android/data/<包名>` 挂载同形，按它摘除会误伤系统挂载。
//!
//! [`is_module_mount_source`] 只是上述组合判据里的一条证据（挂载源前缀），单独使用会漏判
//! 所有 bind 层：bind 会把底层文件系统的源继承下来，真机上本模块每一层的 `source` 都是
//! MediaProvider FUSE 的 `/dev/fuse`，前缀根本匹配不上。见 [`is_module_redirect_mount`]。

use crate::platform::{module_paths, mountinfo, paths};
use std::fs;

/// 本模块挂载源使用的前缀。
///
/// `srx_fuse_redirect[<pid>]` 是按应用启动的 scoped FUSE 会话；`srx_fuse_host[<pid>]` 是
/// 共享 FUSE daemon 的宿主会话。两种来源都由本模块创建，恢复流程可以放心摘除；
/// 其它前缀（例如系统媒体 FUSE 的 `/dev/fuse`）一律视为外部挂载。
const MODULE_MOUNT_SOURCE_PREFIXES: [&str; 2] = ["srx_fuse_redirect", "srx_fuse_host"];

/// 本模块创建的一条挂载记录，带挂载点。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleMount {
    pub mount_id: u64,
    pub source: String,
    pub mount_point: String,
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

/// 判断挂载源是否指向模块自己的临时锚点目录。
///
/// 命名空间后端的重定向会把真实存储根先绑到模块私有的临时锚点上
/// （`/data/adb/modules/storage.redirect.x/tmp/real_storage/<user>/...`），再在它之上建立
/// 应用可见的绑定。`mountinfo` 里这些记录的源就是该锚点路径，不带 `srx_fuse_*` 前缀。
///
/// 它是"底层树已就位"的信号，可与 [`is_namespace_redirect_mount`] 一起用来判定命名空间
/// 后端是否已经生效。
///
/// **注意**：bind 会把底层文件系统的源继承下来，真机上锚点绑定的 `source` 同样是
/// `/dev/fuse`，这条判据因此匹配不到。要认出锚点必须同时看**挂载点**，见
/// [`is_module_anchor_mount_target`]。
pub fn is_module_anchor_mount_source(source: &str) -> bool {
    source.starts_with(module_paths::REAL_STORAGE_TMP_PREFIX)
}

/// 判断挂载点是否就是模块自己的临时锚点目录。
///
/// 锚点目录是模块私有路径（`/data/adb/modules/storage.redirect.x/tmp/real_storage/...`），
/// 系统绝不会往那里挂载，因此按挂载点识别比按挂载源可靠：真机上锚点绑定的 `source`
/// 被 bind 继承成 MediaProvider FUSE 的 `/dev/fuse`，`root` 是 `/0`（锚点绑的是
/// `/storage/emulated/0`，它在系统 FUSE 里的相对路径就是 `0`），两者都不带模块特征。
///
/// 漏掉这条会让仅映射模式的应用看不到任何模块挂载：它只建立锚点与映射，没有存储根
/// 重定向，应用侧因此等不到挂载确认而被 AMS 判启动超时。
pub fn is_module_anchor_mount_target(target: &str) -> bool {
    target.starts_with(module_paths::REAL_STORAGE_TMP_PREFIX)
}

/// 判断某条挂载记录是否为本模块建立的命名空间后端重定向。
///
/// 命名空间后端用 `MS_BIND` 把应用沙箱子树绑到公共视图上，`mountinfo` 里这条记录的形态是：
///
/// ```text
/// 5419 5404 254:60 /media/0/Android/data/<包名>/sdcard /storage/emulated/0 - f2fs /dev/block/dm-60
///                  ^^^ root 字段带沙箱路径        ^^^ 挂载点          ^^^ source 是块设备
/// ```
///
/// 沙箱路径落在 **`root`** 而不是 `source`，且 `source` 是块设备——所以按 source 匹配永远认
/// 不出来。但也不能只按 `root` 匹配：系统自己的媒体 FUSE 挂载形如
/// `254:60 /0/Android/data/<包名> /storage/emulated/0/Android/data/<包名> - fuse /dev/fuse`，
/// `root` 里同样含 `Android/data/<包名>`。两者靠**文件系统类型**分开：模块的 bind 保留底层
/// `f2fs` 等真实类型，系统媒体 FUSE 的类型是 `fuse`。
pub fn is_namespace_redirect_mount(
    entry: &mountinfo::MountInfoEntry<'_>,
    package_name: &str,
) -> bool {
    if package_name.is_empty() || entry.fs_type.starts_with("fuse") {
        return false;
    }
    let root = mountinfo::unescape_field(entry.root);
    ["Android/data/", "Android/media/", "Android/obb/"]
        .iter()
        .any(|prefix| root.contains(&format!("{prefix}{package_name}")))
}

/// 判断挂载记录的 `root` 是否指向本应用沙箱子树。
///
/// 模块的重定向目标固定落在应用专属目录下的 `sdcard` 子目录（`Android/data|media|obb/<包名>/sdcard`），
/// 所有重定向挂载都是从这个子树 bind 出来的，因此 `root` 里必然带 `<包名>/sdcard` 这一段。
///
/// 这是**唯一在 bind 之后仍然保留的归属证据**：`source` 会被 bind 原样继承（真机上是
/// MediaProvider FUSE 的 `/dev/fuse`），`fs_type` 也随之继承（可能是 `fuse`，也可能是底层
/// 分区的 `f2fs`），只有 `root` 是模块自己选的沙箱路径。
///
/// 与 [`is_namespace_redirect_mount`] 的分工：那条判据要求 `fs_type` 非 `fuse`，用于识别
/// 「底层树已就位」；这条判据**不限制文件系统类型**，用于识别「这一层是本模块挂上去的」。
/// 两者不可互相替代——真机上同一轮挂载会同时产出 `f2fs`（从 `/data/media` 绑）与 `fuse`
/// （从 `/storage/emulated` 绑）两种形态，只认其中一种就会漏判一半的层。
pub fn is_module_sandbox_root(root: &str, package_name: &str) -> bool {
    if package_name.is_empty() {
        return false;
    }
    ["Android/data/", "Android/media/", "Android/obb/"]
        .iter()
        .any(|prefix| root.contains(&format!("{prefix}{package_name}/sdcard")))
}

/// 去掉 `/media` 前缀后的 `root`，以及它相对存储根的首段（用户号）。
///
/// `root` 有两种等价写法：经系统 FUSE 视图时是 `/<user>/...`，经 `/data` 分区视图时是
/// `/media/<user>/...`。两者表示同一棵存储树，判定前先归一，避免漏判一半。
fn storage_root_tail(root: &str) -> Option<(&str, &str)> {
    let rest = root.strip_prefix("/media").unwrap_or(root);
    let rest = rest.strip_prefix('/')?;
    let (user, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if user.is_empty() || !user.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((user, tail))
}

/// 判断 `root` 是否恰好是系统自己挂的**应用专属目录**。
///
/// 系统 MediaProvider 的媒体 FUSE 把应用专属目录挂成
/// `254:60 /0/Android/data/<包名> /storage/emulated/0/Android/data/<包名> - fuse /dev/fuse`，
/// 即 `root` 恰好止于 `Android/{data,media,obb}/<一段包名>`。
///
/// 模块的层永远不会是这个形态：存储根重定向与映射都从 `sdcard` 子树或映射目标 bind，
/// `root` 一定更深（带 `/sdcard` 或别的路径段）。**这是模块层与系统层唯一的分界**，
/// 因此判定顺序上必须先把这一形态排除掉。
fn root_is_app_private_directory(root: &str) -> bool {
    let Some((_, tail)) = storage_root_tail(root) else {
        return false;
    };
    ["Android/data/", "Android/media/", "Android/obb/"]
        .iter()
        .any(|prefix| {
            tail.strip_prefix(prefix)
                .is_some_and(|name| !name.is_empty() && !name.contains('/'))
        })
}

/// 判断 `root` 是否落在 emulated 存储视图内（`/<user>/...` 或 `/media/<user>/...`）。
///
/// 模块的所有重定向挂载都从存储树 bind，`root` 必然落在这两个视图里；系统在存储树上的
/// 挂载只有存储根自身与应用专属目录两种形态，前者 `root` 是 `/<user>` 或 `/`，会被
/// [`root_is_app_private_directory`] 之外的分支排除。
fn root_is_emulated_storage_view(root: &str) -> bool {
    storage_root_tail(root).is_some_and(|(_, tail)| !tail.is_empty())
}

/// 判断一条挂载记录是否由本模块为本应用建立，即「这一层能否安全摘除」。
///
/// 判据按证据强度从强到弱排列，命中任意一条即为本模块的层：
///
/// 1. 挂载源带 `srx_fuse_*` 前缀（scoped FUSE 会话，`FSName` 成功进入 mountinfo 时的形态）；
/// 2. 挂载源或**挂载点**指向模块私有临时锚点（锚点自身的绑定，见
///    [`is_module_anchor_mount_target`]）；
/// 3. `root` 指向本应用沙箱子树（见 [`is_module_sandbox_root`]）——bind 之后仍然保留的证据；
/// 4. `root` 落在 emulated 存储视图内且不是系统自己挂的应用专属目录——覆盖路径映射从
///    「映射目标」bind 出来的层，那类 `root` 不带包名（真机上形如 `/0/Download/SrtMapOnlyMapped`）。
///
/// 第 2 条里的挂载点判定、以及第 3、4 条都是新增的：此前只按挂载源判断，而 bind 会把底层
/// 文件系统的源继承下来（真机上本模块每一层的 `source` 都是 MediaProvider FUSE 的
/// `/dev/fuse`），于是**每一层都被判成外部挂载而拒绝摘除**，重挂只能在旧层之上叠加。
///
/// 排除项只有一个：[`root_is_app_private_directory`] 描述的系统媒体 FUSE 形态。判定顺序上
/// 它排在最后两条之前——模块的层与它同形，只有「`root` 是否更深」这一点可分。
///
/// 参数用已反转义的字段而不是 `MountInfoEntry`：调用方一侧持有解析出的 `LiveMount`，
/// 另一侧持有原始条目，统一在字段层判定可以让两条路径共用同一份判据。
pub fn is_module_redirect_mount(
    source: &str,
    root: &str,
    target: &str,
    package_name: &str,
) -> bool {
    if is_module_mount_source(source)
        || is_module_anchor_mount_source(source)
        || is_module_anchor_mount_target(target)
    {
        return true;
    }
    if root_is_app_private_directory(root) {
        return false;
    }
    is_module_sandbox_root(root, package_name) || root_is_emulated_storage_view(root)
}

/// 列出目标命名空间内全部由本模块创建、或由本模块为本应用建立的挂载，按挂载 ID 升序。
///
/// 应用进程用它判定"我的重定向是否已经生效"，替代了过去的挂载状态标记文件：挂载是内核
/// 记录的事实，不需要任何组件额外写文件，也不会因为进程退出而留下残骸。`pid <= 0` 表示读取
/// 调用方自身的命名空间，即应用在 `specialize_post` 里查自己。
///
/// 三条来源都要认，缺一条就会让对应后端的应用空等：FUSE 后端的源带 `srx_fuse_*` 前缀、
/// 命名空间后端的 bind 记录靠 `root` 里的沙箱路径识别（见 [`is_namespace_redirect_mount`]）、
/// 临时锚点的 `real_storage` 绑定说明底层树已就位。另外路径映射的层 `root` 里没有包名
/// （从「映射目标」bind），由 [`is_module_redirect_mount`] 的第 4 条覆盖——漏掉它会让
/// 仅映射模式的应用永远等不到确认，被 AMS 判启动超时。
///
/// 返回的挂载 ID 集合是幂等比较用的判据：同一命名空间内重复调用结果不变，新增挂载会带来
/// 新 ID，据此可以等到挂载集合稳定后再认定生效。
pub fn app_redirect_mounts_in(pid: i32, package_name: &str) -> Vec<ModuleMount> {
    let path = if pid > 0 {
        format!("/proc/{pid}/mountinfo")
    } else {
        "/proc/self/mountinfo".to_string()
    };
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut mounts = content
        .lines()
        .filter_map(|line| {
            let entry = mountinfo::parse_entry(line)?;
            let source = mountinfo::unescape_field(entry.source);
            let root = mountinfo::unescape_field(entry.root);
            if !is_module_mount_source(&source)
                && !is_module_anchor_mount_source(&source)
                && !is_namespace_redirect_mount(&entry, package_name)
                && !is_module_redirect_mount(
                    &source,
                    &root,
                    &paths::normalize(&mountinfo::unescape_field(entry.target)),
                    package_name,
                )
            {
                return None;
            }
            Some(ModuleMount {
                mount_id: entry.mount_id,
                source,
                mount_point: paths::normalize(&mountinfo::unescape_field(entry.target)),
            })
        })
        .collect::<Vec<_>>();
    mounts.sort_by_key(|mount| mount.mount_id);
    mounts
}
