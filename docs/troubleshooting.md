# 排障说明

本文汇总运行时日志、关键标记和常见崩溃原因。设备侧验证流程请看 [设备侧测试说明](device-testing.md)。

## 查看模块日志

详细日志开启时，native 日志优先写入 `/data/adb/modules/storage.redirect.x/logs/running.log`，文件监视记录写入 `file_monitor.log`。`adb logcat -s "SRX" "StorageRedirect"` 主要用于查看 Java 日志、native Warn/Error 镜像和私有通道不可用时的文件监视回退，不再是完整 native 日志来源。

详细日志关闭时 `running.log` 只保留告警与错误、文件监视记录、概览计数和低频状态事件（挂载完成、热重载回执），逐请求的诊断轨迹（挂载视图、别名绑定、资源根展开、扫描未命中命名空间等）只在开启详细日志后写入。

也可以在 WebUI 或管理 App 的设置页右上角点击文件图标导出日志包。导出的 `storage-redirect-x-logs-*.tar.gz` 会包含模块日志、轮转历史、关键配置、运行状态快照、最近相关 logcat 和 dmesg tail，适合在 Issue 或排障沟通中直接提供。公共存储视图不可写时（存储异常的典型现场），导出会自动降级把日志包写到模块目录 `logs/` 下，此时需要用 root 文件管理器或 `adb` 手动取出。导出在采集挂载与路径快照时会为可能被存储停摆拖住的命令加硬超时，超时只丢弃该段证据，不会让整包导出失败。

设备侧日志目录：

```text
/data/adb/modules/storage.redirect.x/logs/
```

主要日志会按大小轮转并保留 `.1`、`.2` 历史，例如 `file_monitor.log`、`file_monitor.log.1`、`file_monitor.log.2`。`running.log`、`file_monitor.log` 由 `srx_daemon` 私有日志接收器统一写入，默认不需要常驻 `logcat` 采集器。生效次数统计 `stats` 同样由 `srx_daemon` 写入，但存放在模块目录之外的持久路径 `/data/adb/storage.redirect.x/stats`，模块升级不会影响该文件。

`media_provider_state.log` 与 `app_status.log` 不是周期采样：采集器只在详细日志打开、配置变化与开机时各抓一次快照，其余现场由诊断包导出与停摆取证按需采集。因此关闭详细日志后这两个文件不会再增长，属于预期行为。

日志包会在导出一开始保存分层 logcat 快照，并在末尾补抓导出期间日志。`-t 10000` 等参数表示行数而不是秒数；可查看包内 `state/logcat-capture.txt` 判断实际截取时间。为控制体积，main/system 只保留 8000 行上下文，相关标签保留 10000 行，events 保留 1500 行；只有独立且通常较小的 crash 环形缓冲区会完整导出。

## 关键日志标记

| 日志 | 含义 |
|------|------|
| `java hook init ok` | LSPlant + Hooker.dex 初始化成功 |
| `java hook init failed` | Java hook 初始化失败（检查 dex 是否完整） |
| `java hook query ok` | MediaProvider.query() hook 安装成功 |
| `hook_redirect=true` | 进程启用了 hook 重定向模式 |
| `row filter reason=hide_nonexist` | cursor 行因目标不存在被隐藏 |
| `row filter reason=hide_probe_nonexist` | MediaStore 相册桶/relative_path 探针被隐藏；用于避免缩略图、计数、聚合相册漏出 |
| `row rewrite reason=rewrite` | cursor 行路径被重写（目标存在） |
| `row rewrite reason=rewrite_pending` | cursor 行路径被重写（目标不存在，pending 记录） |

## 文件监视归因

监视记录的第一列应用名来自归因结果，不是"落盘进程"。系统代写链路（MediaProvider、相册选择器、DocumentsUI 等）会以自身进程完成落盘，因此记录里的进程字段常常是这些系统组件，而真正的调用方要靠 `identify_method` / `identify_reliability` 判断：

| 情况 | 显示 |
|------|------|
| `identify_method=caller` / `provider_open` / `provider_mkdir` 等直接信号 | 具体调用方应用名，详情行给出识别方式 |
| `identify_method=caller_pid` | 记录带 `caller_pid`（内核 FUSE 头里的请求方 tid），由 root 侧按 pid 反解出真实包名。适用于 root 文件管理器 helper 一类"进程名不是包名"的写入：识别结果就是它所属的应用，原始进程名保留在详情行的 `native=` 字段 |
| `identify_method=unknown` 或 `identify_reliability=none`，且落盘方是系统代写进程 | 应用名显示"未知来源"，详情行给出"代写进程 <包名>"；若详情行带 `caller_uid` / `caller_pid`，说明请求方身份是已知的，只是它不是应用（如 shell 二进制），可据此继续定位 |
| 文件系统观察类记录（inotify 侧） | 显示"文件系统观察"，观察器与实际落盘进程分开列出 |

"未知来源"表示这次写入确实无法归因到具体应用：典型来源是经 MediaProvider 中转、调用方信号已经失效的写入（系统后台创建的目录），以及 root 文件管理器绕过 FUSE 直接落盘的写入。它不是"没有记录"，路径、进程与结果仍然完整，可据此对照 `running.log` 与 `file_monitor.log` 定位。

## MediaStore 写入返回 null（`createMedia returned null`）

应用侧 `MediaStore.insert()` 返回 null / 抛 `FileNotFoundException` 时，**先分清两种完全不同的成因**，否则会把环境竞态当成数据面缺陷：

1. **卷还没挂上**（最常见，尤其刚重启 MediaProvider）：MP 日志里有
   - 首行：`E MediaProvider: Volume external_primary not found, calling identity: UserHandle{0},`
   - 续行：`attached volumes: {MediaVolume name: [internal] ..., MediaVolume name: [external_primary] id: [emulated;999] ...}`
   - 异常：`E DatabaseUtils: java.lang.IllegalArgumentException: Volume external_primary not found`
   `attached volumes` 里没有 user 0 的 `external_primary`，就说明是重启窗口内的竞态：`insert` 直接抛异常，应用侧只表现为 `createMedia returned null`。等 1~3 秒重试即可成功（集成测试对创建类用例已做有界重试，日志里会出现 `mediastore_create_retry ... reason=create_media_null`）。
2. **文件已重定向但打开失败**：MP 日志是 `Open with FUSE. FilePath: ...` 之后
   `java open backup failure ... FileNotFoundException: open failed: ENOENT`，且
   `running.log` / logcat 里应能看到 `java mapped open prep failed caller_uid=... kind=...`
   （或 `media redirect target uri=...` 缺失，说明 insert 阶段的重定向登记没发生）。
   这才是模块数据面的线索，按路径解析/归因继续查。

排查时**先看 MP 自己的卷状态**，再看我们的 hook 日志——两者的用户可见现象完全一样。

## MediaStore 查询排障

图库类 APP 看到真实相册时，不要只看目标 APP 自身是否已经 mount 到沙盒。相册、缩略图和计数常来自 `com.android.providers.media.module` 的 MediaStore 查询结果。

排查顺序：

1. 确认目标 APP 配置没有显式放行：
   ```powershell
   adb shell "su -c 'cat /data/adb/modules/storage.redirect.x/config/apps/<package>.json'"
   ```
   如果 `allowed_real_paths` 里包含 `DCIM`、`Pictures`、`Android` 等路径，看到或写入这些路径是符合配置的。
2. 重启 MediaProvider 并抓日志：
   ```powershell
   adb shell "su -c 'killall -9 com.android.providers.media.module 2>/dev/null; logcat -c'"
   adb shell "am force-stop <package>"
   adb shell "monkey -p <package> -c android.intent.category.LAUNCHER 1"
   adb logcat -d -s SRX StorageRedirect
   ```
3. 正常应看到 `java hook query ok`、目标 APP UID 对应的 `java query` / `java cursor` 日志，受限相册桶应出现 `row filter reason=hide_probe_nonexist` 或 `java cursor filter...`。若只看到目标 APP 自身 `mount confirmed`，没有 MediaProvider 中目标 UID 的 query/cursor 日志，说明 MediaStore 查询链路没有完整闭环。

## 私有目录里的 SQLite 打不开（`SQLITE_CANTOPEN` / 边车 EACCES）

应用把自己的 SQLite 库放在 `Android/media/<pkg>/` 下时（支付宝内的 XRadiant 就是这种），
如果 `-wal`/`-shm` 是 **owner-only（0600）**，应用打开数据库会直接失败：

```
# quality-allow(chinese-language): 下面两行原样保留真机错误文本，供按日志关键字检索
E/SQLiteLog: (14) os_unix.c:44191: (13) open(.../XRadiant.db-wal) -
E/SQLiteDatabase: Failed to open database '.../XRadiant.db'.
```

随后抛 `SQLiteCantOpenDatabaseException`（`SQLITE_CANTOPEN`，落点是 `PRAGMA journal_mode`）。

原因不是路径解析，而是权限位口径：MediaProvider 的 FuseDaemon 用**自身进程身份**打开下层文件
（passthrough 只是把它自己打开的 fd 交给内核，内核不会改用调用方凭据重开），而 MediaProvider 的
附加组只有 `media_rw`/`ext_data_rw` 一类、且没有任何 capability。因此**组位就是 MP 的可服务性**：

| 模式 | MP 视图内能否打开 |
|------|------------------|
| `0660`/`0664` | 能 |
| `0600`（owner-only） | 不能，连属主自己都拿 EACCES |

原生 Android 不会出现这种文件：MP 的创建口径固定为 `(mode & ~0777) | 0664`（目录 `| 0775`），
不透传应用 umask。owner-only 边车只可能来自「绕过 MP 的直连层」，典型成因是自有私有目录恢复失败
（日志 `own private restore failed ... errno=2`）后应用在 f2fs 上按 umask 077 自建边车。

现在的处置（不修改任何既有文件属性）：

- 模块自己的创建入口（宿主 FUSE 的 app-private 后端）按 MP 的原生创建口径产出 `0664`/`0775`——
  这条口径只在**新建**时决定落盘权限，等同系统视图自己建出来的结果；
- 已存在的 owner-only 入口**不会被模块改写**：MP 打不开就是打不开，这是原生语义。模块只按
  `own private sqlite entries unserviceable by system view count=... sample=...`（详细日志级别）
  上报数量与样本路径，便于定位是哪个绕过系统视图的写入路径产生的；
- MP 侧 sqlite 干预只在 MP 自己打得开该入口时才接管，`-shm` 毒化检查改走 `O_PATH` 尺寸判定，
  owner-only 入口不再拖垮毒化自愈链。

历史遗留 owner-only 边车的手工处置（任选其一，都由应用自己重建即可，不要手改主库权限）：

```powershell
# 删掉毒化/不可服务的 -shm，SQLite 下次打开会按 -wal 重建（无损）
adb shell "su -c 'rm -f /storage/emulated/0/Android/media/<pkg>/<dir>/<db>.db-shm'"
# 或让应用在系统视图里重建会话（干净关闭后重开），新边车由 MP 以 0664 创建
```

`own private view mount point unavailable ... root_dir= android_dir=` 与 `own private view
snapshot ...`：恢复在冷启动首轮挂载时失败了。这不是权限问题——快照里如果
`mounts=[...]` 已经列出 `/storage/emulated` 与 `/storage/emulated/0` 的 FUSE 挂载，
说明视图本身在位，拒绝来自 **MediaProvider 的 FUSE 层**（它只服务自己注册过的
uid/pid，模块 fork 出的挂载子进程无论 root 还是降权到应用 uid 都拿不到解析）。
MediaProvider 就绪窗口在冷开机后可达数分钟，窗口内恢复必然失败。模块会把失败记进
`tmp/own_private_pending/<包名>` 并按退避（3s×3→15s→30s）把该包重新排进增量挂载，
成功后标记自动删除；日志按 `own private restore retry pkg=... round=N` /
`own private restored` 追踪。预算分两类：窗口内的必然失败（快照里 `root_view` 链
不可达）不消耗预算、只按轮次退避（总轮数上限 120，约 55 分钟）；视图就绪后仍失败
才计入 20 次的放弃预算。若出现 `retry exhausted`，按 `ready_failures=` 区分：为 0
说明是窗口过长，非 0 说明恢复存在数据面/锚点层面的真问题，需按快照进一步定位。窗口期内应用仍可经模块数据面访问自有 media 子树（功能不受影响），只是
跨进程共享（如 XRadiant 这类被其它进程消费的 sqlite 库）要等恢复落位后才回到同一层
内核缓存。

## 公共存储里凭空多出应用自己的目录（沙盒根组漂移）

现象：应用明明开启了重定向（`enabled: true`），公共存储里却持续长出新目录，例如
`/storage/emulated/0/Documents/ringtone/gamefree/jly4/a/tone.xml`；应用侧 `open` 报
`EACCES`，`running.log` 里成对出现：

```
# quality-allow(chinese-language): 以下两行是模块日志原文，按原样保留供关键字检索
[RsWarn] auto mkdir storage parent failed .../<pkg>/sdcard/Documents/... errno=13
[RsWarn] redirect parent mkdir failed op=open dir=... errno=13
```

排查顺序：

1. 看沙盒根（重定向根）的组，不看它的属主：

   ```powershell
   adb shell "su -c 'ls -ldn /data/media/0/Android/data/<pkg> /data/media/0/Android/data/<pkg>/sdcard'"
   ```

   正常形态是 `<pkg>` 与 `<pkg>/sdcard` 的**组**都是 `ext_data_rw(1078)`（或某条同样可被
   MediaProvider 使用的组），`sdcard` 为 `2770`。
2. 对照 MediaProvider 的凭据：`grep -E '^(Uid|Groups)' /proc/$(pidof com.android.providers.media.module)/status`
   里只有 `media_rw(1023)`/`ext_data_rw(1078)` 这类附加组，没有任何应用 uid。
3. 若 `sdcard` 的组等于**应用自己的 uid**，就是组漂移：系统代写进程既非属主也不在组，
   `2770` 的 other 位又是 0，逐级遍历必然 `EACCES`；重定向目标不可达后写入回退到真实公共
   路径，目录就这样被建出来。`file_monitor.log` 同一时刻会给出 `open:write ret=-1 errno=13`
   与一条对真实路径的成功观察。

成因与修复：目录创建入口曾用 `chown(uid, uid)` 把父目录 setgid 继承来的组覆盖成应用 uid。
现在只改属主、组交给继承（等同原生 MediaProvider 的创建结果），存量由一次性迁移把
`Android/data` 下 `gid=应用 uid` 或 `gid=1023` 的无 other 权限位目录恢复为 `ext_data_rw`。
现场可先用 `chgrp 1078 <pkg>/sdcard` 临时验证：改完立刻正确落沙盒、零 `EACCES`，
不需要重启设备。回归由测试流场景 41 覆盖（删沙盒根 → 应用重启重建 → 元数据断言 + 媒体代写零穿透）。

## 外部存储整体卡死（读取/写入长时间不返回）

整机所有应用访问 `/storage/emulated/0` 都卡住（应用首屏不加载、壁纸/桌面服务 ANR、系统存储服务无响应）时，先分清是**卡死**还是**报错**——这两类现象的根因完全不同，误判会把排查带到错误方向：

| 形态 | 含义 | 典型 errno |
|------|------|-----------|
| 访问立刻失败 | 挂载记录还在，但背后的 FUSE 连接已经断了（服务进程退出或被 abort） | `ENOTCONN`(107) / `EIO` / `ENODEV` |
| 访问长时间不返回（进程处于 D 状态） | FUSE 连接仍在、**服务端活着但不应答**：请求排队后没人回复 | 无 errno，只有超时 |

模块自己的挂载出问题属于第一种（`srx_fuse_host[...]` 的层会快速报错，由宿主自愈与重挂处理）。**整机卡死几乎总是第二种**，而 `/storage/emulated/<user>` 的服务端是系统的 `com.android.providers.media.module`（MediaProvider 的 FuseDaemon），不是模块。定位顺序如下。

1. 找到哪个连接在积压：

   ```bash
   adb shell "su -c 'for c in /sys/fs/fuse/connections/*; do echo \"\$c waiting=\$(cat \$c/waiting 2>/dev/null)\"; done'"
   ```

   `waiting` 持续增长且不回落，说明该连接的请求无人应答。连接号就是 `/proc/<pid>/mountinfo` 里 `0:<N>` 的 `<N>`，据此可判断是哪个挂载点、谁在提供服务：

   ```bash
   adb shell "su -c 'grep -E \" /storage/emulated|srx_fuse\" /proc/1/mountinfo'"
   ```

   `source` 为 `srx_fuse_host[<pid>]` 的是模块宿主会话；其余 FUSE 挂载（`/dev/fuse`、`/mnt/user/...`）由系统 MediaProvider 提供。

2. MediaProvider 侧取证（判断它是否卡在自己的请求里）：

   ```bash
   adb shell "su -c 'debuggerd -j \$(pidof com.android.providers.media.module)'"   # Java 栈
   adb shell "su -c 'debuggerd -b \$(pidof com.android.providers.media.module)'"   # native 栈
   ```

   若 FuseDaemon 的工作线程停在读取下游文件系统/等待锁上，说明卡点在 MP 自身或它的下游。

3. 模块侧取证：模块宿主有独立的停摆证据文件，出现停摆与换代时会写全线程栈与挂载现场：

   ```bash
   adb shell "su -c 'tail -50 /data/adb/modules/storage.redirect.x/logs/stall-forensics.log'"
   ```

   没有新的停摆记录，说明模块宿主没有参与这次停摆。

4. 归因对照（确认模块是否是故障来源）：创建落盘开关后重启 daemon，模块将不再建立共享宿主会话，应用全部走 namespace bind：

   ```bash
   adb shell "su -c 'touch /data/adb/modules/storage.redirect.x/.disable_fuse_host'"
   adb shell "su -c 'kill \$(pidof srx_daemon)'"
   ```

   创建后 `running.log` 会出现 `fuse host disabled by marker`。若卡死依旧，问题不在共享宿主数据面；判断 MediaProvider 进程内的模块 hook 是否为来源，用同样的方式对照 `.disable_mp_hooks`（跳过 MP 进程内全部模块 hook）与 `.disable_fuse_fix`（只跳过 MP 的 FUSE 修复）。这些标记文件删除后重启生效，不需要重刷模块。

## 常见崩溃原因

| 症状 | 原因 | 解决 |
|------|------|------|
| MediaProvider SIGABRT | Hooker.dex 残缺（缺少内部类） | 验证 dex 大小大于等于 10KB |
| MediaProvider FUSE 零宽修复未生效 | `fuse_fix_enabled` 被关闭，或 MediaProvider 尚未重启加载最新 hook | 保持 `fuse_fix_enabled=true`，重启 MediaProvider 或设备后重新验证 |
| android.process.media SIGBUS | Java hook 安装在非 MediaProvider 的 shared UID 进程中 | 检查 `is_media_provider_package` 逻辑 |
| 模块加载后无任何日志 | NDK/LSPlant 未正确链接 | 验证 so 大小大于等于 1.8MB |

## 禁用模块

如果模块导致设备 bootloop，可创建 `disable` 文件禁用模块：

```bash
# 通过 adb（如果可用）
adb shell "su -c 'touch /data/adb/modules/storage.redirect.x/disable'"
adb reboot

# 通过 Termux（设备上）
su -c 'touch /data/adb/modules/storage.redirect.x/disable && reboot'
```
