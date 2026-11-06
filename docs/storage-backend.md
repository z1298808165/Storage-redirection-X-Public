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
- **接入超时**：宿主策略登记或 namespace 接入没在预算内完成，应用会带 `attach_gate_closed` 回落。

### 判断「接管是否成功」不要看挂载源名

挂载源已原生化（`/dev/fuse`），`srx_fuse_host[...]` 这类源名判据在共享宿主布局下恒落空。
用拓扑事实判断，两种布局都覆盖：

- **bind 布局**（回落 namespace）：规则落点本身是该进程的挂载点。
- **共享宿主布局**：应用视图根 `/storage/emulated` 的超级块 dev 与 init 命名空间里系统 FUSE 的 dev
  不同，即视图已由模块会话服务；规则落点不会在 `mountinfo` 里出现独立挂载层，这是设计目标。

### 映射类问题的两个前提

- **映射目标目录必须在接入前存在于真实存储**：共享宿主布局没有「建立映射挂载」这一步，
  daemon 与 companion 两条接入路径都会在接入前预建目标目录。若目标目录缺失，
  系统代写链路（如下载提供器）会判定父目录不存在而直接放弃。
- **映射按链式语义解析**：父映射的结果若又命中另一条映射的请求路径，会继续解析到不动点；
  自嵌套映射（目标落在自己请求路径的子树内）只生效一次。排查时按「最终落点」核对，
  而不是按第一跳目标。
