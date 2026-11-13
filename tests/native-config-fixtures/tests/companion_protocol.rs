#![cfg(any(target_os = "android", target_os = "linux"))]

use native_config_fixtures::hyos_request;
use native_config_fixtures::platform::companion_socket;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};

fn sockets() -> (OwnedFd, OwnedFd) {
    let mut fds = [-1; 2];
    assert_eq!(
        unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_CLOEXEC,
                0,
                fds.as_mut_ptr(),
            )
        },
        0
    );
    unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
}

#[test]
fn inherited_and_invalid_mount_namespaces_are_rejected() {
    use native_config_fixtures::platform::mount_namespace::private_mount_namespace;
    assert!(private_mount_namespace(-1).is_none());
    assert!(private_mount_namespace(i32::MAX).is_none());
    assert!(private_mount_namespace(std::process::id() as i32).is_none());
}

#[test]
fn identity_roundtrip_and_rejected_boundaries() {
    for (package, process) in [
        ("com.example.app", "com.example.app:remote"),
        ("android", ""),
        ("com.example", "native-worker"),
    ] {
        let data = hyos_request::encode(package, process).unwrap();
        assert_eq!(hyos_request::decode(&data), Some((package, process)));
    }
    for package in [
        "",
        ".",
        "..",
        "a..b",
        "../bad",
        "com.app:other",
        "com/app",
        "com.app\0bad",
        "com.应用",
        "a\nb",
    ] {
        assert!(
            hyos_request::encode(package, "process").is_none(),
            "{package:?}"
        );
    }
    assert!(hyos_request::encode(&"a".repeat(255), &"p".repeat(255)).is_some());
    assert!(hyos_request::encode(&"a".repeat(256), "p").is_none());
    assert!(hyos_request::encode("a", &"p".repeat(256)).is_none());
    assert!(hyos_request::encode("a", "bad process").is_none());
    let mut data = hyos_request::encode("a", "p").unwrap();
    data[2] = b'x';
    assert!(hyos_request::decode(&data).is_none());
    assert!(hyos_request::decode(&[b'x'; hyos_request::REQUEST_SIZE]).is_none());
}

#[test]
fn privileged_requests_bind_process_user_and_private_path() {
    use native_config_fixtures::companion_policy::{
        chown_target_package, mount_caller_matches, package_uid_matches,
    };
    let root = libc::ucred {
        pid: 42,
        uid: 0,
        gid: 0,
    };
    assert!(mount_caller_matches(&root, 42));
    assert!(!mount_caller_matches(&root, 43));
    assert!(!mount_caller_matches(
        &libc::ucred { uid: 10123, ..root },
        42
    ));
    assert!(package_uid_matches(10123, 10123));
    assert!(package_uid_matches(1010123, 10123));
    assert!(package_uid_matches(1000, 1000));
    for (uid, package_uid) in [(-1, 10123), (10123, -1), (99000, 10123), (10123, 10456)] {
        assert!(!package_uid_matches(uid, package_uid));
    }
    for mode in [libc::S_IFREG, libc::S_IFDIR] {
        for path in [
            "/data/media/10/Android/data/com.example/file",
            "/storage/emulated/10/Android/media/com.example/file",
        ] {
            assert_eq!(
                chown_target_package(path, 1010123, mode).as_deref(),
                Some("com.example")
            );
        }
    }
    for path in [
        "/data/system/packages.xml",
        "/storage/emulated/0/Android/data/com.example/file",
        "/storage/emulated/10/Download/file",
        "/data/media/10/Android/data/com.example/file (deleted)",
        "/data/media/10/Android/data/com.example/../../../../system/file",
    ] {
        assert!(
            chown_target_package(path, 1010123, libc::S_IFREG).is_none(),
            "{path}"
        );
    }
    for mode in [
        libc::S_IFIFO,
        libc::S_IFSOCK,
        libc::S_IFCHR,
        libc::S_IFBLK,
        libc::S_IFLNK,
    ] {
        assert!(
            chown_target_package(
                "/data/media/10/Android/data/com.example/file",
                1010123,
                mode
            )
            .is_none()
        );
    }
}

#[test]
fn credentials_survive_sending_before_receiver_configuration() {
    let (client, server) = sockets();
    assert!(companion_socket::send(client.as_raw_fd(), b"H", None));
    assert!(companion_socket::configure(server.as_raw_fd(), 1));
    let (length, credentials, fd) =
        companion_socket::receive(server.as_raw_fd(), &mut [0; 1]).unwrap();
    assert_eq!(length, 1);
    assert_eq!(credentials.pid, unsafe { libc::getpid() });
    assert_eq!(credentials.uid, unsafe { libc::geteuid() });
    assert!(fd.is_none());
}

#[test]
fn credentials_identify_sender_instead_of_socket_creator() {
    let (client, server) = sockets();
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        let ok = companion_socket::send(client.as_raw_fd(), b"H", None);
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }
    drop(client);
    assert!(companion_socket::configure(server.as_raw_fd(), 1));
    let (_, credentials, _) = companion_socket::receive(server.as_raw_fd(), &mut [0; 1]).unwrap();
    assert_eq!(credentials.pid, child);
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
    assert_eq!(status, 0);
}

#[test]
fn transferred_fd_is_owned_and_close_on_exec() {
    let (client, server) = sockets();
    let file = std::fs::File::open("/dev/null").unwrap();
    assert!(companion_socket::configure(server.as_raw_fd(), 1));
    assert!(companion_socket::send(
        client.as_raw_fd(),
        &10000i32.to_ne_bytes(),
        Some(file.as_raw_fd())
    ));
    let mut payload = [0; 4];
    let (length, _, fd) = companion_socket::receive(server.as_raw_fd(), &mut payload).unwrap();
    assert_eq!(length, 4);
    assert_eq!(i32::from_ne_bytes(payload), 10000);
    let fd = fd.unwrap();
    assert_ne!(fd.as_raw_fd(), file.as_raw_fd());
    assert_ne!(
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
        0
    );
    assert_eq!(
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
            .unwrap()
            .to_str(),
        Some("/dev/null")
    );
}

#[test]
fn extra_and_truncated_rights_are_rejected_without_fd_leaks() {
    let mut pipe = [-1; 2];
    assert_eq!(
        unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
        0
    );
    let (read, _write) = unsafe { (OwnedFd::from_raw_fd(pipe[0]), OwnedFd::from_raw_fd(pipe[1])) };
    let pipe_path = std::fs::read_link(format!("/proc/self/fd/{}", read.as_raw_fd())).unwrap();
    let count = || {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| std::fs::read_link(entry.path()).is_ok_and(|path| path == pipe_path))
            .count()
    };
    let baseline = count();
    for fd_count in [2, 100] {
        let (client, server) = sockets();
        assert!(companion_socket::configure(server.as_raw_fd(), 1));
        unsafe {
            let mut control = [0usize; 128];
            let mut byte = b'F';
            let mut iov = libc::iovec {
                iov_base: (&mut byte as *mut u8).cast(),
                iov_len: 1,
            };
            let mut msg: libc::msghdr = std::mem::zeroed();
            msg.msg_iov = &mut iov;
            msg.msg_iovlen = 1;
            msg.msg_control = control.as_mut_ptr().cast();
            msg.msg_controllen = libc::CMSG_SPACE((fd_count * size_of::<i32>()) as _) as _;
            let cmsg = libc::CMSG_FIRSTHDR(&msg);
            (*cmsg).cmsg_level = libc::SOL_SOCKET;
            (*cmsg).cmsg_type = libc::SCM_RIGHTS;
            (*cmsg).cmsg_len = libc::CMSG_LEN((fd_count * size_of::<i32>()) as _) as _;
            for index in 0..fd_count {
                libc::CMSG_DATA(cmsg)
                    .cast::<i32>()
                    .add(index)
                    .write_unaligned(read.as_raw_fd());
            }
            assert_eq!(
                libc::sendmsg(client.as_raw_fd(), &msg, libc::MSG_NOSIGNAL),
                1
            );
        }
        assert!(companion_socket::receive(server.as_raw_fd(), &mut [0; 1]).is_err());
        assert_eq!(
            count(),
            baseline,
            "leaked descriptors from {fd_count} rights"
        );
    }
}

#[test]
fn missing_credentials_and_invalid_passed_fd_are_rejected() {
    let (client, server) = sockets();
    assert!(native_config_fixtures::platform::fs::write_all(
        client.as_raw_fd(),
        b"F"
    ));
    assert!(companion_socket::receive(server.as_raw_fd(), &mut [0; 1]).is_err());
    assert!(!companion_socket::send(client.as_raw_fd(), b"F", Some(-1)));
}

#[test]
fn short_stream_writes_complete_without_retransferring_the_descriptor() {
    let (client, server) = sockets();
    assert!(companion_socket::configure(client.as_raw_fd(), 1));
    assert!(companion_socket::configure(server.as_raw_fd(), 2));
    let capacity: libc::c_int = 4096;
    assert_eq!(
        unsafe {
            libc::setsockopt(
                client.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&capacity as *const libc::c_int).cast(),
                std::mem::size_of_val(&capacity) as _,
            )
        },
        0
    );
    let payload = vec![b'x'; 128 * 1024];
    let receiver = std::thread::spawn(move || {
        // 先让 sendmsg 在小发送缓冲上超时短写，再排空续传数据。
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let mut total = 0;
        let mut descriptors = 0;
        let mut buffer = [0; 4096];
        while total < 128 * 1024 {
            let (size, _, descriptor) =
                companion_socket::receive(server.as_raw_fd(), &mut buffer).unwrap();
            assert!(buffer[..size].iter().all(|byte| *byte == b'x'));
            total += size;
            descriptors += usize::from(descriptor.is_some());
        }
        assert_eq!(descriptors, 1);
    });
    let file = std::fs::File::open("/dev/null").unwrap();
    assert!(companion_socket::send(
        client.as_raw_fd(),
        &payload,
        Some(file.as_raw_fd())
    ));
    receiver.join().unwrap();
}

#[test]
fn eof_timeout_and_broken_peer_are_failures() {
    let (client, server) = sockets();
    assert!(companion_socket::configure(server.as_raw_fd(), 1));
    assert!(companion_socket::receive(server.as_raw_fd(), &mut [0; 1]).is_err());
    drop(client);
    assert!(companion_socket::receive(server.as_raw_fd(), &mut [0; 1]).is_err());
    assert!(!companion_socket::send(server.as_raw_fd(), b"x", None));
    assert!(!companion_socket::send(server.as_raw_fd(), b"", None));
    assert!(!companion_socket::configure(-1, 1));
}

fn exchange(status: &[u8]) -> bool {
    let (client, server) = sockets();
    let status = status.to_vec();
    let payload = hyos_request::encode("com.example.app", "com.example.app:remote").unwrap();
    let thread = std::thread::spawn(move || {
        assert!(companion_socket::configure(server.as_raw_fd(), 1));
        let mut request_type = [0; 1];
        let (_, _, fd) = companion_socket::receive(server.as_raw_fd(), &mut request_type).unwrap();
        assert!(fd.is_none());
        assert_eq!(&request_type, b"H");
        let mut received = [0; hyos_request::REQUEST_SIZE];
        assert!(native_config_fixtures::platform::fs::read_all_timeout(
            server.as_raw_fd(),
            &mut received,
            std::time::Duration::from_secs(1)
        ));
        assert_eq!(received, payload);
        if !status.is_empty() {
            assert!(companion_socket::send(server.as_raw_fd(), &status, None));
        }
    });
    let result = hyos_request::specialize(client.into_raw_fd(), &payload);
    thread.join().unwrap();
    result
}

#[test]
fn specialization_requires_complete_success_status() {
    assert!(exchange(&1i32.to_ne_bytes()));
    assert!(!exchange(&0i32.to_ne_bytes()));
    assert!(!exchange(&(-1i32).to_ne_bytes()));
    assert!(!exchange(&2i32.to_ne_bytes()));
    assert!(!exchange(&[1]));
    assert!(!exchange(&[]));
}
