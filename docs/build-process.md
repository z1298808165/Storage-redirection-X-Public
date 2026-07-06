# 构建流程

本文描述 `srx_core` 的常用构建命令、产物检查和 Windows 相关注意事项。环境依赖请先参考 [构建环境](build-environment.md)。

## 构建命令

### 模块二进制构建

用于刷入设备的常规构建：

```bash
cargo build --target aarch64-linux-android --release
```

产物位于 `target/aarch64-linux-android/release/libsrx_core.so`。

模块二进制不再区分 debug/release 两套日志能力。默认只输出 `FileMonitorOp` 文件操作监控日志；需要普通 Rust/Java 调试日志、Stats 统计广播和诊断采集时，在设置页“模块设置”中打开“详细日志”。

Windows PowerShell 设备侧测试建议固定设置 NDK 和 CMake：

```powershell
$env:ANDROID_HOME = "<Android SDK 安装目录>"
$env:ANDROID_NDK_HOME = "$env:ANDROID_HOME\ndk\30.0.14904198"
$env:ANDROID_NDK_ROOT = $env:ANDROID_NDK_HOME
$env:Path = "$env:ANDROID_HOME\cmake\4.1.2\bin;$env:ANDROID_NDK_HOME\toolchains\llvm\prebuilt\windows-x86_64\bin;" + $env:Path
cargo build --target aarch64-linux-android --release
```

如果命令通过管道写法（例如 `2>&1 | Select-Object ...`）截取输出，PowerShell 可能把 cargo 写到 stderr 的进度信息显示成 `NativeCommandError`。判断编译是否成功时，以最终的 `Finished release profile`、命令真实退出码，以及 `target/aarch64-linux-android/release/libsrx_core.so` / `srx_daemon` 时间戳为准。

### 管理 App release 构建与签名

管理 App 的 release 构建会按以下优先级读取签名配置：

1. `SRX_APP_SIGNING_*` 环境变量；
2. Gradle property `srx.signing.*`；
3. 仓库根目录的 `keystore.properties`；
4. 用户目录私有配置 `~/.srx_core/signing/keystore.properties`。

推荐本地使用第 4 种方式：把 `srx-manager-release.jks` 和 `keystore.properties` 放在同一个用户目录私有文件夹中，并让 `storeFile` 使用相对文件名。这样仓库文档、提交历史和终端日志都不需要出现本机绝对路径。

```properties
storeFile=srx-manager-release.jks
storePassword=<keystore 密码>
keyAlias=srx-manager
keyPassword=<key 密码>
```

仓库根目录的 `keystore.properties`、`*.jks` 和用户目录私有配置都不应提交。若需要临时指定其他配置文件，可设置 `SRX_APP_SIGNING_PROPERTIES` 或 Gradle property `srx.signing.propertiesFile`。

```powershell
.\gradlew.bat --no-daemon --console=plain :app:assembleRelease
```

### 集成测试流构建

测试流 APP 已集成在本仓库 `tests/storage-redirect-test/`，Gradle 模块名为 `:storageRedirectTestApp` 和 `:storageRedirectTestMediaFileApi`。构建并运行测试 APP 单元测试：

```powershell
.\gradlew.bat --no-daemon --console=plain --stacktrace :storageRedirectTestApp:testDebugUnitTest :storageRedirectTestMediaFileApi:testDebugUnitTest :storageRedirectTestApp:assembleDebug
```

需要在本地预检或复现 GitHub Actions 失败时，可以运行完整测试流验证。默认会构建当前模块、刷入测试设备、重启设备、安装测试 APP 并执行 1-29 号设备侧场景：

```bash
bash scripts/verify-test-flow.sh
```

Windows PowerShell 环境没有可用 Bash 时使用同等入口：

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-test-flow.ps1
```

默认目标是 `aarch64-linux-android` / `arm64-v8a`。如果使用 x86_64 模拟器测试，可以设置：

```powershell
$env:SRX_TEST_TARGET_TRIPLE = "x86_64-linux-android"
$env:SRX_TEST_MODULE_ABI = "x86_64"
pwsh -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify-test-flow.ps1
```

调试测试 APP 构建问题时可以临时设置 `RUN_DEVICE_SCENARIOS=0`。公开仓库 PR、CI Build 和 Release workflow 会强制执行测试流门禁；CI/Release 会先构建一次 x86_64 测试模块 zip 和测试 APK，再在 Android 13/14/15/16 x86_64 模拟器上各自运行完整 scenario 1-29。CI 测试流默认启用 `SRT_FAIL_FAST=1` 和 300 秒单场景超时，并在临时模拟器中设置 `SRT_SKIP_FINAL_CLEANUP=1` 跳过最终清理；本地完整验证默认仍执行最终白名单清理，适合复用真机或模拟器。CI/Release 只有在测试流全部通过后才会继续发布资产、更新 `update.json` 或创建正式 Release。

### 版本号规则

`Cargo.toml` 中的 `[package].version` 是当前目标基础版本。提交前需要进入下一轮测试时，先手动修改 `Cargo.toml` 版本并提交；CI 不再自动把补丁号加 1。

CI 和本地测试构建默认生成预发布版本：

```text
<Cargo.toml version>-ci.N
```

`N` 是当前基础版本第一次出现在 `Cargo.toml` 以来，沿第一父提交链统计到 `HEAD` 的有效构建序号。自动更新 `update.json` 的提交会被排除，提交标题前缀包括 `CI：更新更新清单` 和 `发布：更新更新清单`。本地工作区存在未提交改动时，本地脚本和 Gradle 默认构建会把这次本地构建计为下一次序号，避免本地测试包与最后一次已提交 CI 包版本号冲突。

仓库还维护 `.github/build-version-baseline.json` 作为本地测试包和公开 CI 的构建序号基线。本地 `scripts/build-local-module.ps1`、`scripts/build-local-app.ps1` 成功产出 `*-ci.N` 包后会把当前基础版本的最高 `N` 写入该文件；公开 CI 成功发布测试构建并更新 `update.json` 时也会写入同一文件。分区同步到公开仓库时会合并私人源码和公开仓库两边的最高值，所以下一次构建总是从“已在本地或远端用过的最高 `N`”继续加 1。

Android `versionCode` 规则为：正式版 `major * 1000000 + minor * 10000 + patch * 100`；CI/本地测试版使用正式版前预留的 99 个编号，即 `baseCode - 100 + N`。因此 `1.2.57-ci.1` 到 `1.2.57-ci.99` 都低于正式版 `1.2.57`，同一基础版本超过 99 次测试构建时应提升 `Cargo.toml` 版本。

兼容例外：旧规则已经发布过 `1.2.57-ci.284`，因此 `1.2.57` 这一轮会从 `ci.286` 接续，并继续使用正式版 `1.2.57` 之前的测试版 `versionCode`。后续把 `Cargo.toml` 提升到 `1.2.58` 时不再带这个偏移，会按 `1.2.58-ci.1`、`1.2.58-ci.2` 继续。

正式 Release 仍使用纯基础版本，例如 `1.2.57`，tag 必须是 `v1.2.57`。如需查看当前仓库会解析出的版本，可运行：

```powershell
py .github\scripts\resolve_build_version.py --include-dirty --format json
```

### 更新清单

管理 App 的“检查更新”不再直接请求 GitHub Releases API，而是读取仓库分支上的静态 `update.json`，避免未认证 GitHub API 的每出口 IP 频率限制导致 `HTTP 403`。

默认清单地址由构建时的仓库和分支生成：

```text
https://raw.githubusercontent.com/<owner>/<repo>/<branch>/update.json
```

构建脚本会优先使用 `srx.updateManifestUrl` / `SRX_UPDATE_MANIFEST_URL` 指定的完整地址；未指定时使用 `srx.releaseRepository` / `SRX_RELEASE_REPOSITORY` / `GITHUB_REPOSITORY` / git remote 推断仓库，再用 `srx.releaseBranch` / `SRX_RELEASE_BRANCH` / `GITHUB_REF_NAME` / 当前本地分支推断分支。

本地在 `SRX-R` 分支构建下游仓库时，默认会检查该下游仓库 `SRX-R` 分支的 `update.json`。如果需要临时固定到某个来源，可以显式指定：

```powershell
$env:SRX_UPDATE_MANIFEST_URL = "https://raw.githubusercontent.com/z1298808165/Storage-redirection-X-Public/SRX-R/update.json"
.\gradlew.bat --no-daemon --console=plain :app:assembleRelease
```

CI 构建成功后会把测试版信息写入 `beta`；正式 Release 成功后会把正式版信息写入 `stable`。下游同步本文件后，只要在自己的仓库运行同样的 CI/Release，`update.json` 会自动写成下游自己的 `GITHUB_REPOSITORY` 和资产地址，使用下游编译出的 App 仍会检查下游仓库的更新。

`update.json` 的基本结构：

```json
{
  "schema": 1,
  "repository": "owner/repo",
  "stable": {
    "version": "1.2.57",
    "tag": "v1.2.57",
    "title": "Storage Redirect X v1.2.57",
    "url": "https://github.com/owner/repo/releases/tag/v1.2.57",
    "prerelease": false,
    "downloadUrl": "https://github.com/owner/repo/releases/download/v1.2.57/storage-redirect-x-manager-v1.2.57.apk"
  },
  "beta": null,
  "releases": []
}
```

## 编译后验证

编译完成后建议验证以下两点：

1. **Hooker.dex 正确嵌入**，大小应大于等于 10KB：

   ```bash
   ls -la target/aarch64-linux-android/release/build/srx_core-*/out/Hooker.dex
   # 正常应为 ~12KB。如果是 0 字节或 < 1KB，说明 dex 编译失败
   ```

2. **LSPlant 正确链接**，so 大小应大于等于 1.8MB：

   ```bash
   ls -la target/aarch64-linux-android/release/libsrx_core.so
   # 正常 release 构建约 1.8-2.1MB
   # 如果明显偏小（< 500KB），说明 LSPlant 未链接
   ```

如果 Hooker.dex 编译失败，Android release 构建会直接失败，避免产出缺少 Java hook 的模块。仅在 host/debug 开发场景，或显式设置 `SRX_ALLOW_EMPTY_HOOKER_DEX=1` 时，构建脚本才会写入空文件并输出 warning；此类产物不应用于日常刷入或发布。

## Windows 注意事项

### Host 上直接运行 `cargo test`

`srx_hook` 是 Android-only 依赖，在 Windows host 直接执行普通 `cargo test` 可能报：

```text
srx_hook supports Android only (use cargo clippy/test/doc on host for development)
```

这不代表 Android 目标构建失败。设备侧行为的主验证命令仍是 `cargo build --target aarch64-linux-android --release`；若要在 host 跑单元测试，需要先为相关依赖补 host cfg 或拆出纯逻辑测试。

### d8 内部类文件名问题

Windows 上 `d8.bat` 可能无法正确处理包含 `$` 的文件名，例如 `Hooker$FilteringCursor.class`。症状是 `Hooker.dex` 只有几百字节，而正常应接近 12KB。

处理方式是把 `build.rs` 里的 d8 调用改成 `@argfile` 传参：

```rust
let argfile = out_dir.join("d8_args.txt");
let argfile_content = class_files
    .iter()
    .map(|p| p.display().to_string())
    .collect::<Vec<_>>()
    .join("\n");
std::fs::write(&argfile, &argfile_content)?;

let mut d8_cmd = Command::new(&d8);
d8_cmd
    .arg(format!("@{}", argfile.display()))
    .args(["--min-api", "31"])
    .arg("--output")
    .arg(out_dir);
```

### JDK 版本兼容

如果使用 JDK 22+，将 `build.rs` 里 javac 参数从下面这组：

```rust
.args(["-source", "8", "-target", "8"])
```

改成：

```rust
.args(["--release", "11"])
```
