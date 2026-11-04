// 目录快照：cursor 行过滤的存在性判定，按目录枚举一次后逐行查名字。
//
// 原生 MediaProvider 处理一次相册查询是这样的：SQL 出结果集 → 逐行读列 → 交给
// 调用方，**行级不做任何文件系统判断**；文件是否还在，由扫描器（一次目录枚举）
// 维护进数据库。FUSE 侧同样是「一次 readdir + 轻过滤」。本模块给隔离应用加了一层
// 「这一行在当前视图里到底可不可见」的判定，于是两万行的查询变成了两万次存在性
// 系统调用——实测单次 FUSE 往返约 250µs（见 temp/issue-7-media-latency-analysis.md），
// 这就是 issue #7 冷启动相册首屏数秒的来源。
//
// 因此这里把存在性判定还原成原生形态：**每个目录枚举一次**，逐行只做一次名字
// 比对。缓存的失效口径也对齐内核：内核对 dentry/属性的缓存是秒级超时，FUSE 侧
// 目录候选缓存同样按有效期校验；这里取同一量级，窗口内新建的文件最迟一个窗口后
// 可见，窗口外的判定与逐条 faccessat 完全等价。
//
// 枚举本身不免费。真机 A/B（5021 条目的单个目录、MediaStore 查询只取 50 行）：
// 枚举一次约 60ms，逐行 faccessat 每行约 0.7ms。也就是说「枚举一次」只有在同一
// 目录里要查的行数足够多时才划算。所以这里不无条件枚举，而是**按代价切换**：
// 同一目录累计查满若干行之后才枚举，小分页继续逐条查，整库查询在第一页就切过去。
// 两条分支的判定结果完全一致，只是成本路径不同。
//
// 语义边界（重要）：
// - 目录不存在 → 其下任何名字都不存在，这是 POSIX 语义，不是近似；
// - 目录不可枚举（权限不足、条目过多）→ 退回逐条存在性检查，结论与改动前一致。

use crate::platform::paths;
use once_cell::sync::Lazy;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::sync::Mutex;

/// 枚举结果的有效期：与内核 dentry/属性缓存、FUSE 侧目录候选缓存同一量级。
/// 它只决定「新增文件最迟多久可见」，窗口外的判定与逐条 faccessat 等价。
const DIR_SNAPSHOT_TTL_MS: i64 = 250;
/// 同一目录累计查满这么多行之后改为枚举。
///
/// 依据是真机 A/B：枚举一个目录的成本 ≈ 单次 getdents 往返 × 目录条目数
/// （5021 条目约 60ms），逐行 faccessat 约 0.2–0.7ms。小分页逐条更便宜，整库
/// 查询每目录上千行时枚举一次就回本。16 行取在两者之间的安全侧。
const DIR_ENUMERATE_AFTER_ROWS: u32 = 16;
/// 同时记住的目录数：一次查询的行落在极少数目录里，超出即整体丢弃重来，
/// 与 FUSE 侧目录候选缓存「宁可不命中也不留垃圾」的处理方式一致。
const DIR_SNAPSHOT_MAX_DIRS: usize = 32;
/// 单目录条目上限：超过就判定为不可枚举、退回逐条检查。取值要覆盖「两万张照片
/// 同在一个目录」这种真实相册形态，否则最大的那批目录反而退回逐条 stat。
const DIR_SNAPSHOT_MAX_NAMES: usize = 65_536;
/// 单次 getdents64 的缓冲区大小。目录项密集时多轮读取，但固定栈缓冲避免每次分配。
const DIR_SNAPSHOT_BUF_SIZE: usize = 8 * 1024;

enum DirState {
    /// 目录不存在（或其路径不可达）：其下所有名字都不存在。
    Missing,
    /// 目录存在但无法枚举（不可搜索/不可读/条目过多）：逐条检查。
    Unlisted,
    /// 目录可枚举，集合就是其直接子项名。
    Listed(HashSet<Vec<u8>>),
}

/// 单个目录的已知信息。
enum DirSnapshotState {
    /// 还没枚举：先按逐条 faccessat 观察，累计行数达到阈值再枚举。
    Observing { rows: u32, at_ms: i64 },
    /// 目录不可达：其下名字都不存在，无需再问内核。
    Missing { at_ms: i64 },
    /// 已枚举：集合即该目录的直接子项名。
    Listed { names: HashSet<Vec<u8>>, at_ms: i64 },
}

impl DirSnapshotState {
    fn at_ms(&self) -> i64 {
        match self {
            DirSnapshotState::Observing { at_ms, .. }
            | DirSnapshotState::Missing { at_ms }
            | DirSnapshotState::Listed { at_ms, .. } => *at_ms,
        }
    }
}

#[derive(Default)]
struct SnapshotCache {
    dirs: HashMap<String, DirSnapshotState>,
}

/// 进程级共享的目录快照。
///
/// MediaProvider 的查询会并发跑在多个线程上，而同一批行通常落在同一批目录：
/// 共享一份缓存比每线程各枚举一次更省，也让缓存的规模只由目录数与目录条目数
/// 决定（两者都有上限）。逐行的锁只做一次哈希查表，未命中时才在锁外做系统调用。
static SNAPSHOTS: Lazy<Mutex<SnapshotCache>> = Lazy::new(|| Mutex::new(SnapshotCache::default()));

/// 游标行存在性判定：按目录折叠枚举，逐行只做一次名字查表。
pub(crate) fn path_exists_for_cursor_row(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let Some((dir, name)) = path.rsplit_once('/') else {
        return path_exists_by_syscall(path);
    };
    // 根路径、目录自身、带尾斜杠的输入都不做折叠，保持逐条判定。
    if dir.is_empty() || name.is_empty() {
        return path_exists_by_syscall(path);
    }
    lookup_or_scan(dir, name.as_bytes()).unwrap_or_else(|| path_exists_by_syscall(path))
}

/// 返回 `Some(存在与否)`；`None` 表示这一行按逐条 faccessat 判定
/// （目录还在观察期，或不可枚举）。
fn lookup_or_scan(dir: &str, name: &[u8]) -> Option<bool> {
    match observe_or_hit(dir, name) {
        Observation::Answer { exists } => Some(exists),
        Observation::PerRow => None,
        Observation::Enumerate => Some(replace_with_snapshot(dir, name)),
    }
}

enum Observation {
    /// 缓存已能给出确定结论。
    Answer { exists: bool },
    /// 观察期未满：这一行按逐条 faccessat 判定。
    PerRow,
    /// 观察期已满：该枚举这个目录了。
    Enumerate,
}

fn observe_or_hit(dir: &str, name: &[u8]) -> Observation {
    let now_ms = paths::monotonic_ms();
    let Ok(mut cache) = SNAPSHOTS.lock() else {
        return Observation::PerRow;
    };
    let Some(entry) = cache.dirs.get_mut(dir) else {
        cache.dirs.insert(
            dir.to_string(),
            DirSnapshotState::Observing {
                rows: 1,
                at_ms: now_ms,
            },
        );
        return Observation::PerRow;
    };
    if now_ms.saturating_sub(entry.at_ms()) >= DIR_SNAPSHOT_TTL_MS {
        // 过期即视为未知：重新进入观察期，避免把旧快照当成事实。
        *entry = DirSnapshotState::Observing {
            rows: 1,
            at_ms: now_ms,
        };
        return Observation::PerRow;
    }
    match entry {
        DirSnapshotState::Missing { .. } => Observation::Answer { exists: false },
        DirSnapshotState::Listed { names, .. } => Observation::Answer {
            exists: names.contains(name),
        },
        DirSnapshotState::Observing { rows, .. } => {
            *rows = rows.saturating_add(1);
            if *rows >= DIR_ENUMERATE_AFTER_ROWS {
                Observation::Enumerate
            } else {
                Observation::PerRow
            }
        }
    }
}

/// 枚举该目录并把结果写回缓存，返回当前这一行的存在性。
fn replace_with_snapshot(dir: &str, name: &[u8]) -> bool {
    let now_ms = paths::monotonic_ms();
    let (state, exists) = match scan_dir(dir) {
        DirState::Missing => (DirSnapshotState::Missing { at_ms: now_ms }, false),
        DirState::Listed(names) => {
            let exists = names.contains(name);
            (
                DirSnapshotState::Listed {
                    names,
                    at_ms: now_ms,
                },
                exists,
            )
        }
        // 不可枚举：退回逐条判定，不把失败态写进缓存。
        DirState::Unlisted => return path_exists_by_syscall(&join(dir, name)),
    };
    if let Ok(mut cache) = SNAPSHOTS.lock() {
        if cache.dirs.len() >= DIR_SNAPSHOT_MAX_DIRS {
            cache.dirs.clear();
        }
        cache.dirs.insert(dir.to_string(), state);
    }
    exists
}

fn join(dir: &str, name: &[u8]) -> String {
    let mut path = String::with_capacity(dir.len() + name.len() + 1);
    path.push_str(dir);
    path.push('/');
    path.push_str(&String::from_utf8_lossy(name));
    path
}

/// 枚举目录的直接子项名。整条路径走原始系统调用，绕开本进程内可能存在的路径 Hook，
/// 与既有的 `path_exists_by_syscall` 保持同一取证口径。
fn scan_dir(dir: &str) -> DirState {
    let Ok(c_dir) = CString::new(dir) else {
        return DirState::Unlisted;
    };
    // SAFETY: c_dir 是有效的 NUL 结尾 C 字符串，faccessat 只读取它。
    if unsafe {
        libc::syscall(
            libc::SYS_faccessat,
            libc::AT_FDCWD,
            c_dir.as_ptr(),
            libc::F_OK,
            0,
        )
    } != 0
    {
        // 目录不可达（不存在 / 路径某级不可搜索）：其下名字一律不可达，与逐条
        // faccessat 的结论一致。
        return DirState::Missing;
    }
    // 目录存在但没有搜索权限时，逐个 faccessat 子项同样会失败而枚举仍可能成功；
    // 两条路径结论不一致，故显式要求可搜索，否则退回逐条。
    // SAFETY: 同上，仅读取 C 字符串。
    if unsafe {
        libc::syscall(
            libc::SYS_faccessat,
            libc::AT_FDCWD,
            c_dir.as_ptr(),
            libc::X_OK,
            0,
        )
    } != 0
    {
        return DirState::Unlisted;
    }
    // SAFETY: c_dir 有效；openat 只读取路径，返回 fd 由下方 close 关闭。
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat,
            libc::AT_FDCWD,
            c_dir.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            0,
        )
    } as libc::c_int;
    if fd < 0 {
        return DirState::Unlisted;
    }
    let names = read_dir_names(fd);
    // SAFETY: fd 由上方 openat 成功返回且未转移所有权。
    unsafe { libc::close(fd) };
    match names {
        Some(names) => DirState::Listed(names),
        None => DirState::Unlisted,
    }
}

fn read_dir_names(fd: libc::c_int) -> Option<HashSet<Vec<u8>>> {
    let mut names = HashSet::new();
    let mut buf = [0u8; DIR_SNAPSHOT_BUF_SIZE];
    loop {
        // SAFETY: buf 是长度为 DIR_SNAPSHOT_BUF_SIZE 的有效可变缓冲区，getdents64
        // 最多写入这么多字节，返回值不超过它。
        let read = unsafe {
            libc::syscall(
                libc::SYS_getdents64,
                fd,
                buf.as_mut_ptr() as *mut libc::c_void,
                buf.len(),
            )
        };
        if read < 0 {
            return None;
        }
        if read == 0 {
            return Some(names);
        }
        let read = read as usize;
        let mut offset = 0usize;
        while offset + DIR_ENTRY_NAME_OFFSET <= read {
            // struct linux_dirent64 { u64 d_ino; i64 d_off; u16 d_reclen; u8 d_type; char d_name[]; }
            let reclen = u16::from_ne_bytes([buf[offset + 16], buf[offset + 17]]) as usize;
            if reclen <= DIR_ENTRY_NAME_OFFSET || offset + reclen > read {
                return None;
            }
            let record = &buf[offset..offset + reclen];
            let name_start = DIR_ENTRY_NAME_OFFSET;
            let name_end = record[name_start..]
                .iter()
                .position(|byte| *byte == 0)
                .map(|pos| name_start + pos)
                .unwrap_or(reclen);
            let name = &record[name_start..name_end];
            if !name.is_empty() && name != b"." && name != b".." {
                if names.len() >= DIR_SNAPSHOT_MAX_NAMES {
                    return None;
                }
                names.insert(name.to_vec());
            }
            offset += reclen;
        }
    }
}

/// linux_dirent64 中 d_name 的偏移：d_ino(8) + d_off(8) + d_reclen(2) + d_type(1)。
const DIR_ENTRY_NAME_OFFSET: usize = 19;

// 通过 syscall 检查路径是否存在，避免走 Hook 链路
fn path_exists_by_syscall(path: &str) -> bool {
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    // SAFETY: c_path 是有效的 NUL 结尾 C 字符串，faccessat 只读取它。
    let ret = unsafe {
        libc::syscall(
            libc::SYS_faccessat,
            libc::AT_FDCWD,
            c_path.as_ptr(),
            libc::F_OK,
            0,
        ) as libc::c_int
    };
    ret == 0
}
