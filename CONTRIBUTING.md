# 贡献指南

感谢你对 Storage Redirect X 的贡献！请在提交 PR 前阅读以下规范。

## 提交与推送强制规范

- Commit 必须按修改目的和类型拆分，问题修复、功能新增、重构、性能、测试、文档、构建和 CI 改动原则上分别提交。每个 Commit 只承担一个主要职责，并保持可独立审查。
- 不得新增 Rust 内联测试。只保留原仓库已有的 6 个 Rust 内联测试；新增验证请使用外置测试、测试流 APP 或 `.github/tests/` 场景。
- 每个包含文件改动的 Commit 必须由 AI Agent 审核精确暂存差异，重点排除无用代码、测试残留、重复实现、过度抽象、占位逻辑、错误路径和兼容性风险，并运行 `scripts/record-ai-review.ps1` 登记审核凭据。完整流程见 [AI Agent 代码质量审核](docs/ai-agent-code-review.md)。
- 默认提交标题使用 `类型(可选范围)：中文描述`，可以保留必要的英文技术名词、包名、命令和文件名。
- 项目自维护的文档、代码注释、PR/Issue、发布说明、界面文案和诊断文本默认使用中文。API、类名、命令、路径、协议字段、产品名、机器解析日志键、固定错误文本和测试标识等必要技术内容可以保留英文；第三方源码、许可证、SPDX 标识和上游补丁原文保持原样。确需保留其它英文自然语言时，必须紧邻写明 `quality-allow(chinese-language): <中文具体原因>`。
- 推送前必须运行 `git status --short --branch`，确认当前分支、工作区状态、待推送提交和远端跟踪关系。
- 推送前必须运行必要验证；本仓库主验证命令是 `cargo build --target aarch64-linux-android --release`。无法运行时，必须在最终说明中写明原因。
- 推送前必须检查 `.github/workflows/*.yml`，确认本次推送触发的是 CI Build 还是 Release workflow。
- 推送前必须运行 `git log --oneline origin/main..HEAD`，确认待推送提交标题符合中文提交规范。
- 不要重写已经推送到远端的历史，除非用户明确批准 force push。
- 不要回滚他人或用户已有改动；遇到不相关的未提交改动时，只处理本次任务涉及的文件。

## 版本与发布要求

- Release workflow 只由指向 `SRX-R` 分支可达提交的 `v*` tag 推送触发，普通 `SRX-R` 分支 push 不会发布正式版本。
- 发布版本 `X.Y.Z` 时，`Cargo.toml` 中 `[package].version` 必须为 `X.Y.Z`，发布 tag 必须是 `vX.Y.Z`。
- 发布 tag 必须指向一个不包含 `[skip ci]`、`[ci skip]`、`[no ci]` 的提交。
- 如果版本变更提交需要跳过普通 CI，可以追加空提交 `Releases: 发布 X.Y.Z`，再把 tag 打在该提交上。
- 正式 Release 的 GitHub Release 正文必须使用中文更新日志，范围为上一版 Release 到本次 `vX.Y.Z` tag。
- CI 构建必须生成中文更新日志；刷入包和正式发布 zip 内不得打包 `CHANGELOG.md`。

## PR 标签规范

提交 PR 时**必须**在 PR 描述中声明标签类型，用于决定 CI/CD 行为：

| 标签 | 说明 | CI 行为 |
|------|------|---------|
| `CI` | 仅验证构建 | 触发自动构建，验证代码是否能正常编译，构建产物作为 Artifact 上传 |
| `releases` | 正式发布版本 | 构建通过后，自动创建 GitHub Release，生成变更日志，上传模块 ZIP 供用户下载 |

## 工作流说明

## 上游 Hook 依赖判断

本项目通过 Cargo git 依赖引用 `srx_hook` 和 `srx_inline_hook`，源码不直接复制到本仓库。`Cargo.lock` 会锁定实际使用的上游 commit。

任何涉及 native hook、PLT hook、inline hook、trampoline、linker 符号解析、dlopen/dlclose 回调、fork child 防护、原函数回调或 Android 新版本 hook 适配的变更，都必须判断是否需要上游库更新：

- 如果问题属于本仓库业务逻辑，PR 描述中说明不需要上游更新。
- 如果需要上游已有修复，PR 应说明更新了哪个依赖和 commit。
- 如果需要上游新增能力或修复 bug，PR 应明确指出需要更新 `srx_hook` 或 `srx_inline_hook`，并描述所需能力或复现条件。

更多边界说明见 [上游 Hook 依赖说明](docs/upstream-hook-dependencies.md)。

### CI 构建（每次 PR / push 到 SRX-R）

- 自动触发，无需额外操作
- 验证代码能否在 `aarch64-linux-android` 目标上成功编译
- 构建产物（Zygisk 模块 ZIP）上传为 GitHub Actions Artifact

### 正式发布（推送 tag 时触发）

当需要发布新版本时：

1. 更新 `Cargo.toml` 中的 `version` 字段
2. 合并 PR 到 main
3. 在 `SRX-R` 分支可达提交上打 tag：
   ```bash
   git tag v1.2.40
   git push origin v1.2.40
   ```
4. Release 工作流自动触发：
   - 构建模块
   - 生成距上一个 Release 的变更日志；其中“修复了什么问题”“增加了什么功能”“新功能怎么使用”“注意事项”按需结合上一正式版到当前 tag 的代码差异，以及这段期间的 CI 更新日志补充；只有实质性用户可见新功能（例如新增设置项、配置项或操作入口）才写“增加了什么功能”和“新功能怎么使用”，没有对应内容时可以省略相应标题，不要硬凑空内容。
   - 创建 GitHub Release 并上传模块 ZIP

## 分支命名规范

- `fix/xxx` — Bug 修复
- `feat/xxx` — 新功能
- `refactor/xxx` — 重构
- `docs/xxx` — 文档更新

## 提交信息规范

提交标题不超过 72 个字符，允许的类型包括 `功能`、`修复`、`重构`、`性能`、`测试`、`文档`、`构建`、`CI`、`依赖`、`界面`、`维护`、`发布` 和 `回退`：

```
修复(重定向)：避免共享 UID 配置失效
功能(管理器)：增加配置导入入口
重构(监视)：拆分路径归因逻辑
文档：更新本地构建说明
```

本地 hook 会在提交和推送时检查标题格式、明显的职责混合、Rust 内联测试白名单、增量代码残留及与 Commit tree 绑定的 AI 审核凭据。请启用 `git config core.hooksPath .githooks`，不要使用 `--no-verify` 绕过检查。
