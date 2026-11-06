//! `/proc/<pid>/mountinfo` 单行解析。
//!
//! 挂载清理、挂载规划与 scoped FUSE 会话收尾都要按挂载点判断"这条挂载记录是什么"，
//! 此前各模块各写了一遍单行解析。这里提供共享的最小解析：字段保持借用，由调用方决定
//! 展开哪些转义字段，避免为未命中的记录分配字符串。

/// `/proc/self/mountinfo` 中一条挂载记录。
pub struct MountInfoEntry<'a> {
    /// 挂载 ID。**不能**用它判断同挂载点多层挂载的叠放顺序：经 detached tree +
    /// `move_mount` 搬运进命名空间的层保留创建侧分配的 ID，可能与后挂的层大小倒挂
    /// （真机 2026-10-15：scoped FUSE 层 id=233 小于其下屏障层 id=10034）。叠层
    /// 顺序只能按 `parent_mount_id` 父链判定，见 [`topmost_by_stack`]。
    pub mount_id: u64,
    /// 父挂载 ID：本层挂载所叠在的那一层（同挂载点叠放时即下面一层）的挂载 ID。
    pub parent_mount_id: u64,
    /// 设备号（`major:minor`）。同一文件系统内 bind 前后的层共享同一设备号，
    /// 覆盖挂载感知的源路径换算用它寻找该文件系统根（`root=="/"`）的挂载点。
    pub dev: &'a str,
    /// 挂载在文件系统内的根，未展开八进制转义。
    pub root: &'a str,
    /// 挂载点，未展开八进制转义。
    pub target: &'a str,
    /// 文件系统类型：内核直挂的 scoped 挂载是 `fuse`，经 fusermount 回退时是 `fuse.srx`。
    pub fs_type: &'a str,
    /// 挂载源，即 `mount(2)` 的 `source`。
    pub source: &'a str,
}

/// 解析一行 `mountinfo`；字段不足或挂载 ID 无法解析时返回 None。
pub fn parse_entry(line: &str) -> Option<MountInfoEntry<'_>> {
    let separator = line.find(" - ")?;
    let mut before_fields = line[..separator].split_whitespace();
    let mount_id = before_fields.next()?.parse::<u64>().ok()?;
    let parent_mount_id = before_fields.next()?.parse::<u64>().ok()?;
    let dev = before_fields.next()?;
    let root = before_fields.next()?;
    let target = before_fields.next()?;
    let mut after_fields = line[separator + 3..].split_whitespace();
    let fs_type = after_fields.next()?;
    let source = after_fields.next()?;
    Some(MountInfoEntry {
        mount_id,
        parent_mount_id,
        dev,
        root,
        target,
        fs_type,
        source,
    })
}

/// 该挂载行的可选字段里是否带指定传播标签（`shared:` / `master:` / `propagate_from:` / `unbindable`）。
///
/// 传播标签只出现在挂载点之后、`-` 之前的可选字段区：`shared:N` 表示该挂载参与某个
/// 对等组（改动会外播），`master:N` 表示它是某个对等组的 slave（会接受入向事件）。
/// 判断命名空间隔离形态时必须能读到这两个字段，否则只剩"按路径猜"这条不可靠的路。
pub fn has_propagation_tag(line: &str, tag: &str) -> bool {
    let Some(separator) = line.find(" - ") else {
        return false;
    };
    let mut fields = line[..separator].split_whitespace();
    // 跳过挂载 ID、父 ID、设备号、root、挂载点，剩下的都是可选字段。
    for _ in 0..5 {
        fields.next();
    }
    fields.any(|field| field.starts_with(tag))
}

/// 从同一挂载点上的多条挂载记录中选出栈顶。
///
/// 内核语义：同挂载点的叠层构成父链（上层 `parent_mount_id` = 下层挂载 ID），栈顶 =
/// 没有任何同挂载点条目叠在它上面的那一层，即「没有被其它同挂载点条目引用为父」的层。
/// 病态的多条独立父链（detached tree 搬运出的兄弟叠层）时取文件序最后一条兜底（与
/// /proc/mounts 的栈顶约定一致），因此 `entries` 必须保持 mountinfo 原始文件序。
pub fn topmost_by_stack<T, M, P>(entries: &[T], mount_id: M, parent_mount_id: P) -> Option<&T>
where
    M: Fn(&T) -> u64,
    P: Fn(&T) -> u64,
{
    let entry_ids: std::collections::HashSet<u64> = entries.iter().map(&mount_id).collect();
    // 被其它同挂载点条目引用为父的层，上面还压着层，不是栈顶。
    let covered: std::collections::HashSet<u64> = entries
        .iter()
        .map(parent_mount_id)
        .filter(|parent| entry_ids.contains(parent))
        .collect();
    let mut top: Option<&T> = None;
    for entry in entries {
        if covered.contains(&mount_id(entry)) {
            continue;
        }
        // 兜底并列时文件序靠后者胜出（后挂的层在 mountinfo 里靠后）。
        top = Some(entry);
    }
    top
}

/// 展开 `mountinfo` 字段里的八进制转义（空格、换行、反斜杠等）。
pub fn unescape_field(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(value.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &bytes[index + 1..index + 4];
            if digits.iter().all(|ch| (b'0'..=b'7').contains(ch)) {
                let code = (digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + (digits[2] - b'0');
                out.push(code);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
