- 将模块运行日志切换为私有 `srx_logd` 写入，显著降低启用模块后的 `logd` 负载。
- 精简系统代写和 MediaStore 相关的高频日志，只保留摘要与真正的慢路径信息，减少运行期噪声。
- 优化日志查看与清空流程，状态日志会及时刷新，清空操作也改为由模块日志守护进程统一处理。

<details>
<summary>English</summary>

- Route module runtime logs through the private `srx_logd` writer to significantly reduce `logd` load after enabling the module.
- Trim high-frequency system-writer and MediaStore logs down to summaries and real slow paths to reduce runtime noise.
- Improve log viewing and clearing so status logs flush in time and log clearing is handled by the log daemon itself.
</details>