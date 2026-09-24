# 自动后端与 FUSE 挂载架构

本文记录从早期“FUSE-first”设计到当前自动后端实现的演进。当前项目并不把 FUSE 视为所有设备上的唯一优先后端：配置面固定使用 `auto`，运行时根据平台能力、规则需求和挂载状态，在 scoped FUSE、共享 FUSE 宿主会话与 mount namespace 之间选择，并保留回退路径。

## 运行方式

- `auto`：设备存在可用 `/dev/fuse` 且规则需要动态匹配时使用 scoped FUSE；其它路径继续使用 mount namespace，FUSE 服务异常时自动回退。

配置位于 `config/global.json`：

```json
{
  "storage_backend_mode": "auto"
}
```

旧版 `fuse_daemon_redirect_enabled` 不再读取；新建、迁移和管理端保存的配置只写入 `storage_backend_mode=auto`。

## 会话归属与能力熔断

- 每个 scoped 会话使用唯一的挂载源（`MountOption::FSName`，形如 `srx_fuse_redirect[<pid>]`），挂载完成后记录该挂载的挂载 ID。收尾时先确认挂载点最顶层仍是本次会话的挂载源；同路径已被新会话替换或已被摘除时跳过卸载，不会摘掉接管者的挂载。
- 挂载归属按挂载源前缀判断，不按文件系统类型判断：内核 `mount(2)` 直挂的 scoped 挂载类型是 `fuse`，只有经 fusermount 回退时才出现 `fuse.srx`。
- scoped 挂载启动或会话收尾连续失败达到预算（连续 3 次）才把整机能力写成 `unavailable`，本轮开机内随后回退 mount namespace；任意一次成功都会清零，daemon 启动会重置。能力快照包含 `state`、`reason` 与 `fail_count`（`schema=2`），管理端、WebUI 与诊断归档仍只读取 `state` 与 `reason`。

## 优点

- FUSE 请求在文件操作开始时执行策略，通配、动态文件和只读排除不依赖挂载时的目录快照。
- 挂载请求会先写入 `tmp/mount_intent/*.intent`，再更新为 `applying`、`mounted` 或 `failed`；正式 mount state 仍只保存可清理的已生效目标。
- 路径映射、只读和沙盒规则共用一个数据面，减少 namespace 多层 bind mount 的组合复杂度。
- Android 13 及以上通过能力探测选择后端；未知厂商实现自动回到 namespace。
- mount namespace 仍作为自动回退路径，便于跨厂商设备保持兼容。

## 缺点

- FUSE 引入用户态往返，随机读写和高频小文件操作可能增加延迟；启用 passthrough 只能优化已允许文件的数据面，不能消除策略判断开销。
- FUSE 服务退出会影响对应挂载根，必须依赖健康检查、熔断和 namespace 回退。
- `fuse` 模式接管范围更大，厂商 MediaProvider、内核 FUSE 和 SELinux 差异需要真机验证。
- FUSE 运行时属于共享链路，策略或资源泄漏的影响面大于单个应用的 bind mount。

## 当前状态与维护边界

`auto` 是所有设备的唯一配置模式；测试流会记录每个场景的 `backend_effective`，并保留 FUSE cache 容量、mount intent 和原始运行日志。系统 MediaProvider/系统 writer 仍使用现有调用方识别 hook，普通应用不安装进程内 PLT hook。

当前实现的关键边界如下：

- 有能力且规则需要时使用 scoped FUSE；共享宿主会话可通过 `srx_fuse_host[<pid>]` 为多个应用 namespace 提供共享数据面。
- `mount namespace` 不是待删除的旧实现，而是 `auto` 后端的正式路径和 FUSE 不可用时的兼容回退。
- `srx_fuse_redirect` 仍是 scoped FUSE 的兼容前缀；测试、账本和诊断必须同时识别 `srx_fuse_redirect` 与 `srx_fuse_host`。
- 只有在新的共享宿主路径、真实设备兼容性和回退行为持续通过验证后，才可以进一步收窄旧 scoped FUSE 路径；当前不应删除该回退。
