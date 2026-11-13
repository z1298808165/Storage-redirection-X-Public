# 模块打包

本文描述 Zygisk 刷入模块的目录结构、打包方式和常见注意事项。构建产物说明请先参考 [构建流程](build-process.md)。

## 模块产物

CI Build 和正式 Release 都产出单个刷入包：

- `storage.redirect.x-v<version>.zip`：日常使用和问题定位共用。默认文件监控和概览页轻量运行时生效计数通过 `srx_daemon` 私有通道直接写入文件，不启动常驻 `logcat`；需要排查问题时，在管理 App 或模块 WebUI 的设置页“模块设置”中打开“详细日志”，模块会启用 native/Java running 日志和 media/app 状态采集，关闭后立即停止这些详细记录。

## 目录结构

```
module.zip
├── META-INF/com/google/android/
│   ├── update-binary
│   └── updater-script
├── customize.sh
├── COPYING
├── LICENSE
├── module.prop
├── post-fs-data.sh
├── service.sh
├── uninstall.sh
├── sepolicy.rule
├── zn_modules.txt
├── service.d/
│   ├── app_status.sh
│   ├── boot.sh
│   ├── common.sh
│   ├── config_events.sh
│   ├── debug_collectors.sh
│   ├── log_collectors.sh
│   └── media_state.sh
└── zygisk/
    └── arm64-v8a.so
```

## 打包步骤

1. 将 `assets/zygisk_module/` 中的所有文件作为模块模板。
2. 将编译产物 `libsrx_core.so` 复制为 `zygisk/arm64-v8a.so`。
3. 将 `srx_daemon` 复制到 `bin/srx_daemon`。
4. 从仓库根目录复制 `LICENSE` 和 `COPYING`，确保刷入包保留 GPL-3.0-or-later 声明、上游来源说明和 GPL 正文。
5. 生成 `module.prop`，版本号按 [构建流程](build-process.md#版本号规则) 从 `Cargo.toml` 基础版本和当前构建序号解析。
6. 打包为 zip。更新日志保留在 CI 记录或 GitHub Release 正文中，不写入刷入包。

`zn_modules.txt` 选择 `/system_ext/bin/hyos_spawner`，并声明 companion 与同 ABI 的 `zygisk/<abi>.so`。shell 打包入口将模板的 arm64 路径替换为目标 ABI，PowerShell 入口固定打包 arm64。该库同时导出 ZN API v4 的 `zn_module`、`zn_companion_module`，在 Hyos runtime v1 注册特化回调；回调通过 companion 按 root 配置建立应用存储视图，不在特化子进程中初始化 Java/native hook。测试覆盖与运行边界见 [设备侧测试说明](device-testing.md#native-单元测试与-zn-回调夹具)。

## 打包注意事项

- **zip 内路径必须使用正斜杠 `/`**。Windows 的 `Compress-Archive` 或 `ZipFile.CreateFromDirectory` 会使用反斜杠 `\`，Android recovery/KernelSU 无法识别。需要使用 `ZipArchive.CreateEntry` 手动指定正斜杠路径。
- **安装关键文本条目保持不压缩**。模块安装器会在设备侧再次解压 zip，部分 Magisk/rootAVD x86_64 环境对压缩文本条目的兼容性不稳定，可能把脚本解成 0 字节。因此 `*.sh`、`*.prop`、`*.rule`、`zn_modules.txt`、`META-INF/*` 和 `bin/srxctl` 在 Linux/macOS 使用 Store，在 PowerShell 使用 `CompressionLevel.NoCompression`；体积较大的 native 二进制、WebUI 和其它资源使用 Deflate 压缩。
- **shell 脚本必须使用 Unix 换行符 (LF)**。Windows 编辑器可能保存为 CRLF，导致 `\r` 被 shell 解释为命令的一部分而报错。
- **shell 脚本不能有 UTF-8 BOM**。BOM (`EF BB BF`) 会导致 shebang 行无法识别。
- **不要包含 `config/`、`logs/`、`tmp/` 目录**。这些是运行时生成的，由 manager app 管理。

## Linux/macOS 打包示例

在仓库根目录使用维护中的打包入口；第六个参数选择 ABI，arm64 默认可省略，x86_64 必须显式传入：

```bash
while IFS='=' read -r key value; do
  case "$key" in
    version) VERSION="$value" ;;
    version_code) VERSION_CODE="$value" ;;
  esac
done < <(python3 .github/scripts/resolve_build_version.py --include-dirty --format github) # 读取仓库版本
bash .github/scripts/package_module.sh "$VERSION" "$VERSION_CODE" ./target/aarch64-linux-android/release/libsrx_core.so ./target/aarch64-linux-android/release/srx_daemon "build/storage.redirect.x-v${VERSION}.zip" arm64-v8a
```

## Windows PowerShell 打包示例

使用 WSL 已构建的 arm64 产物，只执行打包与 zip 校验：

```powershell
.\scripts\build-local-module.ps1 -SkipBuild -NoAdb
```

该入口统一处理许可证、模块版本、正斜杠条目、文本编码及压缩方式；布局规则由脚本维护。

## Windows PowerShell 打包注意事项

- 如果 PATH 中没有 `bash`，请使用上文的 PowerShell 打包流程，不要尝试运行 `.github/scripts/package_module.sh`。
- 不要依赖 `Compress-Archive` 或 `ZipFile.CreateFromDirectory` 创建可刷入的模块 zip。它们可能生成 `zygisk\arm64-v8a.so` 之类的条目；KernelSU 随后会解压出名称中包含反斜杠的文件，安装器则会报告 `missing arm64-v8a lib path=.../zygisk/arm64-v8a.so`。
- 压缩前，请将 `.sh`、`.prop`、`.rule`、`zn_modules.txt` 和 `META-INF` 文件转换为不带 BOM 的 UTF-8 编码，并使用 LF 换行。CRLF 或 BOM 可能导致设备上的 shell 执行失败。
- `module.prop` 必须采用不带 BOM 的 UTF-8 编码，并使用 LF 换行。如果 `id=storage.redirect.x` 以 `\r` 结尾，KernelSU 可能会创建名为 `storage.redirect.x\r` 的第二个模块目录。
- 刷入包应移除 `action.sh`，但保留 `service.d/debug_collectors.sh`、`service.d/media_state.sh` 和 `service.d/app_status.sh`。这些脚本只会在运行时打开“详细日志”后开始记录。
- 必需的二进制文件布局为：`libsrx_core.so` 位于 `zygisk/arm64-v8a.so`，`srx_daemon` 位于 `bin/srx_daemon`。
