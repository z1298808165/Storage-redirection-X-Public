# 共享 FUSE daemon 架构设计

本文记录 FUSE 挂载从“每个应用各自起一个服务”演进到“共享宿主会话 + 独立监督与
namespace 注入”的架构、根因分析和当前实现边界。早期分阶段迁移计划保留作为设计历史；其中身份账本、策略注册表、共享宿主接入和基础监督已经落地，文中的“目标架构”应按当前状态阅读。

参考实现：`huniangitb/Fuse-Proxy`（Android 13+ 用户态 FUSE 代理 + 挂载命名空间隔离）。
该项目的 `injector` / `fuse_daemon` 分离结构是本方案的直接参照，差异见 §3。

---

## 1. 现状与失效窗口

### 1.1 当前结构

```
zygisk specialize_pre ──connect_companion──> root companion ──> companion_mount.rs（按应用挂载）
srx_daemon（持久）      ──reconcile 每 3s──> daemon_mount.rs
                                              └─ 共享宿主会话：FuseHost 建立 srx_fuse_host；应用侧 open_tree + move_mount 注入，失败时回退 scoped srx_fuse_redirect
```

- 共享宿主会话：`src/fuse_host.rs` 在 daemon 启动时建立私有 mount namespace 和宿主 FUSE 会话，挂载源为 `srx_fuse_host[<pid>]`。
- 应用接入：`src/daemon_mount.rs` 与 `src/lifecycle/companion_mount.rs` 优先通过 `open_tree` + `move_mount` 注入共享宿主（默认关闭）；宿主接入失败时回退到原有 scoped FUSE，会话源为 `srx_fuse_redirect[<pid>]`。
- 旧 scoped 路径仍由 `src/fuse_redirect/config.rs` 提供兼容实现，但不再是唯一数据面。
- 状态落盘：`src/daemon_mount.rs:1643 write_mount_state`，字段为
  `version/package/uid/app_start_time/fuse_child=<pid>:<starttime>/target=`。
- 周期 reconcile：`src/daemon.rs:420 reconcile_running_apps`，间隔 3 秒
  （`PERIODIC_RECONCILE_INTERVAL_MS`）。
- 健康判定：`src/daemon_mount.rs:130 has_healthy_mount_state` →
  `has_dead_fuse_child`（比对 `/proc/<pid>` starttime）+ `mount_targets_present`
  （读 `/proc/<pid>/mountinfo`）+ `backend_mount_targets_responsive`（`open()` 探测 errno）。
- 死挂载清理：`src/fuse_redirect/config.rs:596 finish_failed_session`，ENOTCONN 不在
  `is_already_unmounted_errno`（`:643`）里，必须继续 `MNT_DETACH`。

### 1.2 三个失效窗口

**窗口一：服务退出到 reconcile 发现之间有 3 秒以上的黑洞。**
服务进程异常退出后，挂载记录仍留在应用 namespace 里，应用访问 `/storage/emulated/0`
返回 ENOTCONN。daemon 要等到下一个周期（3 秒）才通过 `backend_mount_targets_responsive`
的 `open()` 探测发现，再走一轮完整重挂。这段时间内应用的所有存储访问都是硬失败。

**窗口二：mount namespace 分裂后没有任何判据能识别。**
状态文件只记录 `app_start_time` 与 `fuse_child=pid:starttime`，**不记录 namespace 身份**。
应用重启或 namespace 被替换后，旧 namespace 的挂载随其销毁，但状态文件里那些
`target=` 记录仍然存在。daemon 无法区分"挂载还在但连接断了"与"挂载已随旧 namespace 消失"，
只能按路径盲试卸载。

**窗口三：卸载失败后继续挂载会叠加，而不是收敛。**
`src/daemon_mount.rs:1416 clear_mount_target_stack` 按**路径**统计挂载层数
（`current_mount_target_count`），对最顶层是"谁的挂载"没有任何判断：
- 卸载失败只 `log::warn!` 后返回 false，调用方 `handle_child_process` 仍然继续挂载
  → 同一路径叠上第二层、第三层；
- 每轮 reconcile 都会再叠一层，`MAX_UNMOUNT_PASSES_PER_TARGET = 32` 的上限只是把
  "无限叠加"变成"最多 32 层"；
- 反过来，如果同路径被本模块的新会话或系统 MediaProvider 的 FUSE 接管，按路径卸载会把
  接管者的挂载摘掉，应用立刻看到 ENOTCONN。

---

## 2. 目标架构

```
┌──────────────────────────────────────────────────────────────────┐
│ srx_daemon（持久，唯一实例，flock 保护）                            │
│                                                                  │
│  ┌─ FuseHost ────────────────────────────────────────────────┐  │
│  │ 一个共享 FUSE 会话，挂载在私有宿主路径                       │  │
│  │   source = srx_fuse_host[<pid>]                            │  │
│  │   私有 mount namespace（unshare(CLONE_NEWNS)+MS_PRIVATE）   │  │
│  │   策略按调用方 uid 解析（uid → AppConfig）                  │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌─ Injector ────────────────────────────────────────────────┐  │
│  │ setns(目标 ns) → bind-mount 宿主树到 /storage/emulated/N    │  │
│  │ 同一份 FUSE 会话被 N 个 namespace 共享（共享 superblock）    │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌─ MountLedger 挂载身份账本（src/mount_identity.rs）────────┐  │
│  │ 记录 mount_id、source、ns(dev:ino)、starttime、代数         │  │
│  └───────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌─ Supervisor 服务监督（src/fuse_supervisor.rs）────────────┐  │
│  │ 端点健康探测 → 恢复动作规划 → 摘除失败预算 → poisoned 收敛   │  │
│  └───────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

### 2.1 一个 FUSE 会话，N 个 bind mount

FUSE 挂载的本质是一个 `vfsmount` + 一个 `fuse_conn`。把宿主挂载 **bind mount** 到另一个
mount namespace 后，两边共享同一个 superblock 与同一个 `fuse_conn`，请求全部回到同一个
daemon 进程。因此：

- **应用 namespace 分裂不再影响 FUSE 会话**：会话活在 daemon 自己的 namespace 里，
  应用的 namespace 怎么换都只是重新 bind 一次；
- **卸载一个应用的注入不会影响其它应用**：`umount` 的是 namespace 局部的 bind 挂载，
  不是共享的 FUSE 会话；
- **服务退出是单一事件**：不再有 N 个各自独立的失败窗口，而是一个被监督的会话；
- **应用侧不再需要"等待挂载落定"**：注入退化为一次跨命名空间搬运（克隆 + 附着），原子完成，不存在
  "挂载集合建立了一半"的中间态。当前 `specialize_post::wait_for_module_mount` 的整套
  轮询判据（`MOUNT_SETTLE_POLLS` + 30 轮预算）由此**整体失去存在理由**，应随本阶段删除。
  这不只是简洁性收益：多进程并发启动时，先启动进程挂好的 FUSE 根会让后启动进程在
  **重定向本体尚未建立**时就观察到"稳定的非空挂载集合"，从而提前放行并读到未重定向的
  视图（真机已复现：三个微信进程同秒启动，主进程在 `mounts=7` 时被放行）。

#### 跨命名空间搬运：只能靠 open_tree + move_mount

宿主挂载建好后，`mount_host_fuse` 会在宿主挂载点上开 shared。但**应用接入并不依赖传播**：
宿主子进程已经对整棵树做过 `MS_REC|MS_PRIVATE`，各应用 namespace 看不到宿主的挂载点，
只开 shared 也不会让它们"看见"。

搬运也不能用 `mount(MS_BIND)`：bind 的**源挂载必须属于调用方当前的 mount namespace**
（内核 `do_loopback` 的 `check_mnt`），源换成 `/proc/self/fd/<n>` 也绕不过去——真机实测直接
返回 `EINVAL`。跨命名空间搬运挂载是 `open_tree(OPEN_TREE_CLONE)` + `move_mount` 这对 API 的
用途：前者在**源命名空间**里克隆出一个不附着于任何命名空间的挂载（由 fd 携带），后者在
**目标命名空间**里把它附着到目标路径。克隆与原挂载共享同一个 superblock，因此 FUSE 请求
仍然全部回到同一个宿主会话。

克隆会**继承宿主挂载点的 shared 传播组**（`mountinfo` 里的 `shared:N`）。照原样保留的话，
各应用的这份挂载会结成同一个 peer group：任一应用在其下新建或摘除挂载都会传播到其它应用
与宿主命名空间，既跨应用干扰，又与"卸载一个应用的注入不影响其它应用"的前提冲突。因此
附着后立即把该挂载在本命名空间内改为 `MS_REC|MS_PRIVATE`，与 scoped 层的形态保持一致。

参照实现 `huniangitb/Fuse-Proxy` 的做法是 `ns_make_shared(pid, mount_path)`
（`src/injector/injector.c`），靠传播让子 namespace 得见全局挂载点；本项目**未采用该路径**，
因为它要求挂载点在宿主 namespace 里保持可得，而我们的宿主 namespace 是刻意与外界隔离的。

### 2.2 策略必须按调用方 uid 解析（硬阻塞）

**这是本改造真正的技术门槛。** 当前 `RedirectPolicy` 在会话创建时就绑定到单一
`package_name` / `uid`（`src/fuse_redirect/policy.rs:133 RedirectPolicy::new`），
FUSE 回调里 35 处 `self.policy` 全部读这一个实例，**从不读调用方 uid**。

一个共享挂载要服务所有应用，就必须把策略解析从"会话级常量"改成"每请求按 uid 查表"：

```
FUSE 回调(req)
  └─ policy = registry.for_uid(req.uid())     // fuser::Request::uid() 来自 FUSE 头
       ├─ 命中 → 该应用的 RedirectPolicy
       └─ 未命中 → 直通回退到真实后端（不做任何改写）
```

可行性已确认：`vendor/fuser-android/src/request_param.rs:26` 的 `Request::uid()` 直接返回
FUSE 输入头的 `uid` 字段，即调用方进程的 uid。

改造范围：`FuseRedirectFs.policy` 由 `RedirectPolicy` 改为
`PolicyRegistry`（`uid → Arc<RedirectPolicy>` + 配置代数），每个回调入口取一次
`Arc` 再走原有逻辑。单策略场景下行为与现在完全一致，因此可以先落地注册表、
再切换到共享宿主，分两步验证。

### 2.3 显式保存挂载身份（已落地）

`src/mount_identity.rs` 落盘每个 namespace 的挂载身份：

| 字段 | 来源 | 用途 |
|---|---|---|
| `mount_id` | `/proc/<pid>/mountinfo` 第 1 字段 | 区分同路径上新旧会话的挂载 |
| `source` | `MountOption::FSName` | 归属前缀判定（`srx_fuse_redirect` / `srx_fuse_host`） |
| `ns_dev` / `ns_ino` | `stat("/proc/<pid>/ns/mnt")` | 判定 namespace 是否被替换 |
| `target_start_time` | `/proc/<pid>/stat` 第 22 字段 | 防止 PID 复用后误认 |
| `generation` | 每次成功挂载递增 | 诊断与幂等重挂 |
| `detach_attempts` | 摘除失败累计 | poisoned 收敛依据 |

归属判定 `classify_mount` 输出四种结论：`Owned` / `Detached` / `Superseded` /
`StaleNamespace`。**先证明归属，再动手**——这是消除"按路径盲摘"和"盲目叠加"的前提。

### 2.4 死连接：先摘除，再恢复（已落地）

`src/fuse_supervisor.rs` 把恢复决策从 reconcile 里拆出来：

```
probe_endpoint(pid, target)  →  端点健康：健康 / 断连(dead_connection) / 缺失 / 未探测
        +
classify_mount(...)          →  归属判定：自有 / 已摘除 / 被接管 / 命名空间过期
        ↓
plan_recovery(verdict, health, poisoned)  →  恢复动作：先摘除再注入 / 直接注入 /
                                              跳过接管 / 清理过期 / 拒绝注入
```

关键约束：

- **摘除前校验归属**：`clear_mount_target_stack_verified` 只在最顶层挂载是本模块的、
  且账本记录的 `mount_id` 与实时一致时才摘；被其它组件或本模块新会话接管时**保留并报告**，
  绝不摘掉别人的挂载；
- **摘除未验证通过就不注入**：`handle_child_process` 在 `ClearOutcome::Unverified` 时
  累计 `detach_attempts`，达到 `MAX_DETACH_ATTEMPTS = 3` 后显式拒绝注入并记
  `reason=detach_not_verified`。这是把"无限叠加"改成"有限重试后收敛"的关键；
- **收敛后可观测**：poisoned 状态随账本落盘，reconcile 每轮输出
  `daemon reconcile skip inject pkg=... action=refuse_poisoned`，不会静默。

### 2.5 独立健康检查与服务监督（已落地基础）

- 端点健康统一由 `fuse_supervisor::probe_endpoint` 提供，替换了
  `backend_mount_targets_responsive` 里重复的 `open()` + errno 判断；
- 监督计数 `SupervisorSummary` 按恢复动作分类累计，在有活动时写入 reconcile 日志；
- `srx_daemon doctor` 子命令输出账本 + 实时探测 + 判定结论，回答
  "daemon 认为它挂了什么、那些挂载现在还活着吗"，有不健康挂载时返回非零退出码。

---

## 3. 与 Fuse-Proxy 的差异

| 维度 | Fuse-Proxy | 本项目（目标） |
|---|---|---|
| 进程划分 | `injector`（主控）+ `fuse_daemon`（FUSE 框架） | `srx_daemon` 单进程内分 `FuseHost` / `Injector` / `Supervisor` 角色 |
| 宿主挂载点 | `/mnt/nsp_global` 全局挂载 | 模块目录下私有宿主路径（不进 `/mnt`，避免被其它组件列举） |
| 规则解析 | `br_get_app_cfg(uid)` 每请求查 uid→AppConfig | 同等机制，`PolicyRegistry` 按 `req.uid()` 查表 |
| 应用发现 | zygisk 通知（首次通知后转被动模式，永久停止 `/proc` 扫描） | 保留周期 `/proc` 扫描；zygisk 已有 `specialize_pre` 通道，可后续切被动 |
| 死连接恢复 | 未在文档中展开 | 显式：身份账本 + `plan_recovery` + poisoned 收敛 |
| 模块化 | dlopen `.so` 模块 + 故障围栏 + 熔断 | 本项目策略在进程内编译，不引入模块 ABI |

**不照搬的部分**：Fuse-Proxy 的 zygisk 被动模式要求双 ABI 部署（只装 64 位会让 32 位
zygote 上的应用彻底漏注入），本项目保留周期扫描作为兜底更稳妥；其 dlopen 模块框架
对本项目没有收益，不引入。

---

## 4. 分阶段迁移计划

### 阶段 0（本次已落地）——身份账本 + 监督 + 验证式摘除

不改数据面，先建立"归属判据 + 收敛"：

- `src/mount_identity.rs`：身份账本（新增）
- `src/fuse_supervisor.rs`：端点探测 + 恢复规划 + 监督计数（新增）
- `src/daemon_mount.rs`：
  - `clear_previous_mounts` 改为返回 `ClearOutcome`，走 `clear_mount_target_stack_verified`
  - 摘除未验证 → `record_detach_failure` → poisoned 后拒绝注入
  - 挂载成功后 `record_mount_identity` 登记 `mount_id` + ns 身份
  - `supervise_mount_request` 作为 reconcile 的注入门禁
  - `doctor_report` 诊断入口
- `src/daemon.rs`：reconcile 接入监督门禁与账本清理
- `src/bin/srx_daemon.rs`：新增 `doctor` 子命令

**验证方式**：`cargo check` / `cargo build --release` 通过；设备和 CI 使用当前完整场景 1-37
（重点关注重定向读写、死挂载恢复、MediaStore 代写、应用重启、共享宿主和回退路径），
并用 `srx_daemon doctor` 观察账本与判定。

### 阶段 1——策略注册表与按 uid 注册（已落地）

- `FuseRedirectFs.policy` 改为 `PolicyRegistry`，每个回调按 `req.uid()` 取策略。
- `SharedPolicyTable`：宿主会话持有可写的按 uid 策略表句柄，daemon 在挂载前把该应用的策略
  （`fuse_config_from_request(request, None, …)`，虚拟根取整个存储根）经**控制通道**送进会话，
  在会话内用同一份 `RedirectPolicy::new` 构造并登记（`fuse host policy registered uid=…
  table=N`）。控制通道是 `socketpair`，不是命名套接字，外部无法注入策略。
- **未登记 uid 一律拒绝**：宿主会话的回退策略是"一律拒绝"（`deny_all`），不是直通。宿主会话
  服务的是完整存储视图，若让未登记调用方回退到直通策略，它会直接读写真实存储（沙盒失效），
  而这种越权落点**无法靠事后清理恢复**，只能在决策前挡住。scoped 会话不受影响：回退仍是会话
  策略，与改造前一致。
- 拒绝在 `backend_for_relative` 的最前面生效，所有调用点（lookup/getattr/create/write/readdir…）
  都把 `None` 当失败处理，因此未登记调用方既读不到内容也写不进任何位置（表现为 `ENOENT`）。
- 真机 A/B 取证（Android 16）：进入宿主 namespace 后，已登记 uid `10284` 能列出真实存储根；
  未登记 `uid 0` 读/写均为 `ENOENT`，真实存储未产生任何文件。
- `session()`（会话绑定策略）只用于日志与 `statfs`，不参与路径决策，因此不构成内容泄漏面。

### 阶段 2——共享宿主会话 + namespace 注入（会话建立已真机验证；接入实现已落地，默认关闭）

- daemon 启动时建立 `FuseHost` 共享宿主会话，挂载源为 `srx_fuse_host[<pid>]`；子进程进入私有
  namespace，挂载点为模块私有目录，并在挂载点上开 shared。**该会话已在 Android 16 真机验证
  可以真实建立**（`fuse host session mount registered` + `srx_fuse_host` 进程常驻）。
- 直通会话必须显式标记为 `is_passthrough_host`：它的 `redirect_target` 就是存储根本身，而按子
  路径推导重定向根的函数对"根本身"返回 `None`，否则策略构造会直接失败（真机表现为宿主子进程
  在 `policy` 阶段退出，且旧代码在该分支没有任何日志）。
- **应用接入的实现已落地并真机 A/B 验证**（`fuse_host::attach_app_to_host`）。daemon 与
  companion 两条挂载路径共用同一份实现——这条链路上"挂载落在哪个命名空间、怎么搬过去"是
  唯一的要害，两处各写一份最容易只改对一处，而症状（应用读到真实存储）不会报错。做法：
  在宿主 namespace 内 `open_tree(OPEN_TREE_CLONE)` 克隆游离挂载 → 切回**应用自己的
  namespace** → `move_mount` 附着 → 立刻 `MS_REC|MS_PRIVATE` 切断传播 → 复核应用视图里该
  目标最上层的挂载源等于本次会话的挂载源。子进程附着成功即退出：挂载归 mount namespace
  所有，常驻只会白占一个进程并拖住应用 namespace 的引用计数。
  （`mount(MS_BIND)` 做不到跨命名空间搬运，真机直接 `EINVAL`；克隆继承的 shared 传播组
  必须显式切断，理由见 §2.1。）
- **登记进宿主会话的策略必须丢弃 `real_root_override`**：那是给应用命名空间内的 scoped
  会话用的锚点别名（真实存储根先绑到 `tmp/real_storage/<user>`，绑定只存在于应用
  namespace），宿主子进程在自己的私有命名空间里看到的是一个**空目录**。照搬覆盖会让策略
  把真实根读成空——真机表现为仅映射模式的应用整个丢失公共存储视图，视图根只剩被沙盒化的
  那几条路径。宿主命名空间里 `/data/media/<user>` 本来就是未经覆盖的真实存储，无需别名。
- **挂载台账把宿主会话与 scoped 子进程分开记**：接入产生的挂载写 `fuse_host=<pid>:<start>`
  而不是 `fuse_child=`，回滚与清理只卸载、不终止任何进程——宿主会话跨应用共享，按它的 pid
  发信号等于把所有接入应用一起打成死挂载。会话死亡时 `has_dead_fuse_child` 据此把该应用的
  挂载状态判为失效并触发重挂；已被重建会话留下的层也不再按"可保留"处理（否则保留的是一条
  已经断开的 FUSE 连接）。
- **策略登记改为同步应答**：宿主子进程登记后回 `(uid, 表大小)`，调用方按 uid 匹配等待应答
  （同一条控制通道被所有挂载 worker 共用，所以应答必须带 uid）；登记未确认时保持 scoped 路径，
  避免"策略还没进会话就接入"导致整片 `ENOENT`。
- **接入只允许落在存储视图根**：宿主会话按 uid 注册策略，虚拟根只能是整个存储视图根。落在更深的
  scoped 子根时，内核会把该子树下的请求按整根解析，命中的是与本应用规则无关的真实路径——读错
  内容却不报错。因此闸门只在 `mount_root` 就是该 uid 的存储视图根时放行。
- **接入默认关闭**，验证期由环境变量 `SRT_FUSE_HOST_ATTACH`（`1`/`true`/`yes`）显式打开。
  接入把数据面从"每应用一个 scoped 会话"换成"全局一个宿主会话"，因此正式启用前必须换成
  **应用进程可读的开关**（配置项或模块文件）：`companion_mount` 跑在应用进程内，读不到
  daemon 的环境变量——真机实测同一应用在 daemon 侧接入已打开时，仍是应用自己启动时用
  scoped 会话挂好的。关闭时行为与改造前完全一致，日志里可以看到 `reason=attach_gate_closed`。
- **真机 A/B 结论**（Android 16，同一应用同一配置）：scoped 会话与宿主会话下，应用视图根
  内容逐项一致（真实公共目录 + 被沙盒化的路径），应用在视图里的写入落在自己的沙盒、公共
  存储无残留；`kill` 掉宿主子进程后状态判活生效，应用自动重挂到新会话。
- 宿主会话失效后由 daemon 在 reconcile 中重建，失败时保留 scoped FUSE 回退；`srx_fuse_redirect`
  前缀路径不能删除。
- **僵尸态必须排除**：会话线程结束时子进程自行退出（FUSE 连接被 `FUSE_DESTROY` 结束），父进程
  若不回收，`/proc/<pid>` 仍然存在，只按 pid 判活会把它当成存活会话，自愈永远不会重建。因此
  存活判定同时检查 `/proc/<pid>/stat` 的状态字符并在判定死亡时回收；真机 `kill -9` 宿主子进程
  后 1 秒内完成重建（`fuse host session exited on its own` → `fuse host recovered`）。

### 宿主会话可观测性（已落地）

宿主子进程的失败原因此前完全不可见：它在私有日志通道可用之前就退出，而 `service.sh` 把 daemon
的 stderr 丢到 `/dev/null`，panic 信息也不会留下。现在有三层取证：

- 阶段文件 `tmp/fuse_host.stage`：子进程只用一个栈缓冲加系统调用逐步追加，父进程在失败时把它
  转写到 `running.log`（`fuse host stage trace ...`）；
- ready 通道传**阶段码**而不是布尔值：`dir` / `policy` / `mount` / `stability` / `shared`；
- 父进程回收子进程并记录退出原因（`exit=` / `signal=`），同时区分"对端已关闭"和"等待超时"。
- 真机排查时注意 `pidof srx_fuse_host` 不可用（该子进程没有独立 cmdline），应按
  `/proc/*/comm` 匹配；进入其 namespace 用 `busybox nsenter -t <pid> -m`（toybox nsenter 会去
  读不存在的 `ns/user` 而失败）。

### 阶段 3——监督收敛（基础能力已落地，后续增强）

- 身份账本、端点探测、验证式摘除、poisoned 收敛和 `srx_daemon doctor` 已落地。
- 共享宿主死亡后的全量注入重建、按 namespace 独立预算和指数退避仍属于后续增强方向。
- `service.sh` 的 respawn 兜底仍需在真实设备上单独验证后再扩大范围。

### 阶段 4——发现通道优化（可选，未启用）

- 可参考 Fuse-Proxy 的 zygisk 通知转被动模式，降低 `/proc` 扫描开销。
- 在双 ABI 部署完整性和兼容性得到验证前，继续保留周期扫描兜底。

---

## 5. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| 共享会话退出影响面从单应用扩到全部应用 | 高 | 接入默认关闭（`SRT_FUSE_HOST_ATTACH` 显式打开）；宿主会话由 `srx_fuse_host` 自愈重建，会话死亡让状态判为失效并重挂；任一接入失败立即回落 scoped fork |
| uid 感知改造触碰 35 处策略读取点 | 高 | 阶段 1 单独提交，行为不变，用场景全绿验证 |
| 按 uid 直通回退可能漏改写 | 高 | 宿主会话未命中 uid 一律**拒绝**（`deny_all` → `ENOENT`），绝不回退直通；落点越权无法靠事后清理恢复，必须在决策前挡住 |
| 宿主 pid 被当成 scoped 子进程记账 | 高 | 状态文件按 `host_session` 分流：宿主挂载写 `fuse_host=` 只判活、不终止；回滚对宿主挂载只卸载。守卫测试锁定"不得写进 `fuse_child=`" |
| 策略未进宿主会话就接入 | 高 | 登记改为同步应答（`(uid, 表大小)`），未确认即保持 scoped 路径，不进入接入分支 |
| 已重建会话留下的层被当成有效层保留 | 中 | 保留判据拒绝 `is_stale_host_source` 的层，按摘除重建处理；否则保留的是一条已断开的 FUSE 连接（`ENOTCONN`） |
| 接入落在 scoped 子根导致按整根解析 | 中 | 闸门只在 `mount_root` 就是该 uid 的存储视图根时放行；其余情况保持 scoped |
| 跨命名空间搬运挂载写成了 `mount(MS_BIND)` | 高 | bind 的源挂载必须属于调用方当前命名空间，跨命名空间时内核直接 `EINVAL`；接入改用 `open_tree(OPEN_TREE_CLONE)` + `move_mount`，并有守卫测试钉住顺序（见 §2.1） |
| 附着进来的克隆继承了宿主挂载点的 shared 传播组 | 高 | 各应用的挂载会结成同一 peer group，互相传播挂载/卸载；附着后立即在本命名空间内改为 `MS_REC\|MS_PRIVATE` |
| 宿主会话策略照搬应用命名空间的锚点覆盖 | 高 | 锚点绑定只存在于应用命名空间，宿主子进程看到空目录 → 仅映射模式应用丢失公共存储视图；登记时传 `real_root_override=None`，用宿主命名空间里的 `/data/media/<user>` |
| 启用开关写在 daemon 的环境变量里 | 中 | companion 路径在应用进程内，读不到该变量，接入只在 daemon 侧重挂时生效；正式启用前换成应用进程可读的开关 |
| SELinux 对宿主挂载路径的标签限制 | 中 | 宿主路径放在模块目录下，沿用现有 `fs::create_directory` 的属主/模式处理 |
| poisoned 后应用失去重定向（回退到真实存储） | 中 | 这是刻意选择的"可用性优先、拒绝叠加"策略；`doctor` 与日志显式暴露，等待摘除成功或重启 |

---

## 6. 与现有约束的兼容性

- **普通应用不安装 native/inline/PLT hook**：本改造全部走 companion mount / mount
  namespace / FUSE 路径，未新增任何进程内 hook；
- **测试流同时识别两类 FUSE 挂载**：`srx_fuse_redirect` 表示 scoped 回退会话，`srx_fuse_host` 表示共享宿主会话；场景断言不能只匹配其中一种。
- **能力熔断快照语义不变**：`fuse_capability` 的 `state`/`reason`/`fail_count` 未改动。
- **后端选择仍由 `auto` 统一控制**：文档、测试和诊断不应把 FUSE 或 namespace 单独描述成所有设备的固定后端。
