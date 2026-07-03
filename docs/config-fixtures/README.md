# 配置 Schema 共享样例

这些 JSON 文件用于锁定 Rust 核心、管理 App 和 WebUI 对配置格式的共同理解。

- `global-defaults.json`：全局配置的完整默认字段。
- `app-profile-full.json`：应用配置的常用字段组合，包含 `!` 排除规则、局部沙盒路径、只读路径和对象形式路径映射。
- `app-profile-normalization-input.json`：管理端接受的用户输入样例，包含绝对路径、重复路径、非法路径、旧版排除字段、只读路径和循环映射。
- `app-profile-normalization-output.json`：上述输入归一化后的落盘格式，用于锁定各端最终配置语义。
- `backup-v2-minimal.json`：备份格式 v2 的最小可还原 `backup.json` payload。当前导出的外层文件是 `.srxbak.zip`，ZIP 内固定包含该 JSON 文件；旧 `.srxbak.json` 仍可导入。

新增配置字段时，应同步更新这里的样例，并让各端解析/归一化测试复用这些文件。

WebUI 可通过以下命令验证已归一化落盘样例是否仍被导入 normalizer 接受：

```powershell
node scripts/verify-webui-config-fixtures.js
```
