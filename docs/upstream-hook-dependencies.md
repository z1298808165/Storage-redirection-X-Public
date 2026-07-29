# 上游 Hook 依赖说明

本项目的核心 hook 能力依赖两个上游 Rust 库：

- `srx_hook`：<https://github.com/Kindness-Kismet/srx_hook>
- `srx_inline_hook`：<https://github.com/Kindness-Kismet/srx_inline_hook>

## 引用方式

这两个库不是把源码复制到本仓库中维护，而是在 `Cargo.toml` 中以 Cargo git 依赖方式引用：

```toml
# quality-allow(chinese-language): 以下两行是 Cargo.toml 依赖声明原文，属机器解析的配置语法，改写会失去参考价值。
srx_hook = { git = "https://github.com/Kindness-Kismet/srx_hook.git", rev = "5ec71115c4b674d282f337bb7523ba66471bd98a" }
# quality-allow(chinese-language): 同上，保留依赖声明原文以便直接比对 Cargo.toml。
srx_inline_hook = { git = "https://github.com/Kindness-Kismet/srx_inline_hook.git", rev = "e9abbd58e0c6ee459bb0f7724b896a70ece0a661" }
```

依赖按 `rev` 固定到具体 commit（而非跟随 `branch`），因此升级上游必须显式修改 `rev`，不会因上游推送而静默变化。

`Cargo.lock` 会锁定实际使用的上游 commit。构建时 Cargo 会把源码拉取到本机 Cargo git 缓存，例如 `~/.cargo/git/checkouts/`，而不是拉到本仓库目录下。

因此：

- 如果只是更新到上游已有修复，通常需要执行 Cargo 依赖更新并提交 `Cargo.lock` 的 commit 变化。
- 如果需要改上游库本身，必须到对应上游仓库提交修复或功能，再回到本仓库更新锁定的 commit。
- 不要在本仓库里直接复制、内嵌或临时改写这两个库的源码来绕过上游问题。

## 能力边界

### `srx_hook`

`srx_hook` 是 Android 64 位 PLT hook 库，主要负责 native/PLT 层符号 hook。它提供的能力包括：

- `init`、`hook_single`、`hook_partial`、`hook_all`、`refresh`、`clear` 等任务式 hook API。
- 通过 `with_prev_func`、`get_prev_func` 调用原函数。
- caller、callee、ignore 路径规则和模块身份识别。
- dlopen/dlclose 触发的自动刷新，以及 ELF 遍历和重定位处理。
- hook 递归防护、fork child 检测、信号保护等运行时能力。

本项目主要在以下位置使用它：

- `src/hook/stats.rs`：初始化 hook 引擎、注册 open/stat/access/mkdir/rename/read 等 PLT hook。
- `src/hook/runtime.rs`：通过 `with_prev_func` 回调原函数，并判断 fork child 场景。
- `src/hook/fuse_fix.rs`：对 MediaProvider/FUSE 相关 native 比较函数安装 hook。

### `srx_inline_hook`

`srx_inline_hook` 是面向 Android 运行时的纯 Rust inline hook 引擎，并提供 linker 辅助能力和 C ABI。它的能力包括：

- 按函数地址、符号地址、so 名和符号名安装 inline hook。
- 获取原函数跳板，支持 intercept 类场景。
- linker 辅助：符号查找、加载/卸载回调、pending hook/intercept。
- hook 记录、诊断和 trampoline 统计。

本项目当前主要在 `src/java_hook/lsplant.rs` 使用它的 linker 辅助 API：

- `sh_dlopen`
- `sh_dlsym`
- `sh_dlclose`

这些调用用于解析 ART/LSPlant 相关符号。若后续本项目直接使用 inline hook、intercept 或更复杂 linker 能力，也应优先判断是否属于 `srx_inline_hook` 的能力边界。

## 何时需要判断是否更新上游

当功能变更、缺陷修复或设备适配涉及以下内容时，必须额外判断问题是否属于上游库能力缺失或 bug，并在最终说明、Issue、PR 或提交说明中明确写出判断结果：

- PLT hook 注册、刷新、卸载、原函数回调、ignore/caller/callee 规则、fork child 防护、ELF/relocation 解析异常。
- inline hook、intercept、trampoline、linker 符号查找、dlopen/dlclose 回调、Android 版本或架构适配异常。
- hook 安装后出现 SIGSEGV、SIGBUS、死锁、递归调用、原函数指针为空、hook 丢失、重复 hook、跨进程 fork 后行为异常。
- 新功能需要上游目前没有提供的 hook primitive、符号过滤能力、诊断记录、稳定性保护或 Android 新版本支持。
- 本仓库只能通过复杂绕行、重复实现 hook 引擎内部逻辑、硬编码上游内部结构才能解决的问题。

判断后按以下方式处理：

- 若问题可以在本仓库业务层解决，说明不需要上游更新，并解释原因。
- 若需要上游已有修复，更新 Cargo git 依赖锁定 commit，并说明更新的是哪个上游库。
- 若需要上游新增能力或修复 bug，必须提醒需要更新 `srx_hook` 或 `srx_inline_hook`，并尽量描述需要的 API、行为或复现条件。
- 若暂时只能在本仓库 workaround，必须说明这是临时方案，并记录后续上游更新需求。

## 已判定的能力边界

### fork 子进程中的配置锁：暂不需要上游 atfork 能力

`should_bypass_hook_in_fork_child`（`src/hook/runtime.rs`）对系统代写进程与 shared-uid 进程**不**绕过 hook，因此这两类进程的 fork 子进程会进入完整重定向决策链，而链上使用阻塞锁：`SettingsHub` 的 `state.lock()`（`src/config/inspect.rs` 十余处、`src/config/raw_scan.rs`）与 `PathRouter` 的 `state.read()`（`src/redirect/router.rs`）。若 fork 恰好发生在另一线程持锁重载配置期间，子进程中该锁处于永久占用状态，第一次被拦截的 open 就会永久阻塞。`PATH_NORMALIZE_CACHE` 使用 `try_lock`（`src/platform/paths.rs`）说明该风险此前已被意识到。

**真机实测结论：该竞态的前提条件不成立，因此不改。** 在 Android 16（SDK 36、KernelSU）真机上用 `strace -f -e trace=clone,fork,vfork` 观测 MediaProvider（38 个线程）共 55 秒：25 秒 MediaStore 查询负载下 clone 调用 0 次；30 秒混合负载（文件写入、MediaStore 查询、媒体扫描广播）下同样 0 次。strace 本身工作正常（日志含 `Process <pid> detached`），进程快照也确认 MediaProvider 无子进程。模块日志中 shared-uid 相关记录同为 0。即「系统代写进程 fork」这一前提在实际环境中未观测到。

不修改的理由是收益与风险不对称：修复需触及整条决策链十余处锁语义，改错会导致热路径死锁或重定向被错误放行，比原风险更严重；而该竞态无法用外置测试构造，测试流也覆盖不到（普通应用不装进程内 hook，系统代写进程的 fork 时序不可控），缺乏验证手段。

**上游能力现状**：`srx_hook` 提供 `is_forked_child()`，实现为纯 pid 比较（`getpid() != INSTALL_PID`），async-signal-safe，可在 fork 子进程中安全调用；但**没有** `pthread_atfork` 注册能力。若将来出现确实会 fork 的系统代写进程（例如厂商定制 MediaProvider）而需要在子进程侧标记「仅走原函数」，则需要上游新增 atfork 钩子注册 API；在此之前不需要上游更新。

