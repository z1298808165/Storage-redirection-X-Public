use super::*;
use crate::platform::proc_identity::{AttributionBoundary, RequestIdentity};

#[cfg(target_os = "linux")]
static DAEMON_TEST: Mutex<()> = Mutex::new(());

#[cfg(target_os = "linux")]
#[test]
fn concurrent_query_gets_its_own_verified_reply() {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    use std::time::Duration;
    let _daemon = DAEMON_TEST.lock().unwrap();
    let address = SocketAddr::from_abstract_name(DAEMON_SOCKET_NAME).unwrap();
    let daemon = UnixDatagram::bind_addr(&address).unwrap();
    daemon
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let guard = client().lock().unwrap_or_else(|error| error.into_inner());
    let pid = std::process::id() as i32;
    let worker = std::thread::spawn(move || query_process_identity(pid, 10123));
    let mut packet = [0; 256];
    let (size, peer) = daemon.recv_from(&mut packet).unwrap();
    let text = std::str::from_utf8(&packet[..size]).unwrap();
    let nonce = text
        .strip_prefix(&format!("Q\t{pid}\t10123\t"))
        .unwrap()
        .parse()
        .unwrap();
    let identity = RequestIdentity {
        pid,
        uid: 10123,
        start_time: 123,
        package: "com.example.owner".into(),
        ..Default::default()
    };
    daemon
        .send_to_addr(reply(&identity, nonce).as_bytes(), &peer)
        .unwrap();
    let received = worker.join().unwrap();
    drop(guard);
    // SAFETY: geteuid 仅读取当前测试进程的有效 UID。
    if unsafe { libc::geteuid() } == 0 {
        assert_eq!(received, Some(identity));
    } else {
        assert!(received.is_none());
    }
}

#[test]
fn identity_query_requires_credentials_and_rejects_foreign_unverified_senders() {
    let pid = std::process::id() as i32;
    // SAFETY: getuid 无参数，只读取测试进程凭据。
    let uid = unsafe { libc::getuid() };
    let query = format!("Q\t{pid}\t{uid}\t9");
    assert!(answer_query(&query, Some((0, pid))).is_some());
    assert!(answer_query(&query, None).is_none());
    assert!(answer_query(&query, Some((uid.saturating_add(10000), pid))).is_none());
    assert!(answer_query(&query, Some((u32::MAX, pid))).is_none());
}

#[test]
fn late_reply_cleanup_respects_the_query_deadline() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixDatagram;
    use std::time::{Duration, Instant};

    let (sender, receiver) = UnixDatagram::pair().unwrap();
    receiver.set_nonblocking(true).unwrap();
    sender.send(b"late").unwrap();
    QueryClient::drain_replies(receiver.as_raw_fd(), Instant::now());
    let mut buffer = [0; 8];
    assert_eq!(receiver.recv(&mut buffer).unwrap(), 4);
    sender.send(b"late").unwrap();
    QueryClient::drain_replies(
        receiver.as_raw_fd(),
        Instant::now() + Duration::from_secs(1),
    );
    assert_eq!(
        receiver.recv(&mut buffer).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn reply_consumed_by_an_inherited_reader_does_not_block_receive() {
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixDatagram;
    use std::time::Duration;

    let (sender, receiver) = UnixDatagram::pair().unwrap();
    sender.send(b"ready").unwrap();
    let mut ready = libc::pollfd {
        fd: receiver.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: ready 指向一个有效 pollfd，等待本测试刚发送的数据。
    assert_eq!(unsafe { libc::poll(&mut ready, 1, 1000) }, 1);
    // fork 继承的读端可以在 poll 就绪后取走数据。
    receiver.try_clone().unwrap().recv(&mut [0; 8]).unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        send.send(recv_from_root(receiver.as_raw_fd(), &mut [0; 8]))
            .unwrap();
    });
    let completed = receive.recv_timeout(Duration::from_millis(200));
    // 阻塞接收的回归也必须解除并回收线程。
    let _ = sender.send(b"release");
    worker.join().unwrap();
    assert_eq!(
        completed.expect("reply receive outlived its deadline"),
        None
    );
}

#[cfg(target_os = "linux")]
#[test]
fn full_daemon_queue_does_not_block_the_query_before_its_deadline() {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixDatagram};
    use std::time::{Duration, Instant};

    let _daemon = DAEMON_TEST.lock().unwrap();
    let address = SocketAddr::from_abstract_name(DAEMON_SOCKET_NAME).unwrap();
    let receiver = UnixDatagram::bind_addr(&address).unwrap();
    let filler = UnixDatagram::unbound().unwrap();
    filler.set_nonblocking(true).unwrap();
    let mut queued = 0;
    loop {
        match filler.send_to_addr(b"queued", &address) {
            Ok(_) => {
                queued += 1;
                assert!(queued < 65536);
            }
            Err(error) => {
                assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
                break;
            }
        }
    }
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut client = QueryClient::default();
        let started = Instant::now();
        let result = client.round_trip(std::process::id() as i32, 10123);
        send.send((result, started.elapsed())).unwrap();
    });
    let completed = receive.recv_timeout(Duration::from_millis(300));
    // 阻塞发送的回归也必须能解除并回收测试线程。
    drop(receiver);
    worker.join().unwrap();
    let (result, elapsed) = completed.expect("full queue blocked the identity query");
    assert!(result.is_none());
    assert!(elapsed < Duration::from_millis(300));
}

fn reply(identity: &RequestIdentity, nonce: u64) -> String {
    format!(
        "{REPLY_PREFIX}{}\t{}\t{}\t{}",
        identity.pid,
        identity.uid,
        nonce,
        serde_json::to_string(identity).unwrap()
    )
}

#[test]
fn query_reply_preserves_executor_boundary_and_raw_task() {
    let identity = RequestIdentity {
        uid: 10123,
        pid: 42,
        start_time: 55,
        native: "su".into(),
        selinux_context: "u:r:su:s0".into(),
        boundary: AttributionBoundary::Privileged,
        ..RequestIdentity::default()
    };
    assert_eq!(
        parse_reply(&reply(&identity, 3), 42, 10123, 3),
        Some(identity.clone())
    );
    assert_eq!(parse_reply(&reply(&identity, 2), 42, 10123, 3), None);
    assert_eq!(parse_reply(&reply(&identity, 3), 43, 10123, 3), None);
    assert_eq!(parse_reply(&reply(&identity, 3), 42, 10124, 3), None);
}

#[test]
fn query_reply_rejects_package_on_terminal_identity_and_invalid_payloads() {
    let identity = RequestIdentity {
        pid: 42,
        uid: 0,
        start_time: 55,
        boundary: AttributionBoundary::Root,
        ..RequestIdentity::default()
    };
    for bad in [
        RequestIdentity {
            package: "com.test.app".into(),
            ..identity.clone()
        },
        RequestIdentity {
            shared_user_id: "android.uid.system".into(),
            ..identity.clone()
        },
        RequestIdentity {
            start_time: 0,
            ..identity.clone()
        },
        RequestIdentity {
            native: "a".repeat(256),
            ..identity.clone()
        },
    ] {
        assert_eq!(parse_reply(&reply(&bad, 3), 42, 0, 3), None);
    }
    for bad in ["", "R\t42\t0\t3", "R\t42\t0\t3\t{}", "R\t42\t0\t3\tnull"] {
        assert_eq!(parse_reply(bad, 42, 0, 3), None);
    }
}
