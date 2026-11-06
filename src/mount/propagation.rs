//! 挂载传播的最小隔离：只切断必须切断的关系，其余保留内核克隆态。
//!
//! 背景（真机 2026-11-02 实测，duchamp / A16）：
//!
//! 隔离传播的旧做法是 `mount --make-rprivate /`（整树私有化）。它把内核里既有的
//! slave 关系一并切断，于是应用命名空间的 `mountinfo` 里**一行 `master:` 传播标签都不剩**，
//! 而同一台设备上的普通应用有 111 行。这不是字符串泄漏，而是结构性差异：任何应用只要数一下
//! 自己挂载表里的传播标签，就能断定自己被单独关进了一个命名空间。实测普通的
//! `mount --make-rslave /` 也救不回来（全局命名空间自己的挂载是 slave 而非 shared，
//! 没有对等组可以充当 master），所以"先整树私有化再补标签"这条路不存在。
//!
//! 安全上真正需要的只有两条，其余隔离都是多余的：
//!
//! 1. **不外播**：本命名空间里新建的挂载不得出现在全局命名空间。外播只发生在
//!    `shared:` 的对等组上——把每一个 `shared:` 挂载改成 private 即可，不需要动 slave。
//! 2. **不受压**：存储别名子树不得接受入向传播。否则 MediaProvider 重启、切用户、
//!    vold 变更这类事件会沿 slave 关系在我们的视图层之上叠一层真实存储，应用随即读到
//!    真实数据（既是功能事故，也是最直接的暴露）。
//!
//! 因此这里只私有化「全部 shared 挂载」+「存储别名子树的当前服务挂载（递归）」，
//! 其余挂载保持克隆态，`master:` 标签原样留存。私有化后立即复核这两条目标；
//! 任一不成立就返回 `false`，由调用方回退到整树私有化——隐藏性降级，安全性不降级。

use crate::platform::errno::{last as last_errno, text as errno_text};
use crate::platform::{mountinfo, paths, proc_direct};
use libc::{MS_PRIVATE, MS_REC, c_ulong, mount};
use std::ffi::CString;

/// 单次读取 mountinfo 的上限：宿主层数多的设备实测数十 KB，512 KiB 留足余量。
const MOUNTINFO_READ_LIMIT: usize = 512 * 1024;

/// 为某个 Android 用户隔离存储别名的传播，保留其余挂载的传播标签。
///
/// 覆盖的是本次重定向可能触及的全部存储出口（主视图根 + 各家别名根）：只有这些子树
/// 里既有"我们会往上挂"的路径，也有"系统会往上挂"的路径。
pub fn isolate_storage_mounts(user_id: i32) -> bool {
    let mut roots = Vec::with_capacity(8);
    roots.push(paths::storage_user_root_for_user(user_id));
    roots.extend(paths::storage_alias_roots_for_user(user_id));
    let roots: Vec<String> = roots.into_iter().filter(|root| !root.is_empty()).collect();
    isolate_mounts_for_roots(&roots)
}

/// 为调用方给定的根路径集合隔离传播，并复核目标是否达成。
///
/// 返回 `false` 表示复核不通过（仍有 shared 挂载，或有根本的服务挂载仍带传播关系），
/// 调用方必须回退到整树私有化。
pub fn isolate_mounts_for_roots(roots: &[String]) -> bool {
    let Some(lines) = read_mountinfo_lines() else {
        log::warn!("mount propagation: mountinfo unreadable, isolation skipped");
        return false;
    };

    // 第一条：外播只走 shared 对等组，逐个取消共享即可，slave 关系保持不动。
    let mut shared_targets = Vec::new();
    for line in &lines {
        if !mountinfo::has_propagation_tag(line, "shared:") {
            continue;
        }
        let Some(entry) = mountinfo::parse_entry(line) else {
            continue;
        };
        shared_targets.push(mountinfo::unescape_field(entry.target));
    }
    let shared_count = shared_targets.len();
    for target in &shared_targets {
        make_private(target, false);
    }

    // 第二条：存储别名子树私有化，切断入向传播。
    //
    // 只处理「本身就是存储视图挂载」的层，且**非递归**：别名清单里有些路径在本命名空间
    // 根本不存在（如 `/mnt/runtime/*/emulated/<u>`），它们的最长前缀会一路退到 `/mnt`
    // 或 `/storage`——递归私有化这些祖先会把 `master:` 标签从整片无关子树上抹掉
    // （真机 2026-11-02 实测：`/mnt/vendor/*`、`/mnt/rescue`、`/mnt/vm` 等 13 条
    // 与存储无关的挂载一并丢标签，应用可见的传播标签从 111 掉到 92，其中 13 条是纯副作用）。
    // 祖先保持 slave 是无害的：入向事件只会叠在祖先对应的路径上，而我们真正要保护的是
    // 存储视图挂载**自身**这条路（见 [`verify_isolation`]），非递归私有化已经足够。
    let mut isolated = Vec::new();
    let mut skipped = 0usize;
    for root in roots {
        let Some(serving) = serving_mount_point(&lines, root) else {
            continue;
        };
        if !is_storage_view_mount(&serving) {
            // 该别名在本命名空间里没有独立挂载：需要保护的层不存在，跳过。
            skipped += 1;
            continue;
        }
        if isolated.iter().any(|done| done == &serving) {
            continue;
        }
        if make_private(&serving, false) {
            isolated.push(serving);
        }
    }
    log::info!(
        "mount propagation isolated roots={} shared={} subtrees={} skipped={}",
        roots.len(),
        shared_count,
        isolated.len(),
        skipped
    );

    verify_isolation(roots)
}

/// 该挂载点是否为「存储视图挂载」——即本模块会往上叠层的那一层。
///
/// 判据只看路径形态：`emulated` 段（`/storage/emulated*`、`/mnt/{user,installer,…}/<u>/emulated*`）
/// 或 `/data/media`。别名清单里不存在的路径最长前缀会退到 `/mnt`、`/storage` 这类
/// 通用祖先，它们不是存储视图，私有化只会伤及无关子树。
fn is_storage_view_mount(target: &str) -> bool {
    target.split('/').any(|segment| segment == "emulated")
        || target == "/data/media"
        || target.starts_with("/data/media/")
}

/// 系统存储视图**根层**（`…/emulated` 这一级）当前的超级块设备号集合。
///
/// 用途：死宿主层清理按 `dev ∈ known_module_fuse_devs()` 在整个命名空间里摘层，而本模块
/// 曾对系统视图做过 bind —— bind 继承的是系统 MediaProvider 的超级块，这个 dev 会随账本
/// 混进 known 集合。只按 "known 且非当前宿主" 判死层，就会把系统自己的 `…/emulated`
/// 层摘掉（真机表现：重定向应用的别名根层消失、只剩 `…/emulated/<user>`）。
///
/// 因此凡是**当前正服务**在根层上的超级块都必须在清理时受保护。只看根层不看用户层：
/// 用户层正是本模块要接管、要替换的地方。
pub fn system_view_root_devs(user_id: i32) -> std::collections::HashSet<String> {
    let mut root_paths = vec![
        "/storage/emulated".to_string(),
        format!("/mnt/user/{user_id}/emulated"),
        format!("/mnt/installer/{user_id}/emulated"),
        format!("/mnt/androidwritable/{user_id}/emulated"),
        format!("/mnt/pass_through/{user_id}/emulated"),
    ];
    for mode in ["default", "read", "write", "full"] {
        root_paths.push(format!("/mnt/runtime/{mode}/emulated"));
    }
    let mut devs = std::collections::HashSet::new();
    let Some(lines) = read_mountinfo_lines() else {
        return devs;
    };
    for line in &lines {
        let Some(entry) = mountinfo::parse_entry(line) else {
            continue;
        };
        let target = mountinfo::unescape_field(entry.target);
        if !root_paths.iter().any(|root| root == target.as_str()) {
            continue;
        }
        if !entry.dev.is_empty() {
            devs.insert(entry.dev.to_string());
        }
    }
    devs
}

/// 收集仍然违规的挂载点（空 = 达标；`None` = 挂载表读不到）。
///
/// 两项目标各对应一类违规：`shared:`（还有外播通道）、存储视图服务挂载上的
/// `master:`/`propagate_from:`（还接受入向事件）。只查服务挂载（以及服务挂载到根之间的
/// 各层）而不是沿途所有祖先：祖先（典型是 `/`）保持 slave 既安全也无害——入向事件沿祖先
/// 到达时，被复制出来的是挂在**祖先对应路径**上的一层，而我们的视图层在更深的挂载点上，
/// 仍然胜出；真正危险的是"根所在的这一层自己是 slave"，那会把真实存储直接叠到根的路径上。
fn isolation_violations(roots: &[String]) -> Option<Vec<String>> {
    let lines = read_mountinfo_lines()?;
    let mut violations = Vec::new();
    for line in &lines {
        if mountinfo::has_propagation_tag(line, "shared:")
            && let Some(entry) = mountinfo::parse_entry(line)
        {
            violations.push(mountinfo::unescape_field(entry.target));
        }
    }
    for root in roots {
        let Some(serving) = serving_mount_point(&lines, root) else {
            continue;
        };
        // 与本轮实际私有化的集合保持同一口径：别名在本命名空间不存在时（最长前缀退到
        // `/mnt`、`/storage` 这类通用祖先），那层不是存储视图、我们也没有动它，
        // 它的 `master:` 标签是原生态，不能算隔离失败。
        if !is_storage_view_mount(&serving) {
            continue;
        }
        for line in &lines {
            let Some(entry) = mountinfo::parse_entry(line) else {
                continue;
            };
            let target = mountinfo::unescape_field(entry.target);
            // 只看「服务挂载本身，以及从它到根之间的各层」：这些层是入向事件落到根路径上
            // 的必经通道。
            if !is_ancestor_or_self(&serving, &target) || !is_ancestor_or_self(&target, root) {
                continue;
            }
            // 被同挂载点上层遮蔽的旧层不参与判定：按路径的 `mount --make-private` 只能命中
            // 栈顶，改不动它（真机 2026-11-02 实测：MP 重启后旧 `/storage/emulated` 被新层
            // 盖住，它仍带 `master:`，复核因此永远判失败并把整棵树私有化）。它也无害——
            // 入向事件即便从它复制出一层，那层同样被栈顶遮蔽，落不到应用可见的路径上。
            if !is_topmost_at(lines.as_slice(), entry.mount_id, &target) {
                continue;
            }
            if mountinfo::has_propagation_tag(line, "master:")
                || mountinfo::has_propagation_tag(line, "propagate_from:")
            {
                violations.push(target);
            }
        }
    }
    violations.sort();
    violations.dedup();
    Some(violations)
}

/// 复核隔离目标；不达标时就地自愈一次，仍不达标才判失败。
///
/// 自愈这一步不是宽容，而是必须：MediaProvider 换代/切用户/`vold` 变更会在第一轮私有化
/// **之后**把新的挂载换到同一路径上（真机 2026-11-02 实测：重启 MediaProvider 后
/// `/storage/emulated` 被换成新的 fuse 层并带 `master:`，而这层我们从没碰过），复核于是
/// 把"竞态"判成"设备不支持最小隔离"，回退整树私有化——把本可以保住的 111 条传播标签
/// 一次性抹平。这里改为对违规层就地重试私有化，再判一次；一层都改不动才判失败。
fn verify_isolation(roots: &[String]) -> bool {
    for attempt in 0..2 {
        let Some(violations) = isolation_violations(roots) else {
            log::warn!("mount propagation verify skipped: mountinfo unreadable");
            return false;
        };
        if violations.is_empty() {
            return true;
        }
        if attempt == 1 {
            log::warn!(
                "mount propagation verify failed: serving mount still slave targets={:?}",
                violations
            );
            return false;
        }
        // 自愈：违规层多半是第一次私有化之后才换上的新挂载，就地再私有化一次。
        let healed = violations
            .iter()
            .filter(|target| !target.is_empty() && make_private(target, false))
            .count();
        log::info!(
            "mount propagation verify self-heal healed={} targets={:?}",
            healed,
            violations
        );
        if healed == 0 {
            return false;
        }
    }
    false
}

/// 该层是否为所在挂载点的栈顶（同挂载点上没有把它的 id 当作父挂载的层）。
///
/// 按路径改传播属性只能命中栈顶，因此判定也必须只针对栈顶；否则"被遮蔽的旧层"会让复核
/// 永远失败（见调用点注释）。
fn is_topmost_at(lines: &[String], id: u64, target: &str) -> bool {
    for line in lines {
        let Some(other) = mountinfo::parse_entry(line) else {
            continue;
        };
        if other.parent_mount_id == id && mountinfo::unescape_field(other.target) == target {
            return false;
        }
    }
    true
}

/// 读取 mountinfo 的原始行（走裸系统调用，不受本进程内 PLT hook 影响）。
fn read_mountinfo_lines() -> Option<Vec<String>> {
    let content = proc_direct::read_proc_file("/proc/self/mountinfo", MOUNTINFO_READ_LIMIT)?;
    Some(content.lines().map(|line| line.to_string()).collect())
}

/// 找出当前服务某路径的挂载点（最长前缀匹配），用于定位"这一条子树归谁管"。
fn serving_mount_point(lines: &[String], path: &str) -> Option<String> {
    let normalized = paths::normalize(path);
    let mut best: Option<String> = None;
    for line in lines {
        let Some(entry) = mountinfo::parse_entry(line) else {
            continue;
        };
        let target = mountinfo::unescape_field(entry.target);
        if !is_ancestor_or_self(&target, &normalized) {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|current: &String| target.len() > current.len())
        {
            best = Some(target);
        }
    }
    best
}

/// `ancestor` 是否为 `path` 的祖先或本身（按路径段比较，避免 `/storage/emulated1` 误判）。
fn is_ancestor_or_self(ancestor: &str, path: &str) -> bool {
    if ancestor.is_empty() {
        return false;
    }
    if ancestor == path {
        return true;
    }
    let prefix = ancestor.trim_end_matches('/');
    path.strip_prefix(prefix)
        .is_some_and(|rest| rest.starts_with('/'))
}

/// 把挂载点改成 private；`recursive` 时连同其子挂载一起改。
///
/// 失败只记日志不中断：挂载可能已被系统摘除，真正的把关交给 [`verify_isolation`]。
fn make_private(target: &str, recursive: bool) -> bool {
    let Ok(c_target) = CString::new(target) else {
        return false;
    };
    let mut flags = MS_PRIVATE as c_ulong;
    if recursive {
        flags |= MS_REC as c_ulong;
    }
    // SAFETY: c_target 是 NUL 结尾的合法路径且在本作用域内保持存活；改变传播属性的
    // mount 调用不涉及数据指针，也不修改挂载内容。
    let ret = unsafe {
        mount(
            std::ptr::null(),
            c_target.as_ptr(),
            std::ptr::null(),
            flags,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        let errno = last_errno();
        log::debug!(
            "mount propagation private failed target={} rec={} errno={} {}",
            target,
            recursive,
            errno,
            errno_text(errno)
        );
        return false;
    }
    true
}

/// 整树私有化：只在 [`isolate_mounts_for_roots`] 复核失败时作为安全兜底使用。
///
/// 它会把应用命名空间的传播标签全部抹平（见模块文档），因此绝不在正常路径上调用。
pub fn make_tree_private_fallback() -> bool {
    let Ok(root) = CString::new("/") else {
        return false;
    };
    // SAFETY: root 是 NUL 结尾的合法路径；整树私有化只改传播属性，不涉及数据指针。
    let ret = unsafe {
        mount(
            std::ptr::null(),
            root.as_ptr(),
            std::ptr::null(),
            (MS_REC | MS_PRIVATE) as c_ulong,
            std::ptr::null(),
        )
    };
    if ret != 0 {
        let errno = last_errno();
        log::error!(
            "mount ns: / MS_PRIVATE failed errno={} {}",
            errno,
            errno_text(errno)
        );
        return false;
    }
    log::warn!("mount ns: fell back to full tree private (propagation tags dropped)");
    true
}
