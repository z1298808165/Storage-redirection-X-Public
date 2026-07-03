# 构建环境

本文只描述本地编译 `srx_core` 需要准备的工具与环境变量。构建流程请见 [构建流程](build-process.md)。

## 工具要求

| 工具 | 最低版本 | 说明 |
|------|----------|------|
| Rust | nightly (edition 2024) | 需要 `let_chains` 等特性 |
| Android NDK | r30+ | 用于交叉编译和 LSPlant C++ 构建 |
| CMake | 3.22+ | LSPlant 构建依赖 |
| Ninja | 1.10+ | CMake 生成器 |
| JDK | 11-21 | 编译 Hooker.java 为 class 文件 |
| Android SDK build-tools | 31+ | 提供 d8 工具将 class 转为 dex |
| Rust targets | `aarch64-linux-android` | 真机；可选 `x86_64-linux-android` 用于模拟器 |

### 重要限制

- **推荐 JDK 21 或以下版本**。`build.rs` 现在使用 `--release 11` 编译 Hooker.java；如果本机 JDK 版本更高，仍建议优先使用 CI 同款 JDK 21 以减少工具链差异。
- **Windows 上 d8 对 `$` 文件名有问题**。内部类文件名如 `Hooker$FilteringCursor.class` 中的 `$` 可能被 cmd/bat 解释为变量，导致 dex 只生成一部分内容。这个问题的处理方式见 [构建流程](build-process.md#windows-注意事项)。

## 安装 Rust targets

```bash
rustup target add aarch64-linux-android
rustup target add x86_64-linux-android  # 可选，模拟器用
```

## 环境变量配置

编译前需设置以下环境变量：

```bash
# Linux/macOS
export ANDROID_NDK_HOME=<Android NDK 安装目录>
export ANDROID_HOME=<Android SDK 安装目录>
export JAVA_HOME=<JDK 安装目录>

# Windows PowerShell
$env:ANDROID_HOME = "<Android SDK 安装目录>"
$env:ANDROID_NDK_HOME = "$env:ANDROID_HOME\ndk\30.0.14904198"
$env:JAVA_HOME = "<JDK 安装目录>"
```

确保 `ninja` 在 PATH 中。Android SDK 自带 ninja：

```powershell
# Windows - 添加 SDK 自带的 cmake/ninja 到 PATH
$env:PATH = "$env:ANDROID_HOME\cmake\4.1.2\bin;" + $env:PATH
```

## .cargo/config.toml

需要为 Android target 配置 linker。推荐先复制仓库模板，再把 NDK LLVM bin 加入 PATH：

```powershell
Copy-Item .cargo/config.example.toml .cargo/config.toml
$env:PATH = "$env:ANDROID_NDK_HOME\toolchains\llvm\prebuilt\windows-x86_64\bin;" + $env:PATH
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
