# 上游 Hook 依赖说明

本项目的核心 hook 能力依赖两个上游 Rust 库：

- `srx_hook`：<https://github.com/Kindness-Kismet/srx_hook>（当前临时指向 fork，见下文「当前临时指向 fork」一节）
- `srx_inline_hook`：<https://github.com/Kindness-Kismet/srx_inline_hook>

`srx_hook` 固定到具体 commit（平时指向官方仓库，当前临时指向 fork 的修复提交）。锁定版本提供独立的运行时管理状态、fork 后状态发布保护，以及能够报告注册和清理结果的 checked API。

## 引用方式

这两个库不是把源码复制到本仓库中维护，而是在 `Cargo.toml` 中以 Cargo git 依赖方式引用：

```toml
# quality-allow(chinese-language): 紧随其后一行同步 Cargo.toml 中固定版本的依赖声明（当前临时指向 fork），保留原文便于直接核对。
srx_hook = { git = "https://github.com/z1298808165/srx_hook.git", rev = "88e54766a6117cf6bbb2f6e38254d297afdf41d2" }
# quality-allow(chinese-language): 紧随其后一行同为依赖声明原文，保留原样便于直接与 Cargo.toml 比对。
srx_inline_hook = { git = "https://github.com/Kindness-Kismet/srx_inline_hook.git", rev = "c8abb2e7249dc72f5e1113c3eb1def202fa0d3cb" }
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
- `hook_single_checked`、`hook_partial_checked`、`hook_all_checked` 返回状态、可选 stub 和模块错误；`stub` 存在时任务已经注册，即使调用同时返回错误。
- `hook_batch_checked` 先校验整批请求，再以一次状态发布注册；错误结果中的 stub 按请求顺序列出已发布任务。
- `clear_checked` 返回移除的 slot、剩余 slot、待恢复页面数量及模块错误。

`fork_safe()` 只表示库的 `pthread_atfork` 处理器已注册成功。普通 `fork()` 后，库为子进程创建独立的运行时管理状态并保留继承调用链；Automatic 模式可通过再次调用 `init()` 为子进程恢复 monitor。若 `fork()` 与 loader 或初始化调用重叠，子进程的管理 API 会返回 `ForkContextUnavailable`，此时再次调用 `init()` 也无法恢复管理。该保证不覆盖调用方自己的锁、直接 `clone`/syscall、`vfork()` 或异步信号处理中的任意 API 调用。

本项目主要在以下位置使用它：

- `src/hook/stats.rs`：初始化 hook 引擎、批量注册 open/stat/access/mkdir/rename/read 等 PLT hook，并处理 checked 注册和清理结果。
- `src/hook/runtime.rs`：通过 `with_prev_func` 回调原函数，并判断 fork child 场景。
- `src/hook/fuse_fix.rs`：对 MediaProvider/FUSE 相关 native 比较函数安装 PLT hook，并保留已提交的 checked 注册任务。

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

### fork 子进程中的调用方锁

`should_bypass_hook_in_fork_child`（`src/hook/runtime.rs`）对系统 writer、SAF native bridge 和 shared-UID 进程继续执行 hook 决策；这些路径会访问 `SettingsHub` 的阻塞锁（`src/config/inspect.rs`、`src/config/raw_scan.rs`）和 `PathRouter` 的读锁（`src/redirect/router.rs`）。这些 SRX 调用方锁不由 `srx_hook` 的 atfork 处理器重置或保护。

## 临时依赖状态（待回收）

**这是过渡状态，不是长期依赖来源。** `srx_hook` 目前指向 fork 的修复提交，而不是官方仓库；`srx_inline_hook` 不受影响，仍指向官方仓库。`Cargo.toml` 中该声明用「【临时过渡，待回收】」标注，可用检索标记 `待回收 srx_hook fork` 定位。

- fork 分支：`fix/maps-only-module-ranges`
- 提交：`88e54766a6117cf6bbb2f6e38254d297afdf41d2`
- 上游 PR：<https://github.com/Kindness-Kismet/srx_hook/pull/2>

修复内容：上游 `enumerate_modules` 把 `dl_iterate_phdr` 与 `/proc/self/maps` 的结果取并集，而 `bionic_module_ranges` 只遍历前者。注入框架用自有 linker 映射的库（如 Zygisk Next 的 `libzygisk.so`）不在 bionic 的 solist 里，于是「有模块、无段范围」，被判定为模块租约不可用，导致整批 refresh 失败并回滚已提交的 hook。修复改为从 `/proc/self/maps` 为这类模块补齐 PT_LOAD 范围，使它们正常参与 hook。

回收条件：上游 PR 合并后，把依赖声明改回官方仓库 URL、`rev` 换成合并后的提交，同步 `Cargo.lock`，并删除本节。
