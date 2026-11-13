use std::io;
use std::mem::{self, size_of, size_of_val};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};

// usize 提供 cmsghdr 对齐；额外空间接收并回收畸形请求携带的多余 FD。
const CONTROL_WORDS: usize = 32;

pub fn configure(fd: RawFd, timeout_seconds: i64) -> bool {
    let enabled: libc::c_int = 1;
    let timeout = libc::timeval {
        tv_sec: timeout_seconds as _,
        tv_usec: 0,
    };
    // SAFETY: 三个选项均读取栈上值，长度与类型匹配。
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            &enabled as *const _ as _,
            size_of_val(&enabled) as _,
        ) == 0
            && libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &timeout as *const _ as _,
                size_of_val(&timeout) as _,
            ) == 0
            && libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_SNDTIMEO,
                &timeout as *const _ as _,
                size_of_val(&timeout) as _,
            ) == 0
    }
}

/// 每次消息显式携带发送时的内核凭据，避免把代理创建 socketpair 时的身份当作调用者。
pub fn send(fd: RawFd, payload: &[u8], passed_fd: Option<RawFd>) -> bool {
    if payload.is_empty() {
        return false;
    }
    let mut control = [0usize; CONTROL_WORDS];
    let mut iov = libc::iovec {
        iov_base: payload.as_ptr() as _,
        iov_len: payload.len(),
    };
    // SAFETY: msghdr 全零有效，所有指针仅在 sendmsg 期间借用。
    unsafe {
        let credentials = libc::ucred {
            pid: libc::getpid(),
            uid: libc::geteuid(),
            gid: libc::getegid(),
        };
        let mut msg: libc::msghdr = mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as _;
        msg.msg_controllen = libc::CMSG_SPACE(size_of::<libc::ucred>() as _) as _;
        if passed_fd.is_some() {
            msg.msg_controllen += libc::CMSG_SPACE(size_of::<RawFd>() as _) as usize;
        }
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_CREDENTIALS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<libc::ucred>() as _) as _;
        libc::CMSG_DATA(cmsg)
            .cast::<libc::ucred>()
            .write_unaligned(credentials);
        if let Some(passed_fd) = passed_fd {
            let rights = libc::CMSG_NXTHDR(&msg, cmsg);
            (*rights).cmsg_level = libc::SOL_SOCKET;
            (*rights).cmsg_type = libc::SCM_RIGHTS;
            (*rights).cmsg_len = libc::CMSG_LEN(size_of::<RawFd>() as _) as _;
            libc::CMSG_DATA(rights)
                .cast::<RawFd>()
                .write_unaligned(passed_fd);
        }
        loop {
            let sent = libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL);
            if sent < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            if sent <= 0 {
                return false;
            }
            let mut offset = sent as usize;
            // 流式 socket 可能短写；凭据与 FD 已随首段转交，不得在续写时重复发送 FD。
            while offset < payload.len() {
                let sent = libc::send(
                    fd,
                    payload[offset..].as_ptr().cast(),
                    payload.len() - offset,
                    libc::MSG_NOSIGNAL,
                );
                if sent < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if sent <= 0 {
                    return false;
                }
                offset += sent as usize;
            }
            return true;
        }
    }
}

/// 接收凭据与至多一个 FD；失败时自动关闭已经接收的 FD。
pub fn receive(fd: RawFd, payload: &mut [u8]) -> io::Result<(usize, libc::ucred, Option<OwnedFd>)> {
    let mut control = [0usize; CONTROL_WORDS];
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr() as _,
        iov_len: payload.len(),
    };
    // SAFETY: 缓冲按 cmsghdr 对齐，内核只写入给定容量；FD 的所有权由 recvmsg 转交。
    unsafe {
        let mut msg: libc::msghdr = mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr() as _;
        msg.msg_controllen = size_of_val(&control);
        let received = loop {
            let result = libc::recvmsg(fd, &mut msg, libc::MSG_CMSG_CLOEXEC);
            if result >= 0 {
                break result as usize;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        };
        let mut credentials = None;
        let mut passed_fd = None;
        let mut invalid = msg.msg_flags & (libc::MSG_CTRUNC | libc::MSG_TRUNC) != 0;
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            let length = (*cmsg).cmsg_len as usize;
            let header = libc::CMSG_LEN(0) as usize;
            if length < header {
                invalid = true;
                break;
            }
            if (*cmsg).cmsg_level == libc::SOL_SOCKET {
                let data = libc::CMSG_DATA(cmsg);
                if (*cmsg).cmsg_type == libc::SCM_CREDENTIALS
                    && length == header + size_of::<libc::ucred>()
                {
                    if credentials.is_some() {
                        invalid = true;
                    }
                    credentials = Some(data.cast::<libc::ucred>().read_unaligned());
                } else if (*cmsg).cmsg_type == libc::SCM_RIGHTS {
                    for offset in (0..length - header).step_by(size_of::<RawFd>()) {
                        if offset + size_of::<RawFd>() > length - header {
                            invalid = true;
                            break;
                        }
                        let raw = data.add(offset).cast::<RawFd>().read_unaligned();
                        let owned = OwnedFd::from_raw_fd(raw);
                        if passed_fd.is_some() {
                            invalid = true;
                        } else {
                            passed_fd = Some(owned);
                        }
                    }
                }
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
        let credentials = credentials.filter(|cred| cred.pid > 0 && cred.uid != u32::MAX);
        if invalid || received == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid companion message",
            ));
        }
        match credentials {
            Some(credentials) => Ok((received, credentials, passed_fd)),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "missing companion credentials",
            )),
        }
    }
}
