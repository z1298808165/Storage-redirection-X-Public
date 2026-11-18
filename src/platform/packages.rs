use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Duration;

/// `system_server` 包查询服务的 Unix-domain socket 路径。
pub const SOCKET_PATH: &str = "/data/adb/modules/storage.redirect.x/tmp/package_query/socket";
/// 热路径查询的绝对期限上限。
///
/// 身份反查（`uid_for_package` / `packages_for_uid` / `shared_user_id_for_uid`）与
/// 进程扫描都落在挂载、决策的关键路径上，请求本身只回几行，慢就等于链路坏了，
/// 必须尽快失败。
pub const INLINE_QUERY_BUDGET: Duration = Duration::from_secs(2);
/// 界面清单查询（`A` / `T`）的绝对期限上限。
///
/// 这份请求要枚举该用户的全部应用并在 `system_server` 内逐个解析标签，成本随设备
/// 应用数线性增长：实测旗舰机 467 个应用的冷查询就要 1886 ms，中端 OEM 机上应用
/// 更多必然越线。期限若与成本脱钩，「慢」就会被翻译成「失败」，界面拿不到清单，
/// 连带所有应用名与进程身份都解析不出来。
// quality-allow(lint-suppression): 只有 daemon 的 package-query 入口按用途选期限，lib 目标不直接使用此上限。
#[allow(dead_code)]
pub const INVENTORY_QUERY_BUDGET: Duration = Duration::from_secs(15);
/// 一条查询回复允许的最大字节数。
pub const MAX_REPLY: usize = 4 * 1024 * 1024;
/// 按完整 Android UID 索引的包名集合；共享 UID 保留全部包名。
pub type PackageUids = HashMap<i32, HashSet<String>>;

/// 一个 Android 用户下由 PackageManager 枚举出的应用元数据。
// quality-allow(lint-suppression): daemon 的 PackageManager 查询会构造此类型，其他入口不直接使用。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageApp {
    pub user_id: i32,
    pub uid: i32,
    pub is_system: bool,
    pub package_name: String,
    pub label: String,
    pub apk_path: String,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn valid_package(package: &str) -> bool {
    !package.is_empty()
        && package.len() <= 255
        && package.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
        })
}

// quality-allow(lint-suppression): 这些数字字段解析器只由 daemon 的包清单请求使用。
#[allow(dead_code)]
fn decimal(value: &str, field: &str) -> io::Result<i32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(field));
    }
    value.parse::<i32>().map_err(|_| invalid(field))
}

// quality-allow(lint-suppression): 转义解码器只由 daemon 的 PackageManager 清单解析流程调用。
#[allow(dead_code)]
fn decode_field(value: &str) -> io::Result<String> {
    let input = value.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        if input[index] == b'%' {
            let high = input
                .get(index + 1)
                .copied()
                .and_then(hex_digit)
                .ok_or_else(|| invalid("package field escape"))?;
            let low = input
                .get(index + 2)
                .copied()
                .and_then(hex_digit)
                .ok_or_else(|| invalid("package field escape"))?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            let byte = input[index];
            if !(byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'~')) {
                return Err(invalid("package field encoding"));
            }
            decoded.push(byte);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| invalid("package field UTF-8"))
}

// quality-allow(lint-suppression): 十六进制转换仅供 PackageManager 字段转义解码器使用。
#[allow(dead_code)]
fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// 解析一位用户的 `user\tuid\tsystem\tpackage\tlabel\tapk_path` 清单。
// quality-allow(lint-suppression): daemon 的 A 查询使用它解析指定用户的 PackageManager 清单。
#[allow(dead_code)]
pub fn parse_app_inventory(content: &str, expected_user: i32) -> io::Result<Vec<PackageApp>> {
    if expected_user < 0 {
        return Err(invalid("package user"));
    }
    let mut apps = Vec::new();
    let mut packages = HashSet::new();
    for line in content.lines() {
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 6 {
            return Err(invalid("package app row"));
        }
        let user_id = decimal(fields[0], "package user")?;
        let uid = decimal(fields[1], "package UID")?;
        let is_system = match fields[2] {
            "0" => false,
            "1" => true,
            _ => return Err(invalid("package system marker")),
        };
        let package_name = fields[3];
        let label = decode_field(fields[4])?;
        let apk_path = decode_field(fields[5])?;
        if user_id != expected_user
            || uid < 0
            || uid / 100_000 != expected_user
            || !valid_package(package_name)
            || label.is_empty()
            || (!apk_path.is_empty() && !apk_path.starts_with('/'))
            || !packages.insert(package_name)
        {
            return Err(invalid("package app identity"));
        }
        apps.push(PackageApp {
            user_id,
            uid,
            is_system,
            package_name: package_name.to_owned(),
            label,
            apk_path,
        });
    }
    Ok(apps)
}

/// 解析 `system_server` 返回的 Android 用户 ID 清单。
// quality-allow(lint-suppression): daemon 的 T 查询使用它解析 PackageManager 用户清单。
#[allow(dead_code)]
pub fn parse_users(content: &str) -> io::Result<Vec<i32>> {
    let mut users = Vec::new();
    let mut seen = HashSet::new();
    for line in content.lines() {
        let user = decimal(line, "package user row")?;
        if user < 0 || !seen.insert(user) {
            return Err(invalid("package user identity"));
        }
        users.push(user);
    }
    if users.is_empty() {
        return Err(invalid("empty user inventory"));
    }
    Ok(users)
}

/// base64url 字母表：不含 `+`、`/`，因此不会被任何 URL/form 解码改动。
const BASE64URL_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// 把清单回复编码成 base64url，供 WebUI 这类文本通道使用。
///
/// WebUI 取 stdout 的通道会把整段输出当成 URL 解码一次（KernelSU/APatch 用
/// `loadUrl("javascript:…")` 回传结果）：清单里每一处 `%XX` 转义（含 `%2F` 的路径、
/// `%E7%B3%BB` 这类非 ASCII 标签）都会在到达 JS 之前被还原成裸字节，percent 编码
/// 协议因此不成立，解析必然拒绝整份清单（见 2026-11-18 issue 20 的现场取证）。
/// base64url 只含字母数字与 `-`/`_`，会被解码的通道和不解码的通道都能原样送达。
// quality-allow(lint-suppression): 只有 daemon 的 package-query 入口输出该承载形式，lib 目标不直接调用。
#[allow(dead_code)]
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (first << 16) | (second << 8) | third;
        encoded.push(BASE64URL_ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        encoded.push(BASE64URL_ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        encoded.push(if chunk.len() > 1 {
            BASE64URL_ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            BASE64URL_ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    encoded
}

/// 解析 `package:uid` 行；保留多用户与共享 UID，任一畸形行都会拒绝整份结果。
pub fn parse_uids(content: &str) -> io::Result<PackageUids> {
    let mut packages = PackageUids::new();
    for line in content.lines() {
        let (package, uid) = line.split_once(':').ok_or_else(|| invalid("package row"))?;
        if uid.is_empty() || !uid.bytes().all(|c| c.is_ascii_digit()) {
            return Err(invalid("package UID"));
        }
        let uid = uid.parse::<i32>().map_err(|_| invalid("package UID"))?;
        if uid < 0 || !valid_package(package) {
            return Err(invalid("package identity"));
        }
        packages.entry(uid).or_default().insert(package.to_owned());
    }
    Ok(packages)
}

/// 验证大端长度前缀与 `OK\n` 状态前缀，并返回 UTF-8 正文。
pub fn decode_reply(header: [u8; 4], body: &[u8]) -> io::Result<String> {
    let size = u32::from_be_bytes(header) as usize;
    if !(3..=MAX_REPLY).contains(&size) || size != body.len() {
        return Err(invalid("package response size"));
    }
    let text = std::str::from_utf8(body).map_err(|_| invalid("package response encoding"))?;
    text.strip_prefix("OK\n")
        .map(str::to_owned)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotConnected, text.to_owned()))
}

#[cfg(unix)]
// quality-allow(lint-suppression): 全量扫描仅供 daemon，lib 目标共享编译此模块。
#[allow(dead_code)]
/// 从 `system_server` 读取所有用户的已安装包与完整 UID。
pub fn read_uids() -> io::Result<PackageUids> {
    parse_uids(&query("L")?)
}

#[cfg(unix)]
/// 查询包在 `user` 中的完整 UID；未安装时返回 `None`。
pub fn uid_for_package(package: &str, user: i32) -> io::Result<Option<i32>> {
    if user < 0 || !valid_package(package) {
        return Err(invalid("package request"));
    }
    let rows = parse_uids(&query(&format!("P\t{user}\t{package}"))?)?;
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() != 1 {
        return Err(invalid("ambiguous package reply"));
    }
    let (uid, names) = rows
        .into_iter()
        .next()
        .ok_or_else(|| invalid("package reply"))?;
    if uid / 100000 != user || names.len() != 1 || !names.contains(package) {
        return Err(invalid("mismatched package reply"));
    }
    Ok(Some(uid))
}

#[cfg(unix)]
/// 查询完整 UID 对应的所有共享 UID 包名。
pub fn packages_for_uid(uid: i32) -> io::Result<HashSet<String>> {
    if uid < 0 {
        return Err(invalid("UID request"));
    }
    let mut rows = parse_uids(&query(&format!("U\t{uid}"))?)?;
    let names = rows.remove(&uid).unwrap_or_default();
    if !rows.is_empty() {
        return Err(invalid("mismatched UID reply"));
    }
    Ok(names)
}

/// 查询完整 UID 的 PM 共享用户标签；无法确定组名时返回 `None`，不选择组内成员。
#[cfg(unix)]
pub fn shared_user_id_for_uid(uid: i32) -> io::Result<Option<String>> {
    if uid < 0 {
        return Err(invalid("UID request"));
    }
    parse_shared_user_id(&query(&format!("S\t{uid}"))?)
}

pub fn parse_shared_user_id(reply: &str) -> io::Result<Option<String>> {
    if reply.is_empty() {
        return Ok(None);
    }
    let value = reply.strip_suffix('\n').unwrap_or(reply);
    if !value.contains('.') || !valid_package(value) {
        return Err(invalid("shared user identity"));
    }
    Ok(Some(value.to_owned()))
}

/// 通过独立连接查询 `system_server`，使用热路径的 [`INLINE_QUERY_BUDGET`]。
///
/// 回复只在 UID 为 1000 且 SELinux 域为 `system_server` 时接受；每次调用新建连接，
/// 不跨 fork 保存连接或查询结果。
#[cfg(unix)]
pub fn query(request: &str) -> io::Result<String> {
    query_with_budget(request, INLINE_QUERY_BUDGET)
}

/// 通过独立连接查询 `system_server`；仅 root 可用，`budget` 覆盖连接、发送与收包。
///
/// 调用方按用途选上限：身份反查类走 [`query`]（[`INLINE_QUERY_BUDGET`]），界面
/// 清单类走 [`INVENTORY_QUERY_BUDGET`]。期限是绝对期限而不是空闲期限，因此它必须
/// 大于该请求在目标设备上的实际耗时上限，否则「慢」会被当成「链路坏了」。
#[cfg(unix)]
pub fn query_with_budget(request: &str, budget: Duration) -> io::Result<String> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::time::Instant;
    if request.is_empty()
        || request.len() >= 512
        || request
            .bytes()
            .any(|c| c == 0 || c == b'\n' || !c.is_ascii())
    {
        return Err(invalid("invalid package request"));
    }
    // SAFETY: geteuid 不读写调用方内存。
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    let deadline = Instant::now() + budget;
    let mut stream = connect(deadline)?;
    // SAFETY: ucred 仅含整数，可零初始化为 getsockopt 的输出缓冲。
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: 内核按 length 写入有效栈缓冲；SO_PEERCRED/SO_PEERSEC 均来自对端内核身份。
    if unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut label = [0u8; 256];
    let mut label_length = label.len() as libc::socklen_t;
    // SAFETY: label_length 是 label 的容量，内核仅在该有效缓冲范围内写入安全上下文。
    if credentials.uid != 1000
        || length as usize != std::mem::size_of_val(&credentials)
        || unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERSEC,
                label.as_mut_ptr().cast(),
                &mut label_length,
            )
        } != 0
        || label
            .get(..label_length as usize)
            .map(|value| value.strip_suffix(&[0]).unwrap_or(value))
            != Some(b"u:r:system_server:s0")
    {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    stream.set_write_timeout(Some(remaining(deadline)?))?;
    stream.write_all(format!("{request}\n").as_bytes())?;
    let mut header = [0u8; 4];
    read_bounded(&mut stream, &mut header, deadline)?;
    let size = u32::from_be_bytes(header) as usize;
    if !(3..=MAX_REPLY).contains(&size) {
        return Err(invalid("package response size"));
    }
    let mut body = vec![0; size];
    read_bounded(&mut stream, &mut body, deadline)?;
    decode_reply(header, &body)
}

#[cfg(unix)]
fn connect(deadline: std::time::Instant) -> io::Result<std::os::unix::net::UnixStream> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    // SAFETY: sockaddr_un 仅含整数数组，零初始化同时提供路径末尾的 NUL。
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as _;
    for (dest, source) in address.sun_path.iter_mut().zip(SOCKET_PATH.bytes()) {
        *dest = source as _;
    }
    // SAFETY: socket 不读写调用方内存；返回的描述符在检查成功后立即接管。
    let raw = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if raw < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: raw 已确认有效，且尚无其它对象持有其关闭责任。
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: address 在调用期间有效，长度与 sockaddr_un 的实际大小一致。
    if unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&address as *const libc::sockaddr_un).cast(),
            std::mem::size_of_val(&address) as _,
        )
    } != 0
    {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        let mut event = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        loop {
            let timeout = remaining(deadline)?.as_millis().clamp(1, i32::MAX as u128) as i32;
            // SAFETY: event 指向一个有效 pollfd，数量固定为一。
            let ready = unsafe { libc::poll(&mut event, 1, timeout) };
            if ready > 0 {
                break;
            }
            if ready < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(io::Error::new(io::ErrorKind::TimedOut, "package connect"));
        }
        let mut error: i32 = 0;
        let mut length = std::mem::size_of_val(&error) as libc::socklen_t;
        // SAFETY: error 与 length 为有效输出缓冲，容量匹配 SO_ERROR 的整数结果。
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut i32).cast(),
                &mut length,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
    }
    let stream = std::os::unix::net::UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(unix)]
fn remaining(deadline: std::time::Instant) -> io::Result<std::time::Duration> {
    deadline
        .checked_duration_since(std::time::Instant::now())
        .filter(|time| !time.is_zero())
        .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "package query deadline"))
}

#[cfg(unix)]
fn read_bounded(
    stream: &mut std::os::unix::net::UnixStream,
    mut bytes: &mut [u8],
    deadline: std::time::Instant,
) -> io::Result<()> {
    use std::io::Read;
    while !bytes.is_empty() {
        stream.set_read_timeout(Some(remaining(deadline)?))?;
        let size = match stream.read(bytes) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "package reply truncated",
            ));
        }
        bytes = &mut bytes[size..];
    }
    Ok(())
}
