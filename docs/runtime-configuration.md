# 运行时配置说明

本文描述模块的核心配置项与路径规则。打包和编译说明请看 [构建流程](build-process.md) 与 [模块打包](module-packaging.md)。

## 全局配置

`/data/adb/modules/storage.redirect.x/config/global.json` 支持以下全局开关：

```json
{
  "file_monitor_enabled": true,
  "fuse_fix_enabled": true,
  "storage_backend_mode": "auto",
  "verbose_logging_enabled": false,
  "auto_enable_redirect_for_new_apps": false,
  "auto_enable_new_apps_template_id": "",
  "app_config_auto_save": false
}
```

- `file_monitor_enabled`：启用文件创建监控；覆盖已配置的普通应用，以及 MediaProvider、DownloadProvider 等系统代写链路。普通应用即使关闭“启用重定向”，也会由 `srx_daemon` 在进程外通过 inotify 监控公共存储写入；这类公共根记录只在文件 owner uid 能明确反查到同一个包名时写入，避免误归因。启用重定向的普通应用会监控隔离根、放行路径、`read_only_paths` 和路径映射目标。普通应用不安装进程内 monitor/PLT hook；这是为了避免普通应用因 native/图形/加固运行时兼容问题出现无法打开或闪退。真实 MediaProvider/FUSE 服务端仍使用进程内 hook 保留调用方识别；DownloadProvider、ExternalStorageProvider、MTP、DocumentsUI、PhotoPicker 和厂商文件管理 UI 不再安装进程内 PLT hook，避免安装应用、设置、文件管理器和导出日志等系统存储链路被卡住。缺失、格式错误或不可读时，沿用历史默认值 `false`。
- `fuse_fix_enabled`：启用 SRX 内置的 FuseFixer 兼容保护，用于处理 MediaProvider FUSE 路径检查中的默认可忽略 Unicode 码点。缺失、格式错误或不可读时，默认值为 `true`。
- `storage_backend_mode`：保留为配置格式字段，运行时固定为 `auto`。native 根据设备能力、规则复杂度与共享宿主会话的健康状态选择共享宿主 FUSE 或 mount namespace（per-app scoped FUSE 已退役，见 [共享宿主架构](shared-fuse-daemon-architecture.md)）。旧版 `fuse_daemon_redirect_enabled` 不再读取；管理端和测试流始终写入 `auto`。最终选择与不可用原因写入 `backend_effective` 日志，并进入诊断摘要。
- `verbose_logging_enabled`：启用详细日志。打开后，普通 native 日志通过 `srx_daemon` 的私有 datagram 通道写入 `running.log`，并启动 Java/崩溃上下文和 MediaProvider/应用状态采集。关闭后运行日志只保留告警与错误、文件监视记录、概览计数和低频状态事件，逐请求的诊断轨迹（挂载视图、别名绑定、资源根展开、扫描未命中命名空间等）不再写入；私有日志接收侧对仍在按旧判定发送的陈旧进程同样只放行告警与错误。文件监视记录由 `file_monitor_enabled` 和文件监视过滤配置单独控制，概览页的轻量运行时生效计数始终启用。缺失、格式错误或不可读时，默认值为 `false`。

概览页“生效次数”累计重定向运行时成功启用的次数：普通应用的 companion mount 成功启用一次计一次，daemon 为尚无 mount state 的进程首次补建重定向运行时计一次，系统写入进程的重定向 hook 成功启用一次计一次。已有 mount state 的配置重载不重复计数。它不统计每次文件访问或路径改写，也不受详细日志和文件监视开关影响。`stats` 使用带 schema 的独立格式保存精确值，存放在 `/data/adb/storage.redirect.x/stats`（模块目录之外的持久路径，模块升级不会触碰该目录）。概览页仅按 `K`、`M`、`B`、`T` 等国际短单位缩写显示，点击计数可查看精确值，长按并确认后可清零重新统计。累计计数不会因模块升级归零，刷入后延迟重启期间由旧 daemon 继续累加的次数也不会丢失；只有首次安装或用户主动清零才会从 0 开始。旧版混合口径的纯数字统计不会并入新版计数。
- `auto_enable_redirect_for_new_apps`：通过 Zygisk 在 `system_server` 注册系统包事件接收器；收到新的第三方用户应用安装事件并完成 PackageManager 校验后，自动为该应用生成配置。卸载应用时其配置会被置回 disabled 并保留文件（不丢用户自定义规则），同时留伴生标记；重装触发的安装事件会先据此把配置恢复为 enabled 再走自动开启，因此卸载重装的应用能自动恢复开启，用户手工关闭的配置不受影响。模块会维护 `/data/adb/modules/storage.redirect.x/config/auto_new_apps_baseline` 作为基线，避免升级、重启或重复事件把旧应用误判为新应用。缺失、格式错误或不可读时，默认值为 `false`。
- `auto_enable_new_apps_template_id`：新应用自动重定向启用时使用的配置模板 ID。为空时，新安装应用默认只开启重定向，不附加允许路径、沙盒路径或映射规则。若模板文件被外部修改导致该 ID 不再可用，运行脚本会先按仅开启重定向生成新应用配置；APP 和 WebUI 进入设置页时会清空失效引用，并在自动配置模板状态行提示已回退。缺失、格式错误或不可读时，默认值为空字符串。
- `app_config_auto_save`：控制 WebUI 应用配置页是否在每次配置操作结束后自动保存。缺失、格式错误或不可读时，默认值为 `false`，即仍需点击保存按钮。

## 仅映射模式

每个 `user` 节点都支持 `mapping_mode_only`：

```json
{
  "users": {
    "0": {
      "enabled": true,
      "mapping_mode_only": true,
      "path_mappings": {
        "DCIM/Camera": "Pictures/CameraBackup"
      }
    }
  }
}
```

- `mapping_mode_only=true`：不再把整个 `/storage/emulated/<user>/` fallback 重定向到应用沙盒，只应用 `path_mappings` 中显式配置的映射；未命中映射的路径保持原样。
- `mapping_mode_only=false`、字段缺失、字段为空或不是 bool：保持旧行为，即 `enabled=true` 时继续执行完整隔离重定向，并在其上叠加 `path_mappings`。
- 该模式同样覆盖 `com.android.providers.media.module` 等系统代写进程的按调用方重定向逻辑，适合只希望媒体服务代写特定映射目录、不希望整包路径被隔离的场景。

## FUSE 规划根与功耗

`storage_backend_mode = auto` 的语义是 **FUSE 优先**：能力可用时，一条应用的规划根恒定为该用户的
存储视图根（`/storage/emulated/<user>`），由唯一的共享宿主会话（`srx_fuse_host`）承载，映射、通配、
沙盒、只读、放行与排除全部在策略层逐请求判定。因此**不存在「规划根个数」这个维度**，也没有按根
折叠、按根 fork 会话或按根计预算的路径——历史上的软目标 4 / 上限 16 两档阈值与相应的规划日志已随
改造删除，不要再按它们排查。

功耗与性能特征：

- **空闲成本为零**：会话是单线程阻塞在 `/dev/fuse` read 上，无 IO 时不产生 CPU 唤醒；成本只是每个
  宿主会话一个进程的内存占用，不随应用数线性增长。

### 空闲期唤醒总账

模块的设计原则是：**任何定时器都必须对应一件确实要做的事，并且只在那个时刻醒来**。
下面这张表是当前全部常驻唤醒，排查耗电时按它对照，不要把它们当成主因（合计约 0.02 mW）。

| 站点 | 频率 | 为什么不能更少 |
|---|---|---|
| 宿主会话服务循环 | 0.0017 Hz（纯保险） | 事件驱动：只在「会话退出」与「请求活动边沿」上醒来；空闲上限 10 分钟只用于「边沿通路本身失效」时仍能自愈锚点与归还 |
| 宿主停摆看门狗 | 0.017 Hz（空闲）/ 1 Hz（有在途） | 有在途请求或刚有请求完成时按 1 秒读 sysfs `waiting`；空闲时 60 秒一次纯兜底——`begin_request` 边沿只覆盖「worker 取走了请求」，覆盖不了「请求在内核队列里没人取」，那是唯一只能靠看一眼 `waiting` 发现的形态 |
| 宿主心跳写入 | 0.017 Hz（空闲）／0.2 Hz（活跃） | 冻结检测本质上需要一个「我还在」的脉冲；写入是共享页内存写，零系统调用 |
| daemon 心跳判定 | 0～0.017 Hz | 无会话时无限期阻塞（只等「共享页建立」事件）；有会话时只睡到当前心跳的陈旧截止点；宿主「空闲→活跃」会额外叫醒它一次 |
| daemon 系统进程名册 | **0** | 需求驱动：每回答一次身份询问就刷新一轮；没人问时只有 10 分钟一次的文件维护 |
| daemon 日志落盘 | **0** | 只有真的收到日志才武装 2 秒定时器（timerfd） |
| daemon 主循环 | **0** | 配置目录、运行开关目录、宿主 pidfd、控制 eventfd 全是事件源；超时只用来对齐下一次周期工作的截止点 |
| daemon 周期 reconcile | 0.003 Hz | 外部 umount 没有可订阅事件，只能长周期自愈（5 分钟） |
| 配置指纹兜底 | **0** | 事件通道在线时永不复查（队列溢出本身就是事件，见下）；「在线」要求 config 与 apps 两个 watch 都在位，`apps/` 晚创建时会补挂 |
| 包事件收集器 | **0** | 常驻 inotifyd 阻塞读；30 分钟一次自愈复查只兜「receiver 静默死亡」 |
| 应用崩溃环形采集器 | **0** | logcat 只订阅 `crash` 缓冲（实测 0.000% CPU、0 字节落盘）；主缓冲日志改为按需窗口采集 |

### 无事件源时的四条替代原则

1. **截止点驱动**：能算出「什么时候才可能有结论」的，就睡到那个时刻，而不是按固定节拍空看。
   例如心跳判定只在「当前这条心跳的陈旧截止点」之后才可能成立，daemon 因此睡到那一刻
   （0.2 Hz → 0.017 Hz）；宿主锚点校验、空闲页归还都挂在「有在途请求」的活动边沿上。
2. **需求驱动**：表类数据（系统进程名册）只在有人查、且查不到的时候才刷新——缺口本身就是
   最精确的事件（一次身份询问 = 表里缺这一项）。注意刷新必须**强制重建**而不是「复用指纹
   判定」：表里除了进程还要登记每个系统进程的全部线程 tid，而线程增减不会改变进程名册指纹。
   若只在名册变化时重建，「已有进程新建线程」就会一直查不到，且询问引发的刷新又什么都不做
   ——变成「越问越不刷」。强制重建的频率由 2 秒合并窗口约束，最坏与旧的固定周期同量级，
   但只在真有人查表时才付。
3. **溢出触发**：周期对账类工作（inotify 死条目剪枝、配置指纹复查）都存在「漏事件」风险，
   而**漏事件这件事本身是可观测的**（`IN_Q_OVERFLOW`）。把对账挂到溢出事件上，稳态就变成 0。
4. **把「文件被写了」变成事件**：WebUI 的运行开关（`.runtime_disabled`）是纯文件通道、没有
   IPC，daemon 因此额外监听**模块目录**这一个 inotify watch（不递归，`logs/`、`tmp/` 内部的
   写入不会触发它）。开关被改写即时生效，主循环不再需要任何固定节拍去「看一眼开关」。

### 心跳为什么必须存在（以及它能被压到多低）

进程内看门狗覆盖不了「**整个进程被冻住**」这一种故障：冻结时连同看门狗线程一起停住，
没有任何线程能发出信号。此时只有**进程外**的观察者（daemon）能读出「宿主已经不动了」并
触发 abort，让所有请求立刻以错误返回、随后重建会话。若没有这条脉冲，2026-10-29 那类
整进程停摆会让每个请求永久挂在内核队列里，只能靠重启恢复。

但「证明我还活着」是一个纯时间性质（liveness），它无法被事件替代——**进程停止运行本身不
产生任何事件**。所以这里能做的只是把它压到极低，并让代价随「有没有活」伸缩：

- **空闲期**：心跳间隔 60 秒（0.017 Hz），心跳写的是共享页内存，零系统调用、零磁盘 I/O；
- **有在途请求/内核有积压/刚有请求完成**：立刻回到 5 秒；
- 心跳里**带上本次使用的间隔**，daemon 按「2 倍间隔」推算陈旧阈值（下限仍是 20 秒），
  因此间隔变长不会被误判成冻结；宿主在「空闲→活跃」边沿上写一次唤醒 eventfd，
  daemon 立刻改按活跃阈值评估——活跃期冻结的发现延迟仍然是 20 秒级。

同时，进程内看门狗的**判定与解冻路径改成了零堆分配**（栈缓冲 + 裸 `open/read/write`）：
它要面对的现场往往正是「分配器锁被卡住」，用 `format!`/`read_to_string` 会让看门狗跟着
一起停摆。改成无分配之后，只有「整进程被外部冻住」才必须依赖进程外脉冲。

### 其余与耗电相关的实现要点

- **配置变更在进程内完全是事件驱动，热路径零系统调用**：inotify fd 由 root 在 specialize 前建好
  并随 Zygisk 保留被继承，由一个**阻塞在 `poll(-1)` 上的后台线程独占读取**（空闲零唤醒、64 KiB
  栈）；事件到达后去抖、解析、发布，拦截热路径只做一次原子读（有变更时才抢锁）。桥接进程没有
  拦截热路径可搭车，直接订阅同一条事件通道的回调，因此也不再需要自己的 1 秒指纹线程。
- **指纹复查只在事件通道不可用时存在**（init 或目录 watch 失败、读取线程退出）：此时才退回 1 秒
  一次；事件通道在线时该项为 0，因为丢事件已被 `IN_Q_OVERFLOW` 精确覆盖（溢出会被上报成一次
  全量变更，调用方按全量重新比对）。
- **热路径上的重试一律带退避，且优先由事件触发**：给 FUSE 引擎打 read 补丁的「按库定位」重试
  从固定 500 毫秒改为按库指数退避（500 ms 起、倍增、上限 5 分钟）；而「库晚加载」这个真正的
  触发条件是一个事件（`dlopen` 钩子），命中即重置退避并立刻补装，因此正常机型根本不会走到定时
  重试。远端目标库永不出现的 ROM 上，重试也不会再以 2 Hz 永久做 ELF 定位。
- **文件监视的死条目对账只在队列溢出后执行**：正常路径的 watch 消失都以 `IN_IGNORED` 事件到达
  并在事件路径上就地删除；只有溢出会丢掉这类事件、才可能产生账本漂移，因此不再按固定周期读
  `fdinfo`（数万行解析）。
- **系统进程名册表（`tmp/system_pids`）按「文件身份」重读**：hook 进程命中缓存时完全不碰文件，
  未命中时先 `stat` 一次，只有 inode/大小/mtime 变化（daemon 用 tmp + rename 原子替换，必然换
  inode）才把整表读回来，因此不会因为「缓存过期」被周期性重读，也不会因 miss 风暴把 512 KiB 的
  读取放大成每秒数次。
- **心跳是父子共享内存页**：daemon 在 `fork` 前建立的 `MAP_SHARED` 匿名映射，宿主写入是纯内存
  操作（零系统调用、零磁盘 I/O），模块因此没有「空闲期写盘」；共享页不可用时自动回退文件心跳。
- **看门狗路径零堆分配**：`waiting` 读取与 abort 写都走栈缓冲 + 裸系统调用（`open/read/write`），
  这样即使堆分配器已经被卡住的线程占着，第一道防线仍然能工作。
- **停摆取证也是「有界 + 限频」的**：一次未确认（candidate）场景原本会输出 900 余行 /
  约 110 KiB（`/proc/self/mountinfo` 700 行 + 宿主与 MediaProvider 每个线程的内核栈），
  而这类场景在开机与媒体扫描期每 5~10 分钟就会出现一次——写放大量级与刚修掉的 logcat
  采集器相当，且会把 `running.log` 的轮转窗口挤掉。现在：行数预算分两档——
  未确认场景 80 行（≈10 KiB）、确认场景 600 行；`mountinfo` 只输出与存储相关的行
  （封顶 48，其余按剩余额度补）、每线程栈 6 帧；candidate 场景额外限频到 10 分钟一次
  （`HOST_STALL_SCENE_THROTTLE_MS`），确认场景不受限。另外修掉一个真机暴露的假阳性：
  「本会话从未完成过请求」时 `last_completion_ms == 0`，`now - 0` 会算成 1.79e12 ms，
  于是**每次开机**第一个请求还在飞时必然判成候选停摆（真机实测每次开机 1 次、110 KiB）；
  基线改用看门狗启动时刻后，开机场景归零。
- **常驻日志采集器只订阅 `crash` 缓冲**：logcat 的过滤发生在**客户端**——只要订阅了 `main`
  缓冲，它就必须把 main 的全部条目从 logd 收下来再逐条丢弃，代价是约 0.28% 单核（≈28 mW，
  0.7 Wh/天），与「最终留下多少行」无关。实测对照（同一台机器 60 秒）：

  | 订阅 | CPU | 落盘 |
  |---|---|---|
  | `-b main,crash` + `*:E`（旧） | 0.317% | 1132 行 / 45 秒（100% 平台噪声，25793 行历史里 0 条真实应用错误） |
  | `-b main` + 白名单 | 0.283% | 0 字节 |
  | `-b crash` + 白名单（现在） | **0.000%** | 0 字节 |

  因此常驻部分只留 `-b crash`（Java 崩溃栈 + 原生 tombstone 头，只在真崩溃时才有内容）；
  主缓冲的 Error 与图片组件日志改为**按需窗口采集**
  （`start_app_error_window_capture [分钟]`，诊断包字段 `app_error_window_present`），
  成本与「确实要看日志」严格对齐。另外 `reap_stale_logcat_collectors` 只回收 debug 采集器
  自己的孤儿——把常驻 crash ring 纳入回收会让「verbose 开关翻转」静默杀掉它且不再重建。
- **关闭详细日志时的停止动作必须是「先杀子 shell、再收管道」，且子 shell 用 SIGKILL**：
  采集器是 `( logcat | awk | while read ) &`，子 shell 是 mksh 且阻塞在管道的 `read` 上，
  `SigCgt` 里带 SIGTERM，因此 SIGTERM 会被延后到该次读返回才处理——verbose 关掉后 SRX 行
  可能长时间一条都没有，运行期等价于「没杀」。而停止后紧接着就会把状态写成 `off`、删掉 pid
  文件，此后没有任何配置事件会再尝试停止（`debug_collectors_sync_to off` 的 state 早退），
  泄漏锁定到下次开机。同理，只按 tag reap 也停不下来：子 shell 会在 1 秒内把 logcat 重建
  回来（实测 pid 8032 → 8468）。顺序因此固定为「SIGKILL 子 shell → 按 tag SIGKILL logcat」，
  awk 随输入 EOF 自行退出。`stop_background_process` 的递归段依赖
  `/proc/<pid>/task/<pid>/children`，该文件并非所有内核都提供（A14 模拟器上不存在），
  不能把「子进程会被顺带杀掉」当作前提。
- **所有管道型采集器都必须能真正停掉，且子进程不能留成孤儿**：这条不限于
  详细日志采集器。config / package 两个事件收集器同样是「子 shell + 管道成员」，杀掉子
  shell 只让它不再重建——A14 实测：config 收集器被 SIGKILL 后，它的**读循环仍在处理配置
  事件**、`inotifyd` 仍在喂数据，等于两套事件处理器并存（每条事件处理两次，含一次
  `cmd package list` 级别的 UID 对账）。
  **不要用 `exec -a` 给 `inotifyd` 打标记**：`/system/bin/inotifyd` 在部分机型上只是
  toybox 的符号链接，而 toybox 靠 `argv[0]` 分派 applet——改掉 argv[0] 后它直接起不来，
  整条配置事件链静默失效（A17 实测；表面现象像「进程起来又停掉」，极易误判）。正确做法是
  在 `stop_background_process` 里按 PPID 递归收子进程：`/proc/<pid>/task/<pid>/children`
  不可用时退化为一次 `ps -A -o PID,PPID` 快照取子进程，并且**先取清单再杀**——先杀父会让
  孙进程立刻被 init 收养，之后再也找不到。`exec -a` 只保留给 `logcat` 这种真正的 ELF。
  `stop_collector_by_pid_file` 默认信号是 KILL，因为 TERM 会被延后到这些循环阻塞的
  `read` 返回才处理（旧实现「先停旧再起新」的停因此从未生效过）；KILL 之后 `wait` 必定
  立即返回，调用方不会再被挂住。
- **配置目录的 inotifyd 掩码必须含 `y`（MOVED_TO）**：配置的原子保存写法是「写临时文件 →
  rename 覆盖目标」，这条路径只产生 `m`（MOVED_FROM，路径是临时文件名）和 `y`（MOVED_TO，
  路径才是 `global.json`）。只监听 `wcmdn` 时两半都收不到：`m` 的文件名过不了 `*".json"`
  过滤，`y` 根本不上报——结果是「配置确实写进去了，但开关毫无反应」。A17 实测：掩码
  `wcmdn` 下 rename 只吐出 `m <dir> stage.txt`；扩到 `wcmdny` 才有 `y <dir> renamed.json`，
  开关从「40 秒无反应」变成 2 秒内生效。WebUI 的 `restoreConfigSnapshot` 正是
  `mv "$stage/global.json" "$config/global.json"`。**不加 `e`（ATTRIB）**：`chmod`/`touch`
  只改元数据，不代表配置内容变化，为本不存在的变更跑一次完整 sync 不划算。
- **两个一次性快照采集器必须后台执行**：`media_state` 快照里的 `dumpsys meminfo` /
  `logcat -d` 在 A17 上首次实测 47-58 秒（A14 上 8-10 秒），`app_status` 的
  `cmd package list packages -U` 另有数秒。它们同步跑在 `start_debug_collectors` 里就等于把
  配置事件循环按住整段时间，期间所有开关操作排队——现场表现正是「按了开关很久没反应」。
  快照只写自己的日志文件、不依赖调用方上下文，后台化不改变语义；pid 落盘是为了让下一轮
  `start_debug_collectors` 的「先停旧」能收掉还没返回的上一轮。后台化后 `debug start: logcat`
  到 `debug start: ready` 实测 1 秒内完成，`snapshot captured` 反而出现在 `ready` 之后。
- **同一份脚本会被两种解释器执行，代码不能依赖其中一种的实现细节**：开机时框架用
  `busybox sh service.sh`（ash）启动，而 `srxctl ensure-collectors` 用 `sh "$MODDIR/service.sh"`
  （`/system/bin/sh` = mksh）重启。两条路径的解释器不同，任何一边的方言差异都会变成「有时
  生效、有时不生效」。`srxctl` 自身的 shebang 也是 `#!/system/bin/sh`。
- **所有带 `-o` 的 `ps` 调用必须写 `/system/bin/ps`**：busybox 的 ash 把 applet 当内建命令，
  `ps` 会解析成 **busybox 版**，而它只认小写 `-o pid`——`ps -A -o PID,ARGS` 直接报
  `bad -o argument 'PID'` 且 `rc=1`。错误被 `2>/dev/null` 吞掉之后，唯一表现就是「选不出
  任何进程、什么都没杀」：A17 实测 boot 路径下 `reap_tagged_processes` 的 `victims` 恒空、
  每轮开关泄漏一个 logcat（tagged 1→6），同一份代码在 mksh 路径下完全正常。同一个坑还打掉了
  `stop_background_process` 的 PPID 递归段，使管道成员（读循环 / inotifyd）全成孤儿。
  受影响文件：`common.sh`、`media_state.sh`、`app_status.sh`、`diagnostic_archive.sh`、
  `uninstall.sh`。改完由守卫 `test_process_listing_uses_absolute_ps_path` 拦住回归。

- **读循环里的参数展开模式必须给 `|` 加引号**：Android 的 `/system/bin/sh` 是 mksh
  （R59 起带 ksh 风格扩展模式），裸写 `${x%%|*}` 会被当成「空模式或任意」剥掉整个字符串、
  `${x#*|}` 一个字符都不剥（A14 实测）。于是 `kind` 恒为空、`case` 一个分支都进不去——
  采集器活着、也读到了数据（管道成员 `wchar` 非 0），却从不写 `running.log` /
  `app_status.log`。写成 `${x%%"|"*}` / `${x#*"|"}` 后在 mksh 与 POSIX shell 上都是字面量。
  同一处还要注意 threadtime 把 tag 补齐到 8 列并在其后单独留一个 `:` 字段：tag 短于
  8 字符（如 `SRX`）时该冒号是独立的 `$7`，不跳过就会产出 `[JvInfo] : [JvInfo] 消息`
  这类重复前缀。
- **上一次开机留下的采集器 pid 与状态文件必须在本 boot 首次运行服务时清空**：pid 号会被
  复用，旧 pid 可能已属于无关进程。用它判活会让本 boot 静默跳过采集器启动（实测：
  `state=on` + 旧 pid 存活 → 连 `sync: starting debug collectors` 都不出现）；用它停止则
  会把无关进程杀掉（实测：关闭开关时信号打到复用的 pid）。清理以
  `logs/boot_<boot_id>.marker` 是否已存在为判据——服务被 `srxctl` 重跑时 marker 已在，
  不会清掉正在运行的采集器记录；daemon 的 pid 文件不在此列，它自带 `comm` 校验。
  卸载脚本是同一不变量的另一处落点：它用 `logs/.boot_ok` 与当前 boot_id 比较，不一致就
  跳过按 pid 文件清理（只保留按 tag 回收 logcat，那条不依赖 pid 号）。

## 局部沙盒路径

`mapping_mode_only=true` 时，默认只有 `path_mappings` 会被重定向，未命中映射的路径会保持原样。若某些应用或系统服务仍会在 `/storage/emulated/<user>/` 根目录乱创建文件或目录，可以用 `sandboxed_paths` 指定这些路径仍然进入应用沙盒。

```json
{
  "users": {
    "0": {
      "enabled": true,
      "mapping_mode_only": true,
      "sandboxed_paths": [".xlDownload"],
      "path_mappings": {
        "Download/DLManager": "Download/第三方下载/DLManager"
      }
    }
  }
}
```

- `sandboxed_paths` 支持字符串或字符串数组，例如 `"sandboxed_paths": ".xlDownload"` 或 `"sandboxed_paths": [".xlDownload"]`。
- 路径规则必须是相对存储根目录的安全路径；绝对路径、空路径、`..` 等不安全路径会被忽略。
- 映射优先级高于 `sandboxed_paths`：先匹配 `path_mappings`，未命中映射时再判断是否应进入沙盒。
- 该配置主要服务 `mapping_mode_only=true` 场景；普通完整隔离模式本来就会把未放行路径重定向到沙盒。

## 只读模式

`read_only_paths` 用于把目录保持可读但禁止写入。管理 App 和 WebUI 会把它做成独立的“只读模式”开关和路径区域；手动编辑 JSON 时也可以直接写字段：

```json
{
  "users": {
    "0": {
      "enabled": true,
      "read_only_paths": ["Documents/MyApp"]
    }
  }
}
```

- `read_only_paths` 支持字符串或字符串数组，运行时读取会按真实路径放行并禁止写入。
- `Android/data/<包名>`、`Android/media/<包名>`、`Android/obb/<包名>` 是 MediaProvider 管理的应用私有外部存储根。挂载恢复优先使用系统 FUSE 锚点，关闭针对整棵私有树的属主修复监视，并保留已有子目录元数据；包名根仍有初始化处理。其中 `Android/media/<包名>` 在整根已由共享宿主 FUSE 接管时同样恢复为系统 MediaProvider FUSE 视图，使未接管的跨进程消费方与重定向应用共享同一份 `-shm` 映射与页内锁，避免 SQLite 因双视图回滚到旧检查点；`Android/data/<包名>` 承载重定向沙箱本身、`Android/obb/<包名>` 无跨进程数据库共享需求，两者保持直连真实后端。系统代写属主修复只调整 owner、保留原生 gid；升级后的一次性迁移将 `Android/data` 下无 other 权限位的应用私有目录恢复为 `ext_data_rw`（1078），覆盖两种漂移：`gid=media_rw`（1023，旧 hook 覆盖了继承来的组）与 `gid=应用自身 uid`（目录创建入口 `chown(uid, uid)` 的产物，会让系统代写进程连穿越都做不到），文件和媒体共享子目录继续保留 `media_rw`（1023）。目标是不依赖额外允许路径访问自有目录，当前版本的完整真机回归尚未完成。
- 共享宿主 FUSE 接管存储视图后，应用的写入仍按原生 MediaProvider 语义判定：只有自有私有目录（sharedUserId 组内兄弟包名视同自有）、`Android/media` 共享子树、配置规则（映射、放行、沙盒）与公共媒体集合（Download、DCIM、Pictures 等）可以直写真实存储；对存储根下其它自定义目录的创建与写入会被拒绝，与不使用模块时的系统行为一致。
- 普通应用通过 mount namespace 的只读 bind mount 强制生效，不会因为只读配置额外安装 PLT hook；这是稳定性约束，避免普通应用因 native/图形/加固运行时兼容问题出现无法打开或闪退。
- 真实 MediaProvider/FUSE 服务端仍通过现有系统 writer hook 识别调用方；DownloadProvider、ExternalStorageProvider、MTP、DocumentsUI、PhotoPicker 和厂商文件管理 UI 不进入进程内 PLT hook 链路；写入只读路径会返回 `EROFS`。
- 只读正向规则会提供真实读取通道；即使没有配置 `allowed_real_paths`，应用也能读取该目录但不能写入。`!` 只读排除规则优先覆盖同组正向只读规则，命中后继续按沙盒、映射或显式允许规则处理。
- 路径映射的入口或最终目标命中只读路径时，映射入口也会继承只读，不能通过映射绕过写入限制。
- 只读路径接受相对目录、`!` 排除前缀以及 `*`、`?` 通配符；与允许路径排除规则直接冲突的正向只读路径会被忽略，只读排除规则优先于正向只读规则。

## 路径校验与映射规则

管理 App、WebUI 和 native 运行时都会对路径规则做兜底校验。手动编辑 JSON 时也应遵守以下规则：

- `allowed_real_paths`、`sandboxed_paths` 和 `read_only_paths` 继续使用相对共享存储路径，例如 `Download/MyApp`；只有 `path_mappings` 支持绝对 namespace 路径。
- `path_mappings` 的请求路径和目标路径支持相对共享存储路径，也支持以 `/` 开头的绝对路径；绝对路径会在应用自己的 mount namespace 中生效，可用于 `/data/user/<用户>/<包名>/...`、`/data/data/<包名>/...` 等应用私有目录。
- 应用私有根目录本身不作为映射入口，但其子路径仍支持通配映射，例如 `Android/data/com.example.app/file/* -> Download/AAA`；已有私有子目录在元数据准备阶段保留属主和权限；包名根仍有初始化处理，不表示所有历史权限均已自动恢复。
- 不能包含 `.`、`..` 路径段、控制字符或超过 512 字符的路径。
- 非映射规则不能直接写 `sdcard`、`storage/emulated`、`storage/self/primary`、`data/media` 等存储根或根别名；映射的绝对路径也必须通过 namespace 安全校验。
- `allowed_real_paths` 支持 `!` 排除前缀；`path_mappings` 和 `sandboxed_paths` 不支持 `!`。
- `read_only_paths` 支持 `!` 前缀表示只读排除，也支持 `*`、`?` 通配符。
- 同一路径既作为放行又作为排除时，排除规则优先保留。
- `path_mappings` 的源路径和目标路径不能相同，不能形成循环映射，映射链深度超过 10 层的源规则会被忽略。
- 映射支持有限深度的嵌套链：上一条规则改写出的目标路径如果继续命中下一条规则，会继续按最长前缀规则处理，直到没有后续匹配或达到 10 层上限。
- `Android/data`、`Android/media` 和 `Android/obb` 可以作为映射入口或目标，管理 App 的路径浏览器也可直接进入这些目录。启用重定向时，当前应用自己的 `Android/data/<包名>`、`Android/media/<包名>`、`Android/obb/<包名>` 子树默认保持真实路径，不会再次套入应用沙盒；其它应用包名的同名目录不享受这条默认放行。MediaProvider 侧的访问校验与此独立：`Android/media/<包名>` 子树对同用户应用 uid 保留系统原生的跨应用共享语义，`Android/data` 与 `Android/obb` 仍按属主私有语义校验。`/data/user/<用户>/<包名>/...`、`/data/data/<包名>/...` 等应用私有路径请在映射输入框中直接填写绝对路径。
- `/data/data/<包名>/...` 会按当前用户转换为 `/data/user/<用户>/<包名>/...`，旧配置中的这个历史别名继续有效。
- 启用应用重定向后，未命中的公共路径默认进入该应用的沙盒目录。因此映射请求路径填写应用实际看到的公共路径，目标路径填写希望绑定的存储路径或绝对路径，不需要再把公共路径改写成沙盒完整路径。
- `path_mappings` 可以把公共存储路径与应用私有 namespace 路径互相映射；“任意路径”仅表示在当前应用 namespace 和路径校验允许的范围内自由组合，不包含系统根目录或越权访问。

## 备份格式

WebUI 和管理 App 的设置页都会导出 `.srxbak.zip` 备份包。该文件是通用 ZIP 格式，内部固定包含 `backup.json`：

```text
storage-redirect-x-backup-YYYYMMDD-HHMMSS.srxbak.zip
└── backup.json
```

`backup.json` 的结构仍是 schema v2，包含模块 id、格式版本、创建时间、摘要、SHA-256 完整性校验和配置数据。配置数据包含：

- `global`：全局配置。
- `apps`：全部 `apps/<package>.json` 应用配置。
- `templates`：配置模板。
- `monitor_filters`：文件监控过滤配置。
- `ui`：管理界面本地偏好，例如预测性返回手势。

还原时会先校验 ZIP 中是否存在 `backup.json`，再校验模块 id、schema、配置字段和完整性摘要。旧版 `.srxbak.json` 文件仍可导入，用于兼容历史备份。

## 日志轮转与日志包

运行时日志位于 `/data/adb/modules/storage.redirect.x/logs/`。native 普通日志、文件监视记录和 Stats 不再经由 Android `logd` 与常驻 `logcat` 中转，而是由 `srx_daemon` 内置接收器批量写入；Warn/Error 仍可镜像到 Android 日志用于紧急排障。主要日志按大小轮转并保留 `.1`、`.2` 两级历史：

- `file_monitor.log`：默认 1 MiB。
- `running.log`：默认 2 MiB。
- `media_provider_state.log`：默认 10 MiB。
- `app_status.log`：默认 10 MiB。

设置页右上角的文件图标会导出一个 `storage-redirect-x-logs-YYYYMMDD-HHMMSS.tar.gz` 日志包。点击导出后会先立即截取 logcat，再执行可能耗时几十秒的进程、挂载和 dumpsys 采集，最后补抓导出期间的新日志。

logcat 窗口按“行数上限”而不是固定分钟计算，因此不是“开始前一分钟”：

- `logcat-srx-filtered.txt`：相关标签最多 10000 行。
- `logcat-main-system-context.txt`：main/system 上下文最多 8000 行。
- `logcat-crash.txt`：完整 crash 环形缓冲区；该专用缓冲区通常较小，并非导出全部 main/system 内容。
- `logcat-events.txt`：events 最多 1500 行。
- `logcat-export-period.txt`：从首次截取开始到导出末尾的相关日志，最多 3000 行。
- `state/logcat-capture.txt`：记录实际截取起止时间和各窗口上限；`logcat-buffers.txt` 记录设备缓冲区容量。

繁忙设备在相同行数内覆盖的时间可能只有几十秒，安静设备则可能覆盖数小时。上述分层窗口避免直接导出全部 main/system 缓冲区导致日志包过大、内容过杂，同时优先保留复现时间点、崩溃和 SRX 相关上下文。日志包还包含模块日志及轮转历史、`module.prop`、`stats`、关键配置、设备/进程状态和 dmesg tail；配置迁移应使用 `.srxbak.zip` 备份。

## 排除规则

路径过滤支持排除语法，用于在放行规则中排除特定子路径或文件。

### 语法

| 规则 | 说明 |
|------|------|
| `!` 前缀 | 在规则前加 `!` 表示排除，例如 `!Pictures/Private` |
| `*` 通配符 | 匹配任意字符（不含路径分隔符），例如 `!Download/*.tmp` |
| `?` 通配符 | 匹配单个字符，例如 `!DCIM/Camera/IMG_????.jpg` |

### 优先级

同一应用下，**排除规则优先于放行规则**。即先应用放行规则，再应用排除规则进行过滤。

### 限制

`!` 和通配符仅用于**真实路径规则**，不用于**路径映射**。

自动模式下，内核 bind mount 不能表达的动态通配规则需要 FUSE 承载；宿主会话不可用（设备无 `/dev/fuse`、接入闸门未放行、策略未登记或接入失败）时降级为 mount namespace，并把原因写进日志与诊断摘要。最终后端、FUSE cache 容量和 mount intent 数量同样会进入运行日志与诊断摘要。

### 示例

允许相册但排除私密目录：

```text
Pictures
!Pictures/Private
```

允许下载目录但排除临时文件：

```text
Download
!Download/*.tmp
!Download/*.part
```

允许相机目录但排除测试命名文件：

```text
DCIM/Camera
!DCIM/Camera/TEST_*
```
