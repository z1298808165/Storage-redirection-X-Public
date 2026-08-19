# FUSE-first 迁移架构

本项目把普通应用的数据面收敛为自动后端：配置面固定写入 `auto`，运行时在 scoped FUSE 与 mount namespace 之间选择。

## 运行方式

- `auto`：设备存在可用 `/dev/fuse` 且规则需要动态匹配时使用 scoped FUSE；其它路径继续使用 mount namespace，FUSE 服务异常时自动回退。

配置位于 `config/global.json`：

```json
{
  "storage_backend_mode": "auto"
}
```

旧版 `fuse_daemon_redirect_enabled` 不再读取；新建、迁移和管理端保存的配置只写入 `storage_backend_mode=auto`。

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

## 迁移原则

`auto` 是所有设备的唯一配置模式；测试流会记录每个场景的 `backend_effective`，并保留 FUSE cache 容量、mount intent 和原始运行日志。系统 MediaProvider/系统 writer 仍使用现有调用方识别 hook，普通应用不安装进程内 PLT hook。
