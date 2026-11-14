# 自动后端与 FUSE 挂载架构

本文记录从早期“FUSE-first”设计到当前自动后端实现的演进。配置面固定使用 `auto`，运行时在**共享 FUSE 宿主会话**与 mount namespace 之间选择；per-app scoped FUSE 会话已退役（它会让应用命名空间被自己的会话进程自持，见 [共享宿主架构](shared-fuse-daemon-architecture.md)）。

## 运行方式

- `auto`：**FUSE 优先**——设备存在可用 `/dev/fuse` 时，整棵存储视图根由共享宿主 FUSE
  会话接管，映射、通配、沙盒、只读等规则全部在策略层逐请求判定；只有能力探测判定不可用
  （或该应用自己的失败预算耗尽）时才返回空根，由挂载层回落 namespace bind。
- 用户选择面已收敛：配置里只写 `auto`，历史 `fuse` / `namespace` 值按 `auto` 处理。

配置位于 `config/global.json`：

```json
{
  "storage_backend_mode": "auto"
}
```

旧版 `fuse_daemon_redirect_enabled` 不再读取；新建、迁移和管理端保存的配置只写入 `storage_backend_mode=auto`。

## 会话归属与能力熔断

- 挂载源已原生化：共享宿主视图根与系统 MediaProvider 的 FUSE 同形（源 `/dev/fuse`，类型 `fuse`），
  挂载表里不再出现模块私有源名。「哪个挂载是本模块的」改由模块私有账本（`mount_ledger` 的 dev
  集合，持久化在模块目录下）记录，应用侧读不到。per-app scoped 会话（`srx_fuse_redirect[<pid>]`）
  已退役，历史残留由 daemon 启动时一次性清理。
- 收尾按账本里记录的挂载 ID 与顶层 dev 确认归属；同路径已被回收替换或已摘除时跳过卸载，不会摘掉
  接管者的挂载。
- 接入失败受按应用分桶的预算约束（连续 3 次后 60s 退避窗口，窗口过后放行一次探测、成功即清零；
  「没试」类判定不计入）；设备级 `unavailable` 需要 ≥2 个不同应用的失败（`schema=3` 能力快照，含 `state`、`reason`、`device_fail_count`、`last_failed_scope` 等），单应用失败不会锁出整机。设备级锁出后按 `retry_at_ms` 退避自愈（30s 起倍增封顶 10 分钟，无需重启），任意成功都会清零回 FUSE；daemon 启动会重置快照。管理端、WebUI 与诊断归档只读取 `state` 与 `reason`。

## 优点

- FUSE 请求在文件操作开始时执行策略，通配、动态文件和只读排除不依赖挂载时的目录快照。
- 挂载请求会先写入 `tmp/mount_intent/*.intent`，再更新为 `applying`、`mounted` 或 `failed`；正式 mount state 仍只保存可清理的已生效目标。
- 路径映射、只读和沙盒规则共用一个数据面，减少 namespace 多层 bind mount 的组合复杂度。
- 能力探测只决定「能不能用 FUSE」：可用即接管整根视图，不可用才回落 namespace。
- mount namespace 仍作为自动回退路径，便于跨厂商设备保持兼容。

## 缺点

- FUSE 引入用户态往返，随机读写和高频小文件操作可能增加延迟；启用 passthrough 只能优化已允许文件的数据面，不能消除策略判断开销。
- FUSE 服务退出会影响对应挂载根，必须依赖健康检查、熔断和 namespace 回退。
- 整根接管后所有应用 IO 都经 FUSE，厂商 MediaProvider、内核 FUSE 与 SELinux 差异需要真机验证。
- FUSE 运行时属于共享链路，策略或资源泄漏的影响面大于单个应用的 bind mount。

## 当前状态与维护边界

`auto` 是所有设备的唯一配置模式；测试流会记录每个场景的 `backend_effective`，并保留 FUSE cache 容量、mount intent 和原始运行日志。系统 MediaProvider/系统 writer 仍使用现有调用方识别 hook，普通应用不安装进程内 PLT hook。

当前实现的关键边界如下：

- 共享宿主会话（`srx_fuse_host[<pid>]`）是**唯一**的 FUSE 数据面，为多个应用 namespace 提供共享视图，Auto 模式下应用会话默认收敛到该会话。
- `mount namespace` 不是待删除的旧实现，而是 `auto` 后端在宿主接入不可用时的正式路径。
- `srx_fuse_redirect` 前缀保留用于识别旧版本遗留的 scoped 层；测试、账本和诊断必须同时识别 `srx_fuse_redirect` 与 `srx_fuse_host`。

## 排查：某个应用为什么没走 FUSE

先看 `backend_effective`：它给出该应用最终生效的形态与判定来源，一条就够定位方向。

```
backend_effective pkg=<包名> requested=auto effective=fuse capability=available   selection_reason=scoped_mount_ready:view=srx_host fuse_roots=1 fuse_sessions=1
backend_effective pkg=<包名> requested=auto effective=namespace capability=available   selection_reason=scoped_mount_failed_namespace_fallback:attach_gate_closed fuse_roots=1
```

- `selection_reason=scoped_mount_ready:view=srx_host`：已接入共享宿主，后续落点由策略层决定。
- `...failed_namespace_fallback:<reason>`：接入没成功并回落 namespace。`reason` 直接说明卡在哪一步
  （闸门判定、策略登记、接入超时、宿主缺席）。
- 只有 `unknown` 或失败信号才输出 `scoped gate diag`（含 `capability`、`budget_exhausted`、
  `scope_failures`）；正常放行不打印，避免启动风暴期日志风暴。

常见成因按出现频率排列：

- **能力快照 `unavailable` 且设备级退避未到期**：等 `retry_at_ms` 过去，或重启让快照重置。
- **该应用自己的失败分桶用满预算**：连续三次真实接入失败后进入 60 秒窗口，窗口过后仍会放行一次探测，
  成功即清零；「没试」类判定（根不满足接入条件、宿主尚未就绪）不计入预算。
- **接入超时**：宿主策略登记或 namespace 接入没在预算内完成，应用会带 `host_attach_failed` 回落。
  注意与另外两种「没接入」区分：`no_explicit_rules`（全隔离配置，按设计走 namespace bind，
  更省电）与 `attach_gate_closed`（有显式规则但规划根/映射不适合 FUSE）。
- **接入探针超时但挂载已就位**：接入子进程最后一步会用应用凭据 stat 一次刚附着的视图根，
  那是一次同步 FUSE 往返，而设备上单条请求偶发 20~60 秒。现在超时后会**先读 mountinfo
  比对会话设备号**：已就位即判成功（日志 `fuse host attach probe timeout but mount landed`），
  不再误判成失败后回落 bind 而叠出第二层。

### 挂载结论分两级，且是「观测」不是「等待」

应用侧 specialize_post 的结论只有两种：`app mount confirmed`（观测到重定向已生效）与
`app mount unconcluded`（**这一刻**观测不到证据——没有结论，不是失败，info）。`unconcluded`
在应用启动快于挂载落定时会出现，而应用随后通常就能看见自己的视图（真机实测：同一次启动里
该 pid 稍后 `/storage/emulated` 的设备号就是本会话的）。两条都带 `pkg=`，`unconcluded` 另带
`map_only=` 与 `sent=`（请求是否真的发出去过）。

**执行方的结论由执行方自己记**：`perf companion mount ... ok=true|false`、
`companion fuse view unavailable reason=...`、`daemon fuse view unavailable reason=...`。
应用侧不再有「失败」这一级，因为它拿不到执行方的结论（见下条）。

### 为什么应用侧不等「挂载执行方回执」

曾经的设计是：应用发出挂载请求时保留那条连接，伴生进程挂载落定后写回 4 字节结论，应用侧阻塞
读一次。**这条通道在本项目里不可能成立**——不是实现问题，而是 SELinux 的域迁移：

- 请求在 `pre_app_specialize` 发出，此时进程属 `u:r:zygote:s0`，socket 也由该域创建；
- 应用 specialize 后整个进程切到 `u:r:untrusted_app:s0`，此后读这条继承来的 socket 会被直接
  拒绝。真机 AVC 原文：

  - 请求行：`avc: denied { read } for comm="one:gpu_process" path="socket:[207528]" dev="sockfs"`
  - 上下文：`scontext=u:r:untrusted_app:s0:c18,c257,c512,c768 tcontext=u:r:zygote:s0`
  - 对象类：`tclass=unix_stream_socket permissive=0`

应用侧拿到的是 `-EACCES`，而执行方在同一毫秒里已经 `companion ack sent status=1`——数据确实写
出来了，只是读不到。`recv`/`readv`/`splice` 走同一个 `file_permission` 钩子，换读法无用。

所以现在**发完即关**：连接不在应用进程里多留一个 fd、不跨域边界，应用侧也就不会因为一次注定
失败的读而在 dmesg 里留下可观测的审计行。判据回到「挂载表里能不能看见自己的视图」：

- 沙箱根挂在 `root` 字段（命名空间 bind 后端）；
- 或者视图根的**设备号**与系统媒体 FUSE 不同（共享宿主会话已接管，见下条）。

三条判据都不含模块标识，且不等待、不轮询、不占预算。

### 视图根判据必须比对设备号，不能只看「这一层是 fuse」

Android 11 起 vold 自己就用 fuse 挂 `/storage/emulated`，所以「视图根是 fuse 挂载」在每台
现代设备上都恒为真。真机实测（2026-11-09）：一个回落 `namespace_bind` 的应用因此被判成
`app mount confirmed`——判据根本没区分系统视图和我们会话。

正确对照物是**系统媒体 FUSE 的超级块设备号**：`/mnt/user/<user>/emulated` 上最先出现的
那一层（vold 开机挂的那层），我们的会话是叠在它上面的新层。同一路径的最顶层才是应用看到的
视图，两者设备号不同即已接管。同一台设备同一时刻的两组样本：

```text
已接管：/storage/emulated        -> 0:142（本会话）
       /mnt/user/0/emulated 首层 -> 0:131（系统）
未接管：/storage/emulated        -> 0:131（系统）
       /mnt/user/0/emulated 首层 -> 0:131（系统）
```



### 判断「接管是否成功」不要看挂载源名

挂载源已原生化（`/dev/fuse`），`srx_fuse_host[...]` 这类源名判据在共享宿主布局下恒落空。
用拓扑事实判断，两种布局都覆盖：

- **bind 布局**（回落 namespace）：规则落点本身是该进程的挂载点。
- **共享宿主布局**：应用视图根 `/storage/emulated` 的超级块 dev 与 init 命名空间里系统 FUSE 的 dev
  不同，即视图已由模块会话服务；规则落点不会在 `mountinfo` 里出现独立挂载层，这是设计目标。

### 接管后的视图根契约：条目集合与系统 MediaProvider 逐项一致

应用一旦落在模块会话上，它的根目录枚举就由我们生成，因此必须复刻系统视图的可观察行为，
否则应用只要比对 `getdents` 结果就能分辨“存储被接管”，甚至把内部目录当成用户数据：

- **根级隐藏**：`.transforms`、`.picker_transcoded`（MediaProvider 自建的工作目录）以及
  探测临时点 `.srx_passthrough_probe` 不出现在根列表里。判定必须用**归一后的存储相对路径**
  判空：视图根上提后用户根在视图里是 `<user>` 这一级，按视图相对路径判空会整条跳过隐藏
  （2026-11-14 的 `.transforms` 根级泄漏即此原因）。
- **只藏这两个名字**：`.trash-storage`、用户自建的 `.foo` 等点目录必须照常可见。把根级过滤
  写成“藏掉所有点条目”同样属于行为不兼容。
- **readdir 不返回 `.` / `..`**：系统 MediaProvider 视图本来就不返回它们；VFS 的路径解析
  自行处理，不依赖文件系统给出。
- **挂载栈只有一层**：接入按「替换而非叠加」实现——先把目标路径上的既有层逐层摘到空
  （摘前克隆原栈顶作回滚句柄），附着后复核 `/storage/emulated` 上层数 == 1，不等就整体回滚。
  叠层会让视图内容与基线不一致，且上层归属不可判定。

这四条由设备场景钉住：43（配置态接管）、44（监视态接管）、23（停用配置不接管）；
具体判据见[设备侧测试说明](device-testing.md)。

### 映射类问题的两个前提

- **映射目标目录必须在接入前存在于真实存储**：共享宿主布局没有「建立映射挂载」这一步，
  daemon 与 companion 两条接入路径都会在接入前预建目标目录。若目标目录缺失，
  系统代写链路（如下载提供器）会判定父目录不存在而直接放弃。
- **映射按链式语义解析**：父映射的结果若又命中另一条映射的请求路径，会继续解析到不动点；
  自嵌套映射（目标落在自己请求路径的子树内）只生效一次。排查时按「最终落点」核对，
  而不是按第一跳目标。
