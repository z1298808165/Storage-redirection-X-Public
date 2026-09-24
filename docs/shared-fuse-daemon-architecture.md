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
                                              └─ 共享宿主会话：FuseHost 建立 srx_fuse_host；应用侧 setns + MS_BIND 注入，失败时回退 scoped srx_fuse_redirect
```

- 共享宿主会话：`src/fuse_host.rs` 在 daemon 启动时建立私有 mount namespace 和宿主 FUSE 会话，挂载源为 `srx_fuse_host[<pid>]`。
- 应用接入：`src/daemon_mount.rs` 与 `src/lifecycle/companion_mount.rs` 优先通过 `setns` + `MS_BIND` 注入共享宿主；宿主接入失败时回退到原有 scoped FUSE，会话源为 `srx_fuse_redirect[<pid>]`。
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
- **应用侧不再需要"等待挂载落定"**：注入退化为一次 `MS_BIND`，原子完成，不存在
  "挂载集合建立了一半"的中间态。当前 `specialize_post::wait_for_module_mount` 的整套
  轮询判据（`MOUNT_SETTLE_POLLS` + 30 轮预算）由此**整体失去存在理由**，应随本阶段删除。
  这不只是简洁性收益：多进程并发启动时，先启动进程挂好的 FUSE 根会让后启动进程在
  **重定向本体尚未建立**时就观察到"稳定的非空挂载集合"，从而提前放行并读到未重定向的
  视图（真机已复现：三个微信进程同秒启动，主进程在 `mounts=7` 时被放行）。

#### 传播方向必须先开 shared，否则注入静默失败

宿主挂载建好后，**必须把它在宿主 namespace 里显式设为 shared propagation**，各个应用
namespace 的 `MS_BIND` 才能引用到它。只做 `MS_REC|MS_PRIVATE` 隔离宿主 namespace 是**不够**的
——private 只切断向外传播，不会让子 namespace 得见该挂载；缺了 shared，注入会以 ENOENT 或
挂到空目录的形式静默失败，且因为 bind 本身"成功"而不会产生错误日志。

参照实现 `huniangitb/Fuse-Proxy` 的对应动作是 `ns_make_shared(pid, mount_path)`
（`src/injector/injector.c`），它在挂载 FUSE 之后、验证就绪之前执行，用于把
`/mnt/nsp_global` 变成可被各应用 namespace bind 的共享挂载点。本项目阶段 2 实现时
须在 `FuseHost` 建立后补上等价调用。

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

### 阶段 1——策略注册表（已落地）

- `FuseRedirectFs.policy` 已改为 `PolicyRegistry`，每个回调按 `req.uid()` 取策略。
- 单策略行为保持兼容；未命中 uid 时直通回退，为共享会话提供多应用策略解析能力。

### 阶段 2——共享宿主会话 + namespace 注入（已落地，保留回退）

- daemon 启动时创建私有 namespace，并建立 `FuseHost` 共享宿主会话，挂载源为 `srx_fuse_host[<pid>]`。
- daemon 和 companion 优先通过 `setns(目标 ns)` → `MS_BIND` 将宿主树注入应用 namespace。
- `srx_fuse_redirect` 与 `srx_fuse_host` 前缀共存，账本和测试流按两种前缀识别归属。
- 宿主接入失败或能力不可用时保留 scoped FUSE 回退，不能删除旧路径。

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
| 共享会话退出影响面从单应用扩到全部应用 | 高 | 阶段 2 保留 `srx_fuse_redirect` 回退路径；阶段 3 先做监督再切默认 |
| uid 感知改造触碰 35 处策略读取点 | 高 | 阶段 1 单独提交，行为不变，用场景全绿验证 |
| 按 uid 直通回退可能漏改写 | 中 | 未命中 uid 一律回退真实后端（不改写），不猜测策略 |
| bind mount 的 mount propagation 影响其它 namespace | 中 | 宿主 namespace 用 `MS_REC\|MS_PRIVATE` 隔离；**同时在宿主挂载点上开 shared**，否则应用 namespace 的 bind 拿不到共享会话（见 §2.1） |
| SELinux 对宿主挂载路径的标签限制 | 中 | 宿主路径放在模块目录下，沿用现有 `fs::create_directory` 的属主/模式处理 |
| poisoned 后应用失去重定向（回退到真实存储） | 中 | 这是刻意选择的"可用性优先、拒绝叠加"策略；`doctor` 与日志显式暴露，等待摘除成功或重启 |

---

## 6. 与现有约束的兼容性

- **普通应用不安装 native/inline/PLT hook**：本改造全部走 companion mount / mount
  namespace / FUSE 路径，未新增任何进程内 hook；
- **测试流同时识别两类 FUSE 挂载**：`srx_fuse_redirect` 表示 scoped 回退会话，`srx_fuse_host` 表示共享宿主会话；场景断言不能只匹配其中一种。
- **能力熔断快照语义不变**：`fuse_capability` 的 `state`/`reason`/`fail_count` 未改动。
- **后端选择仍由 `auto` 统一控制**：文档、测试和诊断不应把 FUSE 或 namespace 单独描述成所有设备的固定后端。
