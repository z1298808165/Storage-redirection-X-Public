# 共享 FUSE daemon 架构设计

本文给出把"每个应用各自起一个 FUSE 服务"改造成"一个持久共享 FUSE daemon + 独立监督与
namespace 注入"的目标架构、根因分析、分阶段迁移计划，以及本次已落地的部分。

参考实现：`huniangitb/Fuse-Proxy`（Android 13+ 用户态 FUSE 代理 + 挂载命名空间隔离）。
该项目的 `injector` / `fuse_daemon` 分离结构是本方案的直接参照，差异见 §3。

---

## 1. 现状与失效窗口

### 1.1 当前结构

```
zygisk specialize_pre ──connect_companion──> root companion ──> companion_mount.rs（按应用挂载）
srx_daemon（持久）      ──reconcile 每 3s──> daemon_mount.rs
                                              └─ fork 子进程 setns → spawn_mount2（每个挂载根一个 srx_fuse）
```

- FUSE 会话创建：`src/fuse_redirect/config.rs:410 mount_blocking_with_ready` →
  `fuser::spawn_mount2`，`MountOption::FSName("srx_fuse_redirect[<pid>]")`。
- 服务进程：`src/daemon_mount.rs:1096 start_fuse_service_for_root` 为**每个挂载根** fork 一个
  子进程（`prctl(PR_SET_NAME,"srx_fuse")`）。N 个应用 × M 个挂载根 = N×M 个独立 FUSE 会话。
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
- **服务退出是单一事件**：不再有 N 个各自独立的失败窗口，而是一个被监督的会话。

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

**验证方式**：`cargo check` / `cargo build --release` 通过；设备上跑现有场景 1-29
（重点场景：重定向读写、死挂载恢复、MediaStore 物理回退、应用重启后重挂），
并用 `srx_daemon doctor` 观察账本与判定。

### 阶段 1——策略注册表（不改数据面行为）

- `FuseRedirectFs.policy` → `PolicyRegistry`，每个回调按 `req.uid()` 取策略
- 单策略场景行为不变，场景 1-29 应全绿，作为纯重构验证
- 未命中 uid 时直通回退，为阶段 2 的多应用共享做准备

### 阶段 2——共享宿主会话 + namespace 注入

- daemon 启动时创建私有 namespace，`unshare(CLONE_NEWNS)` + `MS_REC|MS_PRIVATE`
- 在私有 namespace 内把 FUSE 挂到宿主路径，`source = srx_fuse_host[<pid>]`
- 应用挂载改为 `setns(目标 ns)` → `MS_BIND` 宿主树到 `/storage/emulated/N`
- `srx_fuse_redirect` 与 `srx_fuse_host` 前缀共存期，账本按前缀判定归属
- 保留 `srx_fuse_redirect` 路径作为回退，能力熔断机制不变

### 阶段 3——监督收敛

- 宿主会话纳入监督：会话死亡 → 摘除全部注入 → 重建会话 → 重新注入
- 收敛条件从"3 次摘除失败"细化为按命名空间独立预算 + 指数退避
- `service.sh` 增加轻量 respawn 兜底（daemon 自身退出后仍能被拉回）

### 阶段 4——发现通道优化（可选）

- 参考 Fuse-Proxy 的 zygisk 通知转被动模式，降低 `/proc` 扫描开销
- 需先确认双 ABI 部署完整性，否则保留扫描兜底

---

## 5. 风险与对策

| 风险 | 影响 | 对策 |
|---|---|---|
| 共享会话退出影响面从单应用扩到全部应用 | 高 | 阶段 2 保留 `srx_fuse_redirect` 回退路径；阶段 3 先做监督再切默认 |
| uid 感知改造触碰 35 处策略读取点 | 高 | 阶段 1 单独提交，行为不变，用场景全绿验证 |
| 按 uid 直通回退可能漏改写 | 中 | 未命中 uid 一律回退真实后端（不改写），不猜测策略 |
| bind mount 的 mount propagation 影响其它 namespace | 中 | 宿主 namespace 用 `MS_REC\|MS_PRIVATE`；注入时逐个 `setns`，不改全局传播 |
| SELinux 对宿主挂载路径的标签限制 | 中 | 宿主路径放在模块目录下，沿用现有 `fs::create_directory` 的属主/模式处理 |
| poisoned 后应用失去重定向（回退到真实存储） | 中 | 这是刻意选择的"可用性优先、拒绝叠加"策略；`doctor` 与日志显式暴露，等待摘除成功或重启 |

---

## 6. 与现有约束的兼容性

- **普通应用不安装 native/inline/PLT hook**：本改造全部走 companion mount / mount
  namespace / FUSE 路径，未新增任何进程内 hook；
- **测试流识别 scoped 挂载按 `srx_fuse_redirect` 前缀**：阶段 0 未改动该前缀，
  场景断言不受影响；阶段 2 引入 `srx_fuse_host` 时需同步测试流的前缀识别；
- **能力熔断快照语义不变**：`fuse_capability` 的 `state`/`reason`/`fail_count` 未改动。
