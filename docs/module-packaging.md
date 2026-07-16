# 模块打包

本文描述 Zygisk 刷入模块的目录结构、打包方式和常见注意事项。构建产物说明请先参考 [构建流程](build-process.md)。

## 模块产物

CI Build 和正式 Release 都产出单个刷入包：

- `storage.redirect.x-v<version>.zip`：日常使用和问题定位共用。默认文件监控通过 `srx_daemon` 私有通道直接写入文件，不启动常驻 `logcat`；需要排查问题时，在设置页“模块设置”中打开“详细日志”，模块会启用 native/Java running 日志、Stats 和 media/app 状态采集，关闭后立即停止这些详细记录。

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

## 打包注意事项

- **zip 内路径必须使用正斜杠 `/`**。Windows 的 `Compress-Archive` 或 `ZipFile.CreateFromDirectory` 会使用反斜杠 `\`，Android recovery/KernelSU 无法识别。需要使用 `ZipArchive.CreateEntry` 手动指定正斜杠路径。
- **安装关键文本条目保持不压缩**。模块安装器会在设备侧再次解压 zip，部分 Magisk/rootAVD x86_64 环境对压缩文本条目的兼容性不稳定，可能把脚本解成 0 字节。因此 `*.sh`、`*.prop`、`*.rule`、`META-INF/*` 和 `bin/srxctl` 在 Linux/macOS 使用 Store，在 PowerShell 使用 `CompressionLevel.NoCompression`；体积较大的 native 二进制、WebUI 和其它资源使用 Deflate 压缩。
- **shell 脚本必须使用 Unix 换行符 (LF)**。Windows 编辑器可能保存为 CRLF，导致 `\r` 被 shell 解释为命令的一部分而报错。
- **shell 脚本不能有 UTF-8 BOM**。BOM (`EF BB BF`) 会导致 shebang 行无法识别。
- **不要包含 `config/`、`logs/`、`tmp/` 目录**。这些是运行时生成的，由 manager app 管理。

## Linux/macOS 打包示例

```bash
#!/bin/bash
while IFS='=' read -r key value; do
  case "$key" in
    version) VERSION="$value" ;;
    version_code) VERSION_CODE="$value" ;;
  esac
done < <(python .github/scripts/resolve_build_version.py --include-dirty --format github)
OUT_DIR="build/module"
rm -rf "$OUT_DIR"

# 复制模板
cp -r assets/zygisk_module/* "$OUT_DIR/"
cp LICENSE "$OUT_DIR/LICENSE"
cp COPYING "$OUT_DIR/COPYING"

# 复制 so 和 daemon
mkdir -p "$OUT_DIR/zygisk"
cp target/aarch64-linux-android/release/libsrx_core.so "$OUT_DIR/zygisk/arm64-v8a.so"
mkdir -p "$OUT_DIR/bin"
cp target/aarch64-linux-android/release/srx_daemon "$OUT_DIR/bin/srx_daemon"
chmod 755 "$OUT_DIR/bin/srx_daemon"

# 生成 module.prop
cat > "$OUT_DIR/module.prop" << EOF
id=storage.redirect.x
name=Storage Redirect X
version=$VERSION
versionCode=$VERSION_CODE
author=Storage Redirect Team
description=Storage Redirect X module.
EOF

# 打包（关键安装文本使用 Store，其余条目使用 Deflate）
cd "$OUT_DIR"
find . -type f \( -name '*.sh' -o -name '*.prop' -o -name '*.rule' -o -path './META-INF/*' \) -exec dos2unix {} \;
OUTPUT_ZIP="../../storage.redirect.x-v${VERSION}.zip"
rm -f "$OUTPUT_ZIP"
zip -9 -r "$OUTPUT_ZIP" . -x '*.sh' '*.prop' '*.rule' 'META-INF/*' 'bin/srxctl'
find . -type f \( -name '*.sh' -o -name '*.prop' -o -name '*.rule' -o -path './META-INF/*' -o -path './bin/srxctl' \) \
  -print | zip -0 "$OUTPUT_ZIP" -@
```

## Windows PowerShell 打包示例

下例可在没有 `bash`、`zip`、`dos2unix` 的 Windows 环境中生成可刷入 zip。实际本地打包推荐直接使用 `scripts/build-local-module.ps1`。

```powershell
$versionData = py .github\scripts\resolve_build_version.py --include-dirty --format json | ConvertFrom-Json
$version = $versionData.version
$versionCode = [int]$versionData.version_code
$templateDir = "assets\zygisk_module"
$soFile = "target\aarch64-linux-android\release\libsrx_core.so"
$daemonFile = "target\aarch64-linux-android\release\srx_daemon"
$pkgDir = "build\module"
$zipPath = "build\storage.redirect.x-v${version}.zip"

# 准备目录
Remove-Item -Recurse -Force $pkgDir -ErrorAction SilentlyContinue
Remove-Item -Force $zipPath -ErrorAction SilentlyContinue
Copy-Item -Recurse $templateDir $pkgDir
Copy-Item LICENSE "$pkgDir\LICENSE"
Copy-Item COPYING "$pkgDir\COPYING"
New-Item -ItemType Directory -Path "$pkgDir\zygisk" -Force | Out-Null
Copy-Item $soFile "$pkgDir\zygisk\arm64-v8a.so"
New-Item -ItemType Directory -Path "$pkgDir\bin" -Force | Out-Null
Copy-Item $daemonFile "$pkgDir\bin\srx_daemon"
Remove-Item -Force "$pkgDir\action.sh" -ErrorAction SilentlyContinue

# 生成 module.prop。必须使用 LF，避免 id 被 KernelSU 读成 storage.redirect.x\r。
$moduleProp = "id=storage.redirect.x`nname=Storage Redirect X`nversion=v${version}`nversionCode=${versionCode}`nauthor=Kindness-Kismet`ndescription=Storage Redirect X Core Module`nwebui=1`n"
[System.IO.File]::WriteAllText("$pkgDir\module.prop", $moduleProp, [System.Text.UTF8Encoding]::new($false))

# 修复换行符和 BOM
$textFiles = Get-ChildItem -Recurse $pkgDir -File | Where-Object {
    $_.Extension -in ".sh", ".prop", ".rule" -or $_.FullName -like "*\META-INF\*"
}
foreach ($file in $textFiles) {
    $bytes = [System.IO.File]::ReadAllBytes($file.FullName)
    if ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF) {
        $bytes = $bytes[3..($bytes.Length - 1)]
    }
    $content = [System.Text.Encoding]::UTF8.GetString($bytes).Replace("`r`n", "`n")
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($file.FullName, $content, $utf8NoBom)
}

# 打包（使用正斜杠路径，并让安装关键文本保持 no compression）
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::Open($zipPath, [System.IO.Compression.ZipArchiveMode]::Create)
$pkgRoot = (Resolve-Path $pkgDir).Path
Get-ChildItem -Recurse $pkgDir -File | ForEach-Object {
    $relativePath = $_.FullName.Substring($pkgRoot.Length).TrimStart([char]92).Replace([char]92, [char]47)
    $keepUncompressed = $_.Extension -in @(".sh", ".prop", ".rule") -or
        $relativePath.StartsWith("META-INF/") -or $relativePath -eq "bin/srxctl"
    $compression = if ($keepUncompressed) {
        [System.IO.Compression.CompressionLevel]::NoCompression
    } else {
        [System.IO.Compression.CompressionLevel]::Optimal
    }
    $entry = $zip.CreateEntry($relativePath, $compression)
    $entryStream = $entry.Open()
    $fileStream = [System.IO.File]::OpenRead($_.FullName)
    $fileStream.CopyTo($entryStream)
    $fileStream.Close()
    $entryStream.Close()
}
$zip.Dispose()

# 验证关键 entry 名必须使用 /，不能出现 zygisk\arm64-v8a.so。
$zip = [System.IO.Compression.ZipFile]::OpenRead($zipPath)
try {
    $names = $zip.Entries | Select-Object -ExpandProperty FullName
    if ($names -notcontains "zygisk/arm64-v8a.so") { throw "missing zygisk/arm64-v8a.so" }
    if ($names | Where-Object { $_ -match "\\" }) { throw "zip contains backslash entry" }
} finally {
    $zip.Dispose()
}
```

## Windows PowerShell packaging pitfalls

- If `bash` is not available on PATH, use the PowerShell packaging flow above instead of trying to run `.github/scripts/package_module.sh`.
- Do not rely on `Compress-Archive` or `ZipFile.CreateFromDirectory` for flashable module zips. They can create entries such as `zygisk\arm64-v8a.so`; KernelSU then extracts a literal backslash filename and the installer reports `missing arm64-v8a lib path=.../zygisk/arm64-v8a.so`.
- Convert `.sh`, `.prop`, `.rule`, and `META-INF` files to LF and UTF-8 without BOM before zipping. CRLF or BOM can break shell execution on device.
- `module.prop` must be LF and UTF-8 without BOM. If `id=storage.redirect.x` ends with `\r`, KernelSU may create a second module directory named `storage.redirect.x\r`.
- 刷入包应移除 `action.sh`，但保留 `service.d/debug_collectors.sh`、`service.d/media_state.sh` 和 `service.d/app_status.sh`。这些脚本只会在运行时打开“详细日志”后开始记录。
- The required binary layout is `zygisk/arm64-v8a.so` for `libsrx_core.so` and `bin/srx_daemon` for `srx_daemon`.
