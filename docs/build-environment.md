# 构建环境

本文描述本地 WSL 构建 `srx_core` 所需的工具与环境变量。Rust/Cargo 和 Android Gradle 构建、测试均在 WSL 执行；Windows PowerShell 入口可调用 WSL 完成这些步骤，设备安装和场景执行使用 Windows ADB，不向 WSL 转发 ADB。构建流程请见 [构建流程](build-process.md)。

## 工具要求

| 工具 | 最低版本 | 说明 |
|------|----------|------|
| Rust | nightly (edition 2024) | WSL 中需要 `let_chains` 等特性 |
| Android NDK | r30+ | WSL 中用于交叉编译和 LSPlant C++ 构建 |
| CMake | 3.28+ | WSL 中 LSPlant C++ 模块构建依赖（Ninja 生成器） |
| Ninja | 1.10+ | WSL 中的 CMake 生成器 |
| JDK | 21 | WSL 中编译 Hooker.java、运行 Gradle |
| Android SDK build-tools | 31+ | WSL 中提供 d8，将 class 转为 dex |
| curl | 任意现代版本 | WSL 首次构建下载 libcxx AAR（见下） |
| Rust targets | `aarch64-linux-android` | 真机；可选 `x86_64-linux-android` 用于模拟器 |
| Windows ADB | platform-tools | Windows PowerShell 直接连接设备并运行场景套件 |

### libcxx AAR（LSPlant std 模块依赖）

LSPlant 上游迁移到 C++ 标准库模块（`import std;`）后，构建需要
`org.lsposed.libcxx:libcxx` AAR 提供 `cxx` CMake 包与 std 模块源。
`build.rs` 会在首次构建时自动下载（URL 与 sha256 钉死在 `build.rs` 常量里）
到 `~/.cache/srx/libcxx-<版本>/` 并解包，无需手工准备。

注意：

- **AAR 版本必须与 NDK 版本一致**（std 模块与消费方 clang 同版本编译）。
  升级任一方时必须同步更新 `build.rs` 里的 `LIBCXX_AAR_*` 常量与
  `gradle.properties` / CI workflow 的 NDK 版本。
- 离线或预置环境可设置 `SRX_LIBCXX_PREFIX` 指向已解包的 AAR 根目录
  （需含 `prefab/modules/cxx/include`），跳过下载。
- WSL 解包使用 `unzip`；Windows 侧不参与模块编译。

### 重要限制

- **使用 CI 同款 JDK 21**。`build.rs` 使用 `--release 11` 编译 Hooker.java，Gradle 仍需满足自身的 Java 运行时要求。
- **构建工具必须安装在 WSL 中**。Windows 的 Cargo、Gradle 和 Android NDK 不参与本地构建或测试；Windows 侧只需可用的 ADB。

### 前置缺失时的典型报错

WSL 侧缺 SDK 变量或 Linux 版 NDK 时，构建入口会在构建阶段直接停下，报错与原因一一对应：

| 报错 | 原因 | 修法 |
|------|------|------|
| `ANDROID_HOME: parameter null or not set` | WSL 内没有导出 `ANDROID_HOME` / `ANDROID_SDK_ROOT` | 按上文「环境变量配置」写入 WSL shell profile 后重开终端 |
| `Android NDK <版本> not found in WSL SDK` | 该 SDK 目录下没有对应版本 NDK，或该 NDK 只装了 Windows 主机二进制 | 在 Linux 侧用 `sdkmanager "ndk;<版本>"` 安装，并确认 `prebuilt/linux-x86_64/bin/aarch64-linux-android29-clang` 存在 |

Windows 主机上装的 NDK 不能给 WSL 复用：`toolchains/llvm/prebuilt/` 按宿主平台分发，只有 `windows-x86_64`
一个目录时，必须在 WSL 内重新安装对应版本。

## 安装 Rust targets

```bash
rustup target add aarch64-linux-android
rustup target add x86_64-linux-android  # 可选，模拟器用
```

## 环境变量配置

编译前需设置以下环境变量：

```bash
export ANDROID_HOME="/path/to/android-sdk"  # Android SDK 安装目录
export ANDROID_SDK_ROOT="$ANDROID_HOME"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/30.0.16248370"
export JAVA_HOME="/path/to/jdk-21"  # JDK 21 安装目录
export PATH="$ANDROID_HOME/cmake/4.1.2/bin:$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH"  # 追加 CMake 与 NDK 工具链
```

将环境变量加入 WSL shell profile，重新打开 WSL shell 后生效。

## .cargo/config.toml

需要为 Android target 配置 linker。推荐先复制仓库模板，再把 NDK LLVM bin 加入 PATH：

```bash
cp .cargo/config.example.toml .cargo/config.toml  # 复制 linker 模板
export PATH="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH"  # 追加 NDK 工具链
```

模板内容使用不含本机绝对路径的命令名：

```toml
[target.aarch64-linux-android]
linker = "aarch64-linux-android29-clang"
ar = "llvm-ar"

[target.x86_64-linux-android]
linker = "x86_64-linux-android29-clang"
ar = "llvm-ar"
```

如果不想修改 PATH，也可以在本地 `.cargo/config.toml` 中写入 NDK 的绝对路径；该文件已被 `.gitignore` 忽略，不应提交本机路径。
