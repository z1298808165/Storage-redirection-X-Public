# AI Agent 代码质量审核

本仓库的代码质量审核由 AI Agent 执行。静态脚本只负责拦截可机械判断的残留并验证审核凭据，不能替代 AI 对代码必要性、行为和维护成本的判断。

## 提交前流程

1. 只暂存一个职责单一的改动，运行 `git diff --cached --stat`、`git diff --cached` 并阅读相关调用链，不能只看文件名或摘要。
2. AI Agent 检查改动是否必要，是否存在无用代码、测试残留、重复实现、过度抽象、占位逻辑、错误或资源泄漏、并发或全局状态风险、兼容性问题和 hook 边界错误。
3. 运行与风险相称的格式化、静态检查、构建或测试。没有适用测试时，必须记录实际完成的代码路径检查，不能虚构命令或结果。
4. AI Agent 根据 `docs/ai-review-report.example.json` 在 `temp/` 下生成 JSON 报告。报告中的 `baseCommit`、`tree` 和 `files` 必须分别来自当前 `git rev-parse HEAD`、`git write-tree` 和 `git diff --cached --name-only --diff-filter=ACMRD`。
5. 运行 `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/record-ai-review.ps1 -ReportPath temp/<report>.json` 记录审核凭据，然后正常提交。

报告只有在所有阻塞发现均已修复、九项检查均有具体证据且结论为 `pass` 时才能登记。修改暂存内容、切换 `HEAD` 或改写报告后，旧凭据自动失效，必须重新审核。

## 审核标准

- `scope_and_necessity`：每处新增或修改都服务于本 Commit 的唯一目的；没有顺手重构、未来可能用到的扩展点或不可达分支。
- `dead_code_and_test_residue`：没有无用函数、未使用配置、调试输出、临时兼容层、内联测试、测试专用 helper 或测试后残留。
- `duplication_and_reuse`：没有复制已有能力；复用现有抽象时不会扭曲职责，新抽象确实减少重复或复杂度。
- `abstraction_and_complexity`：控制流、数据结构和层级与问题规模相称，没有包装层堆叠、过度泛化或隐藏副作用。
- `error_and_resource_paths`：失败、回滚、文件描述符、锁、内存、进程和生命周期路径完整，不以 `unwrap`、吞错或占位实现掩盖问题。
- `concurrency_and_global_state`：共享状态、锁顺序、原子语义、fork 前后状态及测试修改的全局状态均可恢复且无竞态。
- `compatibility_and_hook_boundaries`：Android 版本、ABI、普通应用与系统写入进程边界正确；需要上游 hook 能力时明确指出。
- `tests_and_verification`：验证覆盖真实风险；不得为了过门禁新增生产源码内联测试，也不得写完内联测试后在提交前删除来冒充持续回归保障。
- `readability_and_maintenance`：命名表达业务含义，注释解释约束而非复述代码，后续维护者无需猜测隐含前提。

## Git 门禁

- `pre-commit` 检查 Rust 内联测试白名单、增量代码残留和当前暂存 tree 的 AI 审核凭据。
- `commit-msg` 校验提交规范，并写入与提交 tree 绑定的 `AI-Review-*` trailers。
- `pre-push` 逐个检查待推送 Commit 的标题、代码质量、审核 trailers 和最终 Rust 内联测试白名单。
- 纯空提交没有代码差异，可以跳过 AI 代码审核；任何包含文件改动的 Commit 都不能跳过。

`quality-allow(<rule>): <具体原因>` 只用于静态规则的确切误报，必须紧邻对应代码。它不能豁免 AI 语义审核，也不能作为保留无用代码、测试残留或未完成实现的理由。
