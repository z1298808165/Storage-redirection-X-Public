use super::reply_identity_query;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixDatagram;
use std::time::Duration;

#[test]
fn full_identity_reply_queue_does_not_stall_the_logger() {
    let receiver = UnixDatagram::unbound().unwrap();
    let sender = UnixDatagram::unbound().unwrap();
    let name = format!("srx-reply-test-{}", std::process::id());
    // SAFETY: sockaddr_un 是普通 C 结构，零初始化保留抽象名字的首字节。
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    address.sun_family = libc::AF_UNIX as _;
    for (dest, source) in address.sun_path[1..].iter_mut().zip(name.bytes()) {
        *dest = source as _;
    }
    let length = (std::mem::size_of::<libc::sa_family_t>() + 1 + name.len()) as libc::socklen_t;
    // SAFETY: address 与 length 覆盖同一已初始化地址，receiver 持有有效 fd。
    assert_eq!(
        unsafe {
            libc::bind(
                receiver.as_raw_fd(),
                (&address as *const libc::sockaddr_un).cast(),
                length,
            )
        },
        0
    );
    // SAFETY: getuid 无参数，只读取当前任务的真实 UID。
    let uid = unsafe { libc::getuid() };
    let query = format!("Q\t{}\t{uid}\t1", std::process::id());
    let target = (address, length);
    receiver
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    reply_identity_query(
        sender.as_raw_fd(),
        &query,
        Some((uid, std::process::id() as i32)),
        Some(&target),
    );
    let mut payload = [0; 2048];
    let size = receiver.recv(&mut payload).unwrap();
    assert!(
        std::str::from_utf8(&payload[..size])
            .unwrap()
            .starts_with("R\t")
    );
    let mut queued = 0;
    loop {
        // SAFETY: 发送固定字节，目标地址来自已成功 bind 的同一地址。
        let sent = unsafe {
            libc::sendto(
                sender.as_raw_fd(),
                b"queued".as_ptr().cast(),
                6,
                libc::MSG_DONTWAIT,
                (&target.0 as *const libc::sockaddr_un).cast(),
                target.1,
            )
        };
        if sent < 0 {
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            break;
        }
        queued += 1;
        assert!(queued < 65536);
    }
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        reply_identity_query(
            sender.as_raw_fd(),
            &query,
            Some((uid, std::process::id() as i32)),
            Some(&target),
        );
        send.send(()).unwrap();
    });
    let completed = receive.recv_timeout(Duration::from_millis(300));
    drop(receiver);
    worker.join().unwrap();
    completed.expect("full queue blocked the logger reply");
}
