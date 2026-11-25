use super::{DEFERRED_REAP_CHILDREN, defer_child_reap, reap_deferred_children};

#[test]
fn timed_out_child_is_reaped_after_it_exits_without_touching_live_children() {
    let mut pipe = [-1; 2];
    // SAFETY: pipe 指向两个有效 fd 槽；子进程只读一个字节后退出。
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        // SAFETY: 两端均由本测试创建，子进程只用读端，不运行析构。
        unsafe {
            libc::close(pipe[1]);
            let mut byte = 0u8;
            libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
            libc::_exit(0);
        }
    }
    // SAFETY: 父进程不使用读端，只通过自己的写端释放子进程。
    unsafe { libc::close(pipe[0]) };
    defer_child_reap(child);
    defer_child_reap(child);
    let start_ticks = crate::platform::process_start_time_ticks(child).unwrap();
    DEFERRED_REAP_CHILDREN
        .lock()
        .unwrap()
        .push((child, start_ticks.wrapping_add(1)));
    reap_deferred_children();
    assert_eq!(
        DEFERRED_REAP_CHILDREN
            .lock()
            .unwrap()
            .iter()
            .filter(|(pid, _)| *pid == child)
            .count(),
        1
    );
    // SAFETY: child 由本测试创建；写入有效的一字节缓冲后关闭写端。
    unsafe {
        libc::write(pipe[1], b"x".as_ptr().cast(), 1);
        libc::close(pipe[1]);
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while DEFERRED_REAP_CHILDREN
        .lock()
        .unwrap()
        .iter()
        .any(|(pid, _)| *pid == child)
        && std::time::Instant::now() < deadline
    {
        reap_deferred_children();
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        !DEFERRED_REAP_CHILDREN
            .lock()
            .unwrap()
            .iter()
            .any(|(pid, _)| *pid == child)
    );
    let mut status = 0;
    // SAFETY: child 是本测试的子进程；非阻塞等待确认它已经被回收。
    assert_eq!(
        unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) },
        -1
    );
    assert_eq!(crate::platform::errno::last(), libc::ECHILD);
}
