pub const IDENTITY_CAPACITY: usize = 256;
pub const REQUEST_SIZE: usize = IDENTITY_CAPACITY * 2;

/// 子进程只做固定大小的消息交换；配置、日志和挂载留在 companion 中。
pub fn specialize(fd: i32, payload: &[u8; REQUEST_SIZE]) -> bool {
    use crate::platform::{companion_socket, fs};
    use std::os::fd::{FromRawFd, OwnedFd};
    // SAFETY: connectCompanion 将此 FD 的所有权转交给调用方。
    let connection = unsafe { OwnedFd::from_raw_fd(fd) };
    use std::os::fd::AsRawFd;
    let fd = connection.as_raw_fd();
    let mut status = [0; 4];
    companion_socket::configure(fd, 5)
        && companion_socket::send(fd, b"H", None)
        && companion_socket::send(fd, payload, None)
        && fs::read_all_timeout(fd, &mut status, std::time::Duration::from_secs(5))
        && i32::from_ne_bytes(status) == 1
}

pub fn valid_identity(package: &str, process: &str) -> bool {
    !package.is_empty()
        && package.len() < IDENTITY_CAPACITY
        && package.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        && process.len() < IDENTITY_CAPACITY
        && process
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
}

pub fn encode(package: &str, process: &str) -> Option<[u8; REQUEST_SIZE]> {
    if !valid_identity(package, process) {
        return None;
    }
    let mut data = [0; REQUEST_SIZE];
    data[..package.len()].copy_from_slice(package.as_bytes());
    data[IDENTITY_CAPACITY..IDENTITY_CAPACITY + process.len()].copy_from_slice(process.as_bytes());
    Some(data)
}

pub fn decode(data: &[u8; REQUEST_SIZE]) -> Option<(&str, &str)> {
    fn component(data: &[u8]) -> Option<&str> {
        let end = data.iter().position(|b| *b == 0)?;
        if data[end..].iter().any(|b| *b != 0) {
            return None;
        }
        std::str::from_utf8(&data[..end]).ok()
    }
    let package = component(&data[..IDENTITY_CAPACITY])?;
    let process = component(&data[IDENTITY_CAPACITY..])?;
    valid_identity(package, process).then_some((package, process))
}
