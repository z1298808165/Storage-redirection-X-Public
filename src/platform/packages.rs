use std::collections::{HashMap, HashSet};
use std::io;

/// `system_server` 包查询服务的 Unix-domain socket 路径。
pub const SOCKET_PATH: &str = "/data/adb/modules/storage.redirect.x/tmp/package_query/socket";
/// 一条查询回复允许的最大字节数。
pub const MAX_REPLY: usize = 4 * 1024 * 1024;
/// 按完整 Android UID 索引的包名集合；共享 UID 保留全部包名。
pub type PackageUids = HashMap<i32, HashSet<String>>;

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

/// 通过独立连接查询 `system_server`；仅 root 可用，单次连接受两秒绝对期限约束。
///
/// 回复只在 UID 为 1000 且 SELinux 域为 `system_server` 时接受；每次调用新建连接，
/// 不跨 fork 保存连接或查询结果。
#[cfg(unix)]
pub fn query(request: &str) -> io::Result<String> {
    use std::io::Write;
    use std::os::fd::AsRawFd;
    use std::time::{Duration, Instant};
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
    let deadline = Instant::now() + Duration::from_secs(2);
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
