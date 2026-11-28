# 排障说明

本文汇总运行时日志、关键标记和常见崩溃原因。设备侧验证流程请看 [设备侧测试说明](device-testing.md)。

## 查看模块日志

详细日志开启时，native 日志优先写入 `/data/adb/modules/storage.redirect.x/logs/running.log`，文件监视记录写入 `file_monitor.log`。`adb logcat -s "SRX" "StorageRedirect"` 主要用于查看 Java 日志、native Warn/Error 镜像和私有通道不可用时的文件监视回退，不再是完整 native 日志来源。

详细日志关闭时 `running.log` 只保留告警与错误、文件监视记录、概览计数和低频状态事件（挂载完成、热重载回执），逐请求的诊断轨迹（挂载视图、别名绑定、资源根展开、扫描未命中命名空间等）只在开启详细日志后写入。

也可以在管理 App 的设置页右上角点击文件图标导出日志包。导出的 `storage-redirect-x-logs-*.tar.gz` 会包含模块日志、轮转历史、关键配置、运行状态快照、最近相关 logcat 和 dmesg tail，适合在 Issue 或排障沟通中直接提供。公共存储视图不可写时（存储异常的典型现场），导出会自动降级把日志包写到模块目录 `logs/` 下，此时需要用 root 文件管理器或 `adb` 手动取出。导出在采集挂载与路径快照时会为可能被存储停摆拖住的命令加硬超时，超时只丢弃该段证据，不会让整包导出失败。

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
| `identify_method=caller_uid` | 请求 UID 唯一对应一个有效包时补全包名；共享 UID 含多个包时不按成员顺序选择 |
| `identify_method=caller_pid` | daemon 按原请求任务的凭据、启动时间和包 UID 核验来源；普通应用派生的 shell/native 子进程可以通过进程组关联到应用，命令行参数不作为包身份依据 |
| `identify_method=executor` | root、shell 或已知 su/特权域停止向发起应用追溯；保留执行者名称及 `caller_uid`、`caller_pid`、`caller_start_time`、`caller_context`、`caller_boundary`，不按旧路径提示补成应用 |
| `identify_method=shared_uid` | 无法区分具体成员时显示 PackageManager 的真实共享用户标签，可靠性为 `fallback`；组名仅供监视，不参与应用配置或重定向来源选择 |
| `identify_method=unknown` 或 `identify_reliability=none`，且落盘方是系统代写进程 | 应用名显示"未知来源"，详情行保留代写进程与已有请求字段；任务不可核验时带 `caller_boundary=unverified`，不按 UID 补成应用 |
| 文件系统观察类记录（inotify 侧） | 显示"文件系统观察"，观察器与实际落盘进程分开列出 |
| `identify_method=saf_provider` | 通过系统文件选择器保存的文件：调用方是选择器的发起应用（A17 取 `getLaunchedFromUid`，A16/A15 回退到 `getCallingPackage`/`getReferrer` + PackageManager 反查），可靠性为 `high` |

原请求任务的 UID 快照优先于应用 fsuid：任一 UID 为 root 时停止向应用追溯；任一 UID 为 shell，或 SELinux 域为 `shell`、`su`、`magisk`、`ksu`、`kernelsu` 时按对应执行者边界处理。代码不按 Shizuku、Sui 或 rish 的产品名称分类；这类工具的结果取决于任务实际 UID 与 SELinux 域。普通非特权域变化不会触发特权边界；任务不可核验时标为 `unverified`。停止追溯不删除请求记录，也不改变自有 FUSE 按挂载命名空间选择重定向规则的行为。

身份查询按内核发送方凭据限制目标：root 与已核验的系统代写者可跨 UID 查询，普通进程只能查询自身 UID 的任务。查询与监视、统计记录共用发送方配额；查询通道忙或超时保持未解析，不等待其它线程的查询完成，也不借旧路径提示补成应用身份。

原生 Binder 调用方查询只读取当前线程已有的 `IPCThreadState`；尚未建立时继续使用已有请求信号或保持未解析，不在文件 hook 内创建 Binder 状态。

日志派生的公共路径提示只接收成功、具体包名、UID 一致且可信度为 `high` 的请求。Provider 在操作前登记的路径提示用于延续路由，不代表操作成功。共享组标签与执行者边界不作为应用包名登记。管理 App 的日志详情保留原任务 UID、PID/TID、启动时钟、SELinux 域与边界，组名和执行者不会请求应用图标或打开应用配置页。不同请求身份的记录分别保留；请求与文件系统观察仍可按关联 ID 配对。

## 判断操作有没有走重定向

`open` / `mkdir` / `unlink` 记录末尾带 `action=`：`redirect`（走了重定向，`from=`/`backend=` 给出原路径与实际路径）、`passthrough`（该应用没配规则）、`allow`（有规则但路径不在范围）、`rule_miss`（路径在映射范围内却没被重定向）、`deny`（被只读规则拦下，带 `rule=read_only`）。字段定义见 [monitor-record-fields](monitor-record-fields.md)。

新监视记录带 `field_encoding=percent`，字段内的 `%`、`|`、回车和换行分别编码为 `%25`、`%7C`、`%0D`、`%0A`。管理 App 在分字段后解码，没有该标记的旧记录保留百分号字面值。重复字段或 `record_integrity=ambiguous` 表示记录存在冲突，界面显示“结果未知：日志字段冲突”，不把它显示为成功或补成应用身份。创建成功与写入失败分别保留，关联 ID 也不能覆盖不同请求身份或结果。

原生执行者名称保留 UTF-8 与标点，进程列、调用方列和任务字段使用同一套转义。daemon 补写旧记录身份时会升级字段编码，同时保留旧百分号的字面含义。请求去重区分原任务身份，不同 PID、启动时钟、域或执行者边界的记录分别保留。

"未知来源"表示这次写入确实无法归因到具体应用：典型来源是经 MediaProvider 中转、调用方信号已经失效的写入（系统后台创建的目录），以及 root 文件管理器绕过 FUSE 直接落盘的写入。它不是"没有记录"，路径、进程与结果仍然完整，可据此对照 `running.log` 与 `file_monitor.log` 定位。

Provider 请求作用域保留框架校验过的调用包名以及 UID/PID；共享 UID 的不同包不会被 UID 查询结果覆盖，未命中自身只读规则时也不借用另一成员的规则。异步目录提示保留原任务快照，仅在宿主内部调用时恢复，并重新核验任务代次与凭据；冲突或失效的提示不补造应用身份。作用域覆盖实际目录操作，嵌套退出恢复外层身份。下载代写按当前下载行的 `notificationpackage` 与完整 UID 校验包名和用户；同一路径存在不同归属的行时保持未知，仅在原路径没有下载行时查询伴随文件别名。跨线程记录保留原请求的完整任务快照，不用于重定向判定。

## 应用列表刷新失败 / 应用名为空（包清单查询）

管理 App 的应用列表、日志页里的应用名都来自同一份 system_server 包清单（`srx_daemon package-query A <user>`）。这条链有两处独立缺陷，都会表现为"列表空、应用名退化成包名、日志页中间进程显示未知来源"：

1. **通道会解码 `%XX`**：历史 WebUI 取 stdout 的通道曾把整段输出当 URL 解码一次（KernelSU/APatch 用 `loadUrl("javascript:…")` 回传结果），清单里每一处 `%2F`、`%E7%B3%B%` 这类转义在到达 JS 之前就被还原成裸字节，percent 编码协议随之失效。该 WebUI 通道已移除；当前管理 App 与模块脚本使用纯文本 stdout，不经过这条解码链。现场判据：拿到的文本是 `0\t1000\t1\tandroid\tAndroid 系统\t/system/…`（制表符还在、转义已消失）就是它。
2. **期限与成本脱钩**：界面清单要枚举该用户全部应用并逐个解析标签，成本随应用数线性增长（实测旗舰机 467 个应用的冷查询 1.9 秒，且间隔 70 秒后重现），而旧实现给整条连接设的是 2 秒绝对期限，慢就等于失败。现在 `A`/`T` 用 15 秒预算，身份反查类（`P`/`U`/`S`/`L`）仍是 2 秒。

| 现象 | 判据 |
|------|------|
| 列表空白，提示"刷新应用列表失败" | `[RsWarn] package query failed request=A <user> error=... euid=... budget_ms=...`——优先看 `logs/running.log`；它经 daemon 私有通道落盘，通道拥塞时会退到 logcat |
| 日志页应用名退化成包名，中间进程显示"未知来源" | 同一根因：应用名由这份清单填充，清单失败后只剩包名，系统代写进程按身份未知渲染 |
| `error=package query deadline` / `error=package connect` | 该机枚举或连接越过期限；按下面命令实测耗时 |
| `error=... Operation not permitted euid=<非 0>` | 调用方不是 root，与模块无关 |

手工复现（冷启动后第一次才有代表性）：

```sh
MOD=/data/adb/modules/storage.redirect.x
time $MOD/bin/srx_daemon package-query A 0 | wc -l          # 纯文本：App 与模块脚本都用这个
```

连续几次都是秒级而不是毫秒级，说明 system_server 侧没有复用缓存。实测间隔几十秒后的下一次查询就是冷查询（duchamp 上紧邻两次 69 ms，间隔 70 秒后 1014 ms），所以用户每刷新一次都要付一次冷枚举的代价。日志包里若出现 `SrxPackageQuery: package lookup failed`，按堆栈看服务端异常；没有这一行说明请求根本没到服务端，问题在客户端连接（socket 缺失、SELinux、调用方非 root）。

同一通道还要读 `file_monitor.log` 一类的文本文件（`cat`），那里的 `%XX` 同样会被解码。当前只在路径本身含 `|` 时影响字段划分（`%7C` 被还原成裸 `|`），显示内容不受影响；这是与上面同一类问题的残留，修的时候按同样思路（换承载形式，或让写侧不再依赖 percent 编码）。

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
不透传应用 umask。owner-only 边车只可能来自「绕过 MP 的直连层」，两条路径都要看：

- 自有私有目录**恢复失败**（日志 `own private restore failed ... errno=2`）后应用在 f2fs 上按
  umask 077 自建边车；
- 恢复**源取错层**：若恢复源不是 MediaProvider 的 FUSE 而是 f2fs 直连后端
  （`/mnt/pass_through/<user>/emulated`），目录内容虽落在同一份后端、但服务方不是 MP，
  应用在其中新建的边车同样按自身 umask 落 `0600`。恢复源现在固定取自 **init 命名空间**的
  系统视图（模块从不挂载该命名空间，那里的 `/mnt/user/<user>/emulated` 恒为 MP 的 FUSE），
  因此正常情况下不再出现这一形态。
- 恢复**锚点没建起来**：锚点只有一种合法取法——在 init 命名空间里
  `setns(/proc/1/ns/mnt)` → `open_tree(OPEN_TREE_CLONE)` → `SCM_RIGHTS` 交回宿主 →
  `move_mount` 附着。**不得用 `mount(MS_BIND)` 替代**：内核 `do_loopback` 的 `check_mnt`
  要求挂载源属于调用方**当前命名空间**，而 `/proc/1/root/...` 是跨命名空间 magic link，
  bind 必然失败，锚点拿不到、自有私有目录就整段没交回系统视图（真机 2026-11-13 AVD(A15)
  实测：回退分支 `bind failed source=/proc/1/root/mnt/user/0/emulated` 之后
  `own private system view restored count=0`，场景 35 判 `file_missing`）。
  锚点建立函数被 companion 与 daemon 各调用一次，而克隆随 `move_mount` 一次性消费，
  第二次必然落到回退分支，所以**回退分支也必须重新克隆**，`/proc/1/root/...` 只作 `stat` 探针。
  与恢复失败同类判据：`own private restored` 的 `count=0` 即整段未恢复。

现在的处置：

- 模块自己的创建入口（宿主 FUSE 的 app-private 后端）按 MP 的原生创建口径产出 `0664`/`0775`——
  这条口径只在**新建**时决定落盘权限，等同系统视图自己建出来的结果；
- 把自有 media 子树交回系统视图之前，模块会把该子树里 MP 打不开的 sqlite 边车按**组位镜像**
  补齐（日志 `own private sqlite access repaired count=... root=...`，warn 级），使 MP 能以
  自身身份打开；这一步只补组位、不改属主，也不触碰主库内容，历史遗留边车因此无需手工处理；
- 补不齐的（例如被其它绕过系统视图的写入路径持续产生）仍按
  `own private sqlite entries unserviceable by system view count=... sample=...`（详细日志级别）
  上报数量与样本路径，便于定位来源；
- MP 侧 sqlite 干预只在 MP 自己打得开该入口时才接管，`-shm` 毒化检查改走 `O_PATH` 尺寸判定，
  owner-only 入口不再拖垮毒化自愈链。

历史遗留 owner-only 边车的手工处置（任选其一，都由应用自己重建即可，不要手改主库权限）：

```powershell
# 删掉毒化/不可服务的 -shm，SQLite 下次打开会按 -wal 重建（无损）
adb shell "su -c 'rm -f /storage/emulated/0/Android/media/<pkg>/<dir>/<db>.db-shm'"
# 或让应用在系统视图里重建会话（干净关闭后重开），新边车由 MP 以 0664 创建
```

**只走视图路径，不要从后端删。** `rm /data/media/.../<db>.db-shm` 绕过 MediaProvider 的节点
簿记，会把这个名字记成「不可创建」：之后同名 `O_CREAT` 恒 `ENOENT`，同目录换个新名字立刻成功，
只有重启 MediaProvider 才恢复（2026-11-25 真机复现）。模块的两处 `-shm` 自愈已改为
「删除后立即补 0 字节同名文件」，并会在主库存在时把缺失的 `-wal`/`-journal`/`-shm` 一并补位
（`.db` 不补），因此不再产生该状态；手工处置同样请只走视图路径。

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

## 文件管理器里能看到 `.transforms` / `.picker_transcoded`

**现象**：某个应用（典型是文件管理器，如 MT 管理器）的存储根列表里出现 `.transforms` 或
`.picker_transcoded`，而系统自己的文件管理器看不到；关掉应用重开又恢复正常。

**原因**：这两个目录是 MediaProvider 的工作目录，系统 FUSE 视图在**根级**对应用隐藏。模块视图
若按错误的坐标系判根（视图根未上提时挂载根就是用户根，上提后用户根变成 `<user>` 这一级），
整条隐藏会被静默跳过。2026-11-14 的回归正是这个形态：应用本身没有开启重定向，只是在“别的应用
改了规则”触发的那一轮 reconcile 里被监视会话**首次**接入模块视图，于是立刻暴露。

**自查**：

```bash
# 应用此刻在系统视图还是模块视图：两个设备号不同即已接管
adb shell "su -c 'pid=\$(pidof <包名>); stat -c %d /storage/emulated/0; nsenter -t \$pid -m -- stat -c %d /storage/emulated/0'"
```

**处理**：升级到包含该修复的版本（根级判据按归一后的存储相对路径判空、readdir 不合成点条目）。
升级后仍能复现时，抓 `running.log` 的 `fuse host attach` / `backend_effective` 片段与
`/proc/<pid>/mountinfo` 一起反馈——需要区分“视图根上有多层挂载”和“根级过滤未命中”两种成因。

## 禁用模块

如果模块导致设备 bootloop，可创建 `disable` 文件禁用模块：

```bash
# 通过 adb（如果可用）
adb shell "su -c 'touch /data/adb/modules/storage.redirect.x/disable'"
adb reboot

# 通过 Termux（设备上）
su -c 'touch /data/adb/modules/storage.redirect.x/disable && reboot'
```
