// SQLite `-shm` 边车的共用判定。
//
// daemon（`fuse_redirect`）与 hook（MediaProvider 侧）必须对同一个后端文件做出完全一致
// 的「wal-index 是否毒化」判定。这两处曾各写一份实现，改一侧忘另一侧就会让行为分叉
// （一侧删、一侧不删，恰好是反复掉登录的形态），因此判定收敛到这里，只保留一个实现。

use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// wal-index 头的有效长度（读到 salt 段为止）。
const SHM_HEADER_LEN: usize = 48;
/// `-wal` 文件头的有效长度（salt 位于偏移 16..24）。
const WAL_HEADER_LEN: usize = 32;
/// `-wal` 头魔数的两种合法形态（大端字节序存储，值本身两种都出现过）。
const WAL_MAGIC_A: u32 = 0x377f_0682;
const WAL_MAGIC_B: u32 = 0x377f_0683;

/// 由 `-shm` 边车路径推出同级 `-wal` 路径：后缀按小写不敏感识别，替换为小写
/// `-wal`，库名主干保持原样。
pub fn wal_path_for(shm_path: &Path) -> Option<PathBuf> {
    let file_name = shm_path.file_name()?.to_str()?;
    let lower = file_name.to_ascii_lowercase();
    let stem_len = lower.len().checked_sub("-shm".len())?;
    if stem_len == 0 || !lower.ends_with("-shm") {
        return None;
    }
    let mut wal_path = shm_path.to_path_buf();
    wal_path.set_file_name(format!("{}-wal", &file_name[..stem_len]));
    Some(wal_path)
}

/// 文件身份：两次读取之间文件被整体替换（新 inode / 新 mtime / 新长度）时，快照不可比。
#[derive(Clone, Copy, Eq, PartialEq)]
struct FileId {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: (i64, i64),
}

impl FileId {
    fn of(metadata: &std::fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            size: metadata.size(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
        }
    }
}

/// 一次 `(wal-index 头, -wal 头, 两侧身份)` 快照。
struct SidecarSnapshot {
    shm: [u8; SHM_HEADER_LEN],
    wal: [u8; WAL_HEADER_LEN],
    shm_id: FileId,
    wal_id: FileId,
}

/// 读取文件前 `N` 字节；不足或不可读时返回 `None`。
fn read_prefix<const N: usize>(path: &Path) -> Option<[u8; N]> {
    let mut buf = [0u8; N];
    let mut file = std::fs::File::open(path).ok()?;
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// 读取一次快照。
///
/// 任一不可读、`-shm` 头未初始化（isInit≠1）或 `-wal` 魔数非法时返回 `None`——这些
/// 情况下 SQLite 自己会重建，不该由模块判定为毒化。
fn snapshot(shm_path: &Path) -> Option<SidecarSnapshot> {
    let shm_meta = std::fs::metadata(shm_path).ok()?;
    let shm = read_prefix::<SHM_HEADER_LEN>(shm_path)?;
    if shm[12] != 1 {
        return None;
    }
    let wal_path = wal_path_for(shm_path)?;
    let wal_meta = std::fs::metadata(&wal_path).ok()?;
    let wal = read_prefix::<WAL_HEADER_LEN>(&wal_path)?;
    let wal_magic = u32::from_be_bytes([wal[0], wal[1], wal[2], wal[3]]);
    if wal_magic != WAL_MAGIC_A && wal_magic != WAL_MAGIC_B {
        return None;
    }
    Some(SidecarSnapshot {
        shm,
        wal,
        shm_id: FileId::of(&shm_meta),
        wal_id: FileId::of(&wal_meta),
    })
}

/// 满尺寸 `-shm` 的 wal-index 是否与同级 `-wal` 代际失配（毒化）。
///
/// SQLite 恢复 wal-index 时按 `memcpy` 原样拷贝 `-wal` 文件头（大端字节模式）的 8 字节
/// salt 到 wal-index 头偏移 32..40，不做字节序转换，因此用原始字节比对即可。失配说明
/// wal-index 停留在旧一代 WAL 或来自分裂视图；SQLite 会信任已初始化的 shm 头，把未
/// checkpoint 的 WAL 整体判无效并静默回滚到旧检查点，随后的写入覆写 WAL 造成不可逆
/// 数据丢失——这种状态必须由模块先删掉边车。
///
/// 判定取两次快照并集，避免把「正在正常换代」误判成毒化：
/// 1. `-wal` 换代窗口内新日志替换旧日志，两处 salt 天然不同，单次读会误判；
/// 2. 只有两次快照的 `-wal` 头、`-wal` 身份、`-shm` 身份全部稳定、且仍然失配，
///    才认定是稳定毒化。
///
/// 反方向（把毒化误判成健康）只是延后一轮自愈，下一次 LOOKUP 会再判，不会造成数据
/// 风险，因此这里宁可不判。两次读都是页缓存级小读，没有 sleep，不阻塞调用线程。
pub fn wal_index_stale(shm_path: &Path) -> bool {
    let Some(first) = snapshot(shm_path) else {
        return false;
    };
    if first.shm[32..40] == first.wal[16..24] {
        return false;
    }
    let Some(second) = snapshot(shm_path) else {
        return false;
    };
    first.shm_id == second.shm_id
        && first.wal_id == second.wal_id
        && first.wal == second.wal
        && second.shm[32..40] != second.wal[16..24]
}
