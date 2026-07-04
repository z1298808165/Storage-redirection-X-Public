- 将模块运行日志切换为私有 `srx_logd` 写入，显著降低启用模块后的 `logd` 负载。
- 精简系统代写和 MediaStore 相关的高频日志，只保留摘要与真正的慢路径信息，减少运行期噪声。
- 优化日志查看与清空流程，状态日志会及时刷新，清空操作也改为由模块日志守护进程统一处理。
- 将路由器重定向命中日志改为采样输出，避免高并发下刷满运行日志。
- 挂载计数改为统一由日志守护进程累加，修复挂载计数可能被守护进程旧值覆盖而丢失的问题。

- 修复 KernelSU 设备上 `srx_logd` 私有日志 socket 的 SELinux 规则，避免 WebView 沙箱进程启动失败并引发 Via 白屏。

<details>
<summary>English</summary>

- Fix the SELinux policy for the private `srx_logd` socket on KernelSU devices, preventing WebView sandbox startup failures that could lead to a blank Via screen.
- Route module runtime logs through the private `srx_logd` writer to significantly reduce `logd` load after enabling the module.
- Trim high-frequency system-writer and MediaStore logs down to summaries and real slow paths to reduce runtime noise.
- Improve log viewing and clearing so status logs flush in time and log clearing is handled by the log daemon itself.
- Sample the router redirect-hit logs to avoid flooding the runtime log under high concurrency.
- Route mount counting through the log daemon so it is the sole writer, fixing lost mount counts when the daemon overwrote fresh values with its stale in-memory total.
</details>
