use crate::platform::inotify::Event;
use crate::platform::paths;
use libc::{
    IN_CLOSE_WRITE, IN_CREATE, IN_DELETE, IN_MOVED_FROM, IN_MOVED_TO, IN_Q_OVERFLOW, c_int,
    inotify_add_watch, inotify_init1,
};
use std::collections::BTreeSet;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

const EVENT_MASK: u32 = IN_CREATE
    | IN_DELETE
    | IN_CLOSE_WRITE
    | IN_MOVED_FROM
    | IN_MOVED_TO
    | libc::IN_MOVE_SELF
    | libc::IN_DELETE_SELF;

static INOTIFY_FD: AtomicI32 = AtomicI32::new(-1);
/// 事件通道是否可用：fd 有效、配置目录 watch 建立成功、读取线程正在运行。
///
/// 它为真时配置变更**完全由事件驱动**，调用方的指纹复查可以彻底关掉（因为没有需要
/// 复查的东西：事件不丢，溢出了也会被当作全量变更上报）；它转假（init 失败、watch
/// 建立失败、读取线程因 fd 失效退出）时调用方必须退回周期指纹复查。
static EVENTS_LIVE: AtomicBool = AtomicBool::new(false);
/// 变更回调：事件到达并发布后额外通知一次。
///
/// 这是给「没有拦截热路径可以搭车」的进程用的——桥接进程只在用户操作时才走决策路径，
/// 空闲期不会调用 [`poll_changed_with_packages`]，因此不能靠热路径取走结论，只能由
/// 事件线程直接回调（回调在事件线程上执行，不在任何 hook 热路径里）。
static CHANGE_CALLBACK: Mutex<Option<ChangeCallback>> = Mutex::new(None);

type ChangeCallback = Arc<dyn Fn(&ChangeKind) + Send + Sync>;
static APPS_WATCH_FD: AtomicI32 = AtomicI32::new(-1);
/// config 目录自身的 watch id。
///
/// 只用于识别 `IN_IGNORED`（目录被删/被替换后内核自动摘除 watch）：那时必须把
/// `EVENTS_LIVE` 置假退回指纹复查，否则「事件再也不来」会表现为热加载静默失效。
static CONFIG_WATCH_FD: AtomicI32 = AtomicI32::new(-1);
static CHANGED_PACKAGES: OnceLock<Mutex<BTreeSet<String>>> = OnceLock::new();
/// init 时记录的配置目录：只用于「apps 目录晚创建时补挂 watch」。
///
/// 用 `OnceLock` 而不是向 `SettingsHub` 取，是为了在事件路径上**不取任何锁**——
/// 事件路径既可能跑在读取线程，也可能跑在 daemon 主循环里。
static WATCHED_CONFIG_DIR: OnceLock<String> = OnceLock::new();

fn changed_packages() -> &'static Mutex<BTreeSet<String>> {
    CHANGED_PACKAGES.get_or_init(|| Mutex::new(BTreeSet::new()))
}
// 初始化 inotify 并添加监听，返回 fd（用于 exempt）
// 必须在 pre_app_specialize 阶段调用（此时有 root 权限）
pub fn init(config_dir: &str) -> i32 {
    // 幂等：同一个进程只保留一个实例。重复 init 会建出**第二个** fd 并覆盖全局，
    // 而已经启动的读取线程仍在读旧 fd —— 配置事件再也读不到，而 EVENTS_LIVE 依旧是
    // 真（没有指纹兜底），等于热加载静默失效。
    let existing = INOTIFY_FD.load(Ordering::Acquire);
    if existing >= 0 {
        log::debug!("config watcher already initialized fd={}", existing);
        return existing;
    }
    let fd = unsafe { inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        log::warn!("inotify init failed");
        return -1;
    }

    let _ = WATCHED_CONFIG_DIR.set(config_dir.to_string());
    let config_wd = add_watch_with_id(fd, config_dir).unwrap_or(-1);
    if config_wd < 0 {
        // 这里是 info 而不是 warn：应用进程（zygote 域）对 `/data/adb/modules/...`
        // 没有 `watch` 权限，SELinux 每次都拒绝（真机 audit：`denied { watch } for
        // comm="main" ... scontext=u:r:zygote:s0`）。这是环境事实、不是异常，事件
        // 通道只在 daemon（root 域）成立；应用侧由 `EVENTS_LIVE` 转假继续走指纹复查。
        // 真机实测每个应用启动因此固定刷一条 warn，属于纯噪音。
        log::info!(
            "watch config dir unavailable {} errno={} (app-domain SELinux denies watch; events_live stays false)",
            config_dir,
            crate::platform::errno::last()
        );
    }
    let dir_watched = config_wd >= 0;

    let apps_dir = paths::join(config_dir, super::source::APPS_CONFIG_DIR);
    let apps_wd = add_watch_with_id(fd, &apps_dir).unwrap_or(-1);
    if apps_wd < 0 {
        log::debug!("apps dir missing or unwatchable");
    }

    APPS_WATCH_FD.store(apps_wd, Ordering::Release);
    CONFIG_WATCH_FD.store(config_wd, Ordering::Release);
    INOTIFY_FD.store(fd, Ordering::Release);
    // 事件通道可信的前提是**两个** watch 都在位：
    // - config 目录 watch 供 global.json 等根级事件；
    // - apps 目录 watch 供 `apps/<pkg>.json` 事件（inotify 不递归，缺了它单个应用的
    //   规则变化不会产生任何事件）。
    // 少任何一个都要保持 EVENTS_LIVE 为假，让调用方继续走指纹复查；apps 目录在早期
    // 尚未创建时由 `drain_changed_packages` 在它被创建的那一刻补挂（见那里的处理）。
    EVENTS_LIVE.store(dir_watched && apps_wd >= 0, Ordering::Release);
    log::info!(
        "config watcher ready fd={} dir_watch={} apps_watch={} events_live={}",
        fd,
        dir_watched,
        apps_wd >= 0,
        dir_watched && apps_wd >= 0
    );
    fd
}

/// 为 apps 目录补挂 watch（目录晚于 init 创建时用）。返回是否成功。
fn attach_apps_watch(fd: c_int, config_dir: &str) -> bool {
    let apps_dir = paths::join(config_dir, super::source::APPS_CONFIG_DIR);
    let Some(wd) = add_watch_with_id(fd, &apps_dir) else {
        return false;
    };
    APPS_WATCH_FD.store(wd, Ordering::Release);
    // 只有 config 目录自身的 watch 仍在位时，「两个 watch 齐了」才成立。
    if CONFIG_WATCH_FD.load(Ordering::Acquire) >= 0 {
        EVENTS_LIVE.store(true, Ordering::Release);
    }
    log::info!("config watcher apps watch attached wd={}", wd);
    true
}

/// 事件通道是否可靠。
pub fn events_live() -> bool {
    EVENTS_LIVE.load(Ordering::Acquire)
}

/// 注册变更回调（覆盖旧回调）。回调在事件线程上执行，必须自行保证幂等与去抖。
pub fn set_change_callback(callback: ChangeCallback) {
    if let Ok(mut slot) = CHANGE_CALLBACK.lock() {
        *slot = Some(callback);
    }
}

fn notify_change(kind: &ChangeKind) {
    let callback = CHANGE_CALLBACK
        .lock()
        .ok()
        .and_then(|slot| slot.as_ref().map(Arc::clone));
    if let Some(callback) = callback {
        callback(kind);
    }
}

/// 在同一个 inotify 实例上追加一个**只用于唤醒**的目录 watch。
///
/// 用途：daemon 主循环需要在「运行开关被 WebUI 改写」时立刻醒来。开关是模块目录下的
/// 一个标记文件，由 WebUI 的 shell 直接写，没有任何 IPC 通道，因此唯一的事件源就是
/// 目录变化本身。
///
/// 只加在 daemon 侧：hook 进程若也监听模块目录，会被自己写 `tmp/` 的动作反复唤醒。
/// inotify 不递归，所以这个 watch 只会收到**模块目录直接子项**的增删改名事件——
/// `logs/`、`tmp/` 内部的写入不会触发它（真机实证：稳态下几乎为零）。
///
/// 返回是否添加成功；失败时调用方必须退回固定节拍的兜底探询。
#[allow(dead_code)] // quality-allow(lint-suppression): 仅 daemon 二进制目标调用；cdylib 进程不监听模块目录。
pub fn add_wakeup_watch(path: &str) -> bool {
    let fd = INOTIFY_FD.load(Ordering::Acquire);
    if fd < 0 {
        return false;
    }
    let added = add_watch(fd, path);
    log::info!("config watcher wakeup watch path={} ok={}", path, added);
    added
}

/// 返回 daemon 配置 watcher fd，供阻塞式事件循环直接等待。
#[allow(dead_code)] // quality-allow(lint-suppression): 仅 daemon 二进制目标消费，cdylib 目标不运行主循环。
pub fn event_fd() -> i32 {
    INOTIFY_FD.load(Ordering::Acquire)
}

// inotify_event 需要 4 字节对齐；内核保证每个事件总长度是 sizeof(int) 的倍数，
// 因此缓冲区起始 4 字节对齐后，后续每个事件也满足对齐要求。
// 使用 4096 字节容纳多个事件，避免 1024 字节时单次 read 截断。
#[repr(align(4))]
struct InotifyBuf([u8; 4096]);

impl InotifyBuf {
    fn new() -> Self {
        Self([0u8; 4096])
    }
}

/// 一次配置变更的范围。
///
/// 热路径取走的是后台线程已经解析好的结论，本身不做任何系统调用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    None,
    /// 全局配置发生变化；同时携带同批次发现的 app 包名，用于优先处理。
    Full(Vec<String>),
    Apps(Vec<String>),
}

/// 后台事件读取线程是否在跑（fork 之后子进程里没有这个线程，必须能重新拉起）。
static EVENT_READER_ALIVE: AtomicBool = AtomicBool::new(false);
/// 拉起读取线程时的串行化锁。
///
/// 这里刻意不用 `Once`：进程 fork 之后子进程里并不存在这个线程，而 `Once` 仍处于
/// 「已完成」状态，会导致子进程永远起不来读取者 → 配置变更静默失效。改用
/// `try_lock` 而不是 `lock`：fork 的瞬间若锁正被上一代的线程持有，子进程 `lock`
/// 会永久阻塞。
static EVENT_READER_SPAWN_LOCK: Mutex<()> = Mutex::new(());
/// 已发布但尚未被热路径取走的变更。
static PENDING_CHANGE: Mutex<Option<ChangeKind>> = Mutex::new(None);
/// 「槽里有待取变更」的廉价标志。
///
/// 热路径（每次拦截操作）读它一次原子变量即可返回，只有真的有变更时才去抢互斥量。
/// 标志与槽内容在**同一临界区**内一起更新，因此不存在「槽里有值但标志为假」的窗口。
static PENDING_READY: AtomicBool = AtomicBool::new(false);

/// 首次在热路径被调用时启动读取线程（懒启动：只有真的会走热路径的进程才付这个线程）。
fn ensure_event_reader() {
    if EVENT_READER_ALIVE.load(Ordering::Acquire) {
        return;
    }
    let Ok(_guard) = EVENT_READER_SPAWN_LOCK.try_lock() else {
        return;
    };
    if EVENT_READER_ALIVE.load(Ordering::Acquire) {
        return;
    }
    let fd = INOTIFY_FD.load(Ordering::Acquire);
    if fd < 0 {
        return;
    }
    EVENT_READER_ALIVE.store(true, Ordering::Release);
    let spawned = std::thread::Builder::new()
        .name("srx-cfg-events".to_string())
        // 该线程只做 poll + 一次 read，缓冲区 4KiB：给 64KiB 栈足够，避免每个进程
        // 预留默认 2MiB 栈虚拟空间。
        .stack_size(64 * 1024)
        .spawn(move || {
            // 只有 watch 仍在位时才算在线。线程启动本身不能弥补 SELinux 拒绝目录
            // watch 的情况，否则会关闭指纹兜底并静默漏掉配置变更。
            EVENTS_LIVE.store(
                CONFIG_WATCH_FD.load(Ordering::Acquire) >= 0
                    && APPS_WATCH_FD.load(Ordering::Acquire) >= 0,
                Ordering::Release,
            );
            event_reader_loop(fd);
            EVENT_READER_ALIVE.store(false, Ordering::Release);
        });
    if let Err(error) = spawned {
        // 线程起不来 = 没人读事件：立刻降级，让调用方退回指纹复查。
        EVENT_READER_ALIVE.store(false, Ordering::Release);
        EVENTS_LIVE.store(false, Ordering::Release);
        log::warn!("config event reader spawn failed: {}", error);
    }
}

fn event_reader_loop(fd: i32) {
    loop {
        let mut poll_fd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll_fd 指向本栈帧上已初始化的 pollfd；-1 表示无限等待。
        let ready = unsafe { libc::poll(&mut poll_fd, 1, -1) };
        if ready < 0 {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            if errno == libc::EINTR {
                continue;
            }
            // fd 失效（被关闭/替换）：线程退出，热路径此后恒返 None，配置变更退回
            // 指纹复查兜底，不会忙循环。
            log::warn!("config event reader stop errno={}", errno);
            break;
        }
        if (poll_fd.revents & (libc::POLLNVAL | libc::POLLERR | libc::POLLHUP)) != 0 {
            // poll 的返回值仍可能为正；持续忽略错误位会使失效 fd 无限忙循环。
            log::warn!("配置事件读取停止 revents={}", poll_fd.revents);
            break;
        }
        if ready == 0 || (poll_fd.revents & libc::POLLIN) == 0 {
            continue;
        }
        let kind = drain_changed_packages();
        if !matches!(kind, ChangeKind::None) {
            notify_change(&kind);
        }
        publish_change(kind);
    }
    // poll 错误位或不可恢复错误意味着事件通道不再可信；调用方必须恢复指纹复查。
    EVENTS_LIVE.store(false, Ordering::Release);
    let _ = INOTIFY_FD.compare_exchange(fd, -1, Ordering::AcqRel, Ordering::Acquire);
}

/// 发布一次变更到待取槽，与尚未取走的结论合并（Full 优先，包名取并集）。
///
/// 先归一化成「(包名集合, 是否全量)」再合并：`ChangeKind` 只有三个变体，逐个组合
/// 写六个 match 分支既啰嗦又容易漏，而 `Full` 本来就是 `Apps` 的超集，统一表达即可。
fn publish_change(kind: ChangeKind) {
    let (mut packages, mut full) = match kind {
        ChangeKind::None => return,
        ChangeKind::Full(packages) => (packages, true),
        ChangeKind::Apps(packages) => (packages, false),
    };
    let Ok(mut slot) = PENDING_CHANGE.lock() else {
        return;
    };
    match slot.take() {
        // 槽里已有的结论同样归一化后并入：任一侧是全量，合并结果就是全量。
        Some(ChangeKind::Full(existing)) => {
            full = true;
            packages.extend(existing);
        }
        Some(ChangeKind::Apps(existing)) => packages.extend(existing),
        Some(ChangeKind::None) | None => {}
    }
    let merged = if full {
        ChangeKind::Full(packages)
    } else {
        ChangeKind::Apps(packages)
    };
    *slot = Some(merged);
    // 与写入同一临界区内置位：读方要么看到值为真（随后必然取走），要么看到假（此时
    // 槽里一定是空的）。
    PENDING_READY.store(true, Ordering::Release);
}

fn take_published_change() -> ChangeKind {
    // 无变更时只做一次原子读：这是每个被拦截操作都会走到的路径。
    if !PENDING_READY.load(Ordering::Acquire) {
        return ChangeKind::None;
    }
    let Ok(mut slot) = PENDING_CHANGE.lock() else {
        return ChangeKind::None;
    };
    let value = slot.take().unwrap_or(ChangeKind::None);
    if slot.is_none() {
        PENDING_READY.store(false, Ordering::Release);
    }
    value
}

/// 立刻启动事件读取线程（幂等）。
///
/// 热路径会在第一次调用 [`poll_changed_with_packages`] 时懒启动；没有热路径可搭车的
/// 进程（桥接进程）用本入口主动启动，从而完全不需要周期指纹。
pub fn start_event_reader() {
    ensure_event_reader();
}

pub fn poll_changed() -> bool {
    !matches!(poll_changed_with_packages(), ChangeKind::None)
}

/// 热路径读取配置变更：**取走后台线程已发布的结论**，不做任何系统调用。
///
/// 这里以前是「每次拦截操作做一次非阻塞 read(inotify fd)」：即使节流到 10ms，满负载
/// 下每个进程每秒仍有上百次系统调用，而这些调用的唯一产出是「大部分时候什么都没变」。
/// 现在 inotify fd 由一个阻塞在 `poll` 上的后台线程独占读取（空闲零唤醒），事件到达后
/// 去抖、解析、发布到这里；热路径只剩一次短锁 + `take()`。
pub fn poll_changed_with_packages() -> ChangeKind {
    ensure_event_reader();
    take_published_change()
}

/// 直接读 fd 的版本，只给**事件驱动的等待方**使用（daemon 主循环）。
///
/// 那个循环本身就阻塞在这个 inotify fd 上，被唤醒时队列里一定有事件，因此必须当场把
/// 事件读出来——若交给后台线程（或加任何节流）而把事件挡回去，poll 会立刻再次返回，
/// 退化成忙循环。热路径必须用 [`poll_changed_with_packages`]（取已发布的结论）。
#[allow(dead_code)] // quality-allow(lint-suppression): 仅 daemon 二进制目标消费，cdylib 目标不运行主循环。
pub fn poll_changed_unthrottled_with_packages() -> ChangeKind {
    if INOTIFY_FD.load(Ordering::Acquire) < 0 {
        return ChangeKind::None;
    }
    drain_changed_packages()
}

fn drain_changed_packages() -> ChangeKind {
    let fd = INOTIFY_FD.load(Ordering::Acquire);

    let mut buf = InotifyBuf::new();
    // SAFETY: 读入 buf 自身的完整区间，长度取自同一数组，不会越界。
    let len = unsafe { libc::read(fd, buf.0.as_mut_ptr() as *mut _, buf.0.len()) };
    if len <= 0 {
        return ChangeKind::None;
    }

    let mut changed = false;
    let mut full_change = false;
    crate::platform::inotify::for_each_event(&buf.0[..len as usize], |event| {
        if (event.mask & (libc::IN_IGNORED | libc::IN_MOVE_SELF | libc::IN_DELETE_SELF)) != 0 {
            // 被监视目录被替换或删除后，watch 可能仍绑定旧 inode 而不产生 IN_IGNORED；
            // 立即标记事件通道不可信，靠降级指纹复查发现新目录并恢复功能。
            if event.wd == APPS_WATCH_FD.load(Ordering::Acquire) {
                APPS_WATCH_FD.store(-1, Ordering::Release);
                EVENTS_LIVE.store(false, Ordering::Release);
                log::warn!("config watcher apps watch removed by kernel");
            } else if event.wd == CONFIG_WATCH_FD.load(Ordering::Acquire) {
                CONFIG_WATCH_FD.store(-1, Ordering::Release);
                EVENTS_LIVE.store(false, Ordering::Release);
                log::warn!("config watcher dir watch removed by kernel");
            }
            return;
        }
        if (event.mask & IN_Q_OVERFLOW) != 0 {
            // 队列溢出会丢掉配置事件。这正是「不需要周期指纹」的底气所在：溢出在这里
            // 就被识别成一次全量变更，调用方按全量重新比对即可，不存在「悄悄漏掉」。
            changed = true;
            full_change = true;
            return;
        }
        // apps 目录可能在 init 时还不存在（早期启动/全新安装）：它一被创建就补挂
        // watch，否则单个应用的规则变化永远不会有事件。补挂后按「全量变更」处理，
        // 因为紧接着就会有文件被写进这个新目录。
        if (event.mask & libc::IN_ISDIR) != 0
            && (event.mask & (IN_CREATE | IN_MOVED_TO)) != 0
            && APPS_WATCH_FD.load(Ordering::Acquire) < 0
            && std::str::from_utf8(event.name)
                .unwrap_or_default()
                .trim_end_matches('\0')
                == super::source::APPS_CONFIG_DIR
        {
            let attached = WATCHED_CONFIG_DIR
                .get()
                .is_some_and(|config_dir| attach_apps_watch(fd, config_dir));
            changed = true;
            if !attached {
                EVENTS_LIVE.store(false, Ordering::Release);
            }
            full_change = true;
            return;
        }
        if !is_config_event(event) {
            return;
        }
        changed = true;
        let name = std::str::from_utf8(event.name)
            .unwrap_or_default()
            .trim_end_matches('\0');
        if event.wd == APPS_WATCH_FD.load(Ordering::Acquire)
            && name.ends_with(".json")
            && !name.is_empty()
        {
            let package = name.trim_end_matches(".json").to_string();
            if !package.is_empty() {
                if let Ok(mut packages) = changed_packages().lock() {
                    packages.insert(package);
                }
                return;
            }
        }
        full_change = true;
    });

    if !changed {
        return ChangeKind::None;
    }
    let packages = changed_packages()
        .lock()
        .map(|mut values| std::mem::take(&mut *values).into_iter().collect())
        .unwrap_or_default();
    if full_change {
        return ChangeKind::Full(packages);
    }
    ChangeKind::Apps(packages)
}

fn add_watch(fd: c_int, path: &str) -> bool {
    add_watch_with_id(fd, path).is_some()
}

fn add_watch_with_id(fd: c_int, path: &str) -> Option<i32> {
    let Ok(c_path) = CString::new(path) else {
        return None;
    };
    let wd = unsafe { inotify_add_watch(fd, c_path.as_ptr(), EVENT_MASK) };
    (wd >= 0).then_some(wd)
}

// 仅处理非目录的最终 .json 文件事件；原子保存产生的 .tmp 事件必须忽略，
// 否则一次 apps/<package>.json 保存会被错误升级为全量重载。
fn is_config_event(event: &Event<'_>) -> bool {
    if (event.mask & libc::IN_ISDIR) != 0 {
        return false;
    }
    if !event.name.is_empty()
        && let Ok(name) = std::str::from_utf8(event.name)
    {
        return name.trim_end_matches('\0').ends_with(".json");
    }
    false
}
