# 贡献指南

感谢你对 Storage Redirect X 的贡献！请在提交 PR 前阅读以下规范。

## 提交与推送强制规范

- 默认提交信息必须使用中文，可以保留必要的英文技术名词、包名、命令、文件名和固定前缀。
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

### Release 发布（打 tag 时触发）

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

使用 [Conventional Commits](https://www.conventionalcommits.org/) 格式：

```
fix: 修复某个问题的简短描述
feat: 添加某个功能的简短描述
refactor: 重构某部分代码
docs: 更新文档
```

这有助于自动生成变更日志时产生清晰的记录。
