# 设备侧测试说明

**重要：设备侧排障时请在设置页打开“详细日志”。**

CI Build 和正式 Release 都会产出单个模块 zip：`storage.redirect.x-v<version>.zip`。默认只保留文件监视记录；打开“详细日志”后，会立即启用 Rust、Java、Stats 和诊断采集日志，关闭后立即停止相关记录。

设备侧回归测试需要配合独立测试 APP 仓库 [`StorageRedirectTest`](https://github.com/z1298808165/StorageRedirectTest)。测试仓库由执行者自行放置；本文只说明需要在哪个仓库中执行命令，不假设本地源码目录。

测试 APP 包名：

```text
me.fakerqu.test.storageredirect
```

测试 APP 通过 `TestService` 运行用例，服务 action 为：

```text
me.fakerqu.test.storageredirection.TEST_CASE
```

## 总体流程

设备侧回归默认以设备上当前已安装的 Storage Redirect X 模块为被测对象。除非用户明确要求编译/刷入本地模块，或明确说明要验证当前本地 `srx_core` 模块产物，否则不要自动构建模块、刷入模块或因此重启设备。

推荐按下面顺序验证：

1. 确认设备在线、root 可用，并记录当前模块版本。
2. 在 `StorageRedirectTest` 构建并安装测试 APP。
3. 运行 `StorageRedirectTest` 的场景脚本。
4. 查看摘要、失败项、模块日志和测试 APP 结果文件。

如果用户明确要求验证本地 `srx_core` 模块产物，则先构建并刷入本地模块 zip，重启设备后确认模块版本已生效，然后继续执行测试 APP 和场景脚本步骤。

当前完整设备侧通过标准是：

- `basic/all` 通过。
- 当前模块应跑完 scenario 1-28；如果显式设置 `RUN_FUSE_DAEMON_SCENARIOS=0` 或验证旧模块不支持 `fuse_daemon_redirect_enabled`，脚本会跳过 FUSE daemon 专属场景。
- 脚本最后输出 `ALL_SCENARIOS_PASSED`。

下面 PowerShell 示例默认先设置目标设备序列号：

```powershell
$Serial = "<serial>"
```

## 构建并刷入模块

这是可选步骤，仅在用户明确要求编译/刷入本地模块，或明确说明要验证当前本地 `srx_core` 模块产物时执行。普通设备侧回归不应默认执行这一节。

Windows PowerShell 下推荐在本仓库根目录使用脚本。脚本会编译 Android release 目标、生成可刷入 zip、校验 zip 条目和 LF 换行；如不传 `-NoAdb`，还会询问是否通过 `ksud module install` 刷入并重启。

```powershell
# 构建模块 zip，可按提示刷入设备
.\scripts\build-local-module.ps1

# 只构建 zip，不执行 ADB 安装
.\scripts\build-local-module.ps1 -NoAdb

# 只打包已有 target 产物，不执行 cargo build，也不检查 adb
.\scripts\build-local-module.ps1 -SkipBuild -NoAdb
```

手动刷入 zip 时，必须推送完整 zip，不要直接覆盖 `/data/adb/modules/storage.redirect.x` 下的 `.so` 或 daemon：

```powershell
adb -s $Serial push build\storage.redirect.x-v<version>.zip /data/local/tmp/storage.redirect.x-local.zip
adb -s $Serial shell "su -c 'rm -rf /data/adb/modules_update/storage.redirect.x'"
adb -s $Serial shell "su -c '/data/adb/ksu/bin/ksud module install /data/local/tmp/storage.redirect.x-local.zip'"
adb -s $Serial reboot
adb -s $Serial wait-for-device
adb -s $Serial shell 'while [ x$(getprop sys.boot_completed) != x1 ]; do sleep 1; done; echo booted'
adb -s $Serial shell "su -c 'cat /data/adb/modules/storage.redirect.x/module.prop'"
```

如果安装 CI artifact 中的模块 zip，流程相同：

```powershell
adb -s $Serial push storage.redirect.x-v<version>.zip /data/local/tmp/srx-ci.zip
adb -s $Serial shell "su -c 'rm -rf /data/adb/modules_update/storage.redirect.x'"
adb -s $Serial shell "su -c '/data/adb/ksu/bin/ksud module install /data/local/tmp/srx-ci.zip'"
adb -s $Serial reboot
adb -s $Serial wait-for-device
adb -s $Serial shell 'while [ x$(getprop sys.boot_completed) != x1 ]; do sleep 1; done; echo booted'
adb -s $Serial shell "su -c 'cat /data/adb/modules/storage.redirect.x/module.prop'"
```

刷入前建议额外确认 zip 至少包含 `module.prop`、`zygisk/arm64-v8a.so`、`bin/srx_daemon`、`service.d/debug_collectors.sh`、`service.d/media_state.sh`、`service.d/app_status.sh`，并且 zip entry 使用 `/` 而不是 `\`。

## 构建并安装测试 APP

`StorageRedirectTest` 已在仓库内优先配置国内下载源。Windows 下建议在测试 APP 仓库根目录直接使用 Gradle Wrapper：

```powershell
.\gradlew.bat --no-daemon :app:testDebugUnitTest :media-file-api:testDebugUnitTest :app:assembleDebug --console=plain --stacktrace
adb -s $Serial install -r app\build\outputs\apk\debug\app-debug.apk
```

授予测试 APP 必要权限：

```powershell
adb -s $Serial shell pm grant me.fakerqu.test.storageredirect android.permission.READ_EXTERNAL_STORAGE
adb -s $Serial shell pm grant me.fakerqu.test.storageredirect android.permission.WRITE_EXTERNAL_STORAGE
adb -s $Serial shell pm grant me.fakerqu.test.storageredirect android.permission.READ_MEDIA_IMAGES
adb -s $Serial shell pm grant me.fakerqu.test.storageredirect android.permission.READ_MEDIA_VIDEO
adb -s $Serial shell pm grant me.fakerqu.test.storageredirect android.permission.READ_MEDIA_AUDIO
adb -s $Serial shell pm grant me.fakerqu.test.storageredirect android.permission.POST_NOTIFICATIONS
adb -s $Serial shell appops set me.fakerqu.test.storageredirect MANAGE_EXTERNAL_STORAGE allow
```

部分 Android 版本会拒绝旧权限或通知权限，脚本会容忍这类失败；手动执行时以不阻断测试为准。

## 运行完整回归

优先在测试 APP 仓库根目录使用 `StorageRedirectTest` 的 PowerShell 场景脚本。脚本会写入不同 SRX 配置、启动测试 APP、隔离每个服务用例、清理结果目录，并在失败时抓取模块日志和相关 logcat。脚本结束时会按白名单清理测试 APP 结果目录、`srt_file_tests`、场景固定目录、随机 `srt_*` 媒体文件、固定测试媒体文件以及 MediaProvider 可能留下的 `.pending-<数字>-srt_*` / `.trashed-<数字>-srt_*` 临时名和同名冲突 ` (数字)` 后缀；不会递归删除整个公共媒体目录。

```powershell
.\.github\scripts\run-storage-redirect-scenarios.ps1 -Serial $Serial
```

如果只想跳过 `basic/all`，只跑场景脚本：

```powershell
.\.github\scripts\run-storage-redirect-scenarios.ps1 -Serial $Serial -SkipBasicAll
```

只跑部分场景时可以用 PowerShell 参数或环境变量：

```powershell
.\.github\scripts\run-storage-redirect-scenarios.ps1 -Serial $Serial -SkipBasicAll -Scenarios 9,17,19,22,23,24,26,28

$env:SRT_SCENARIOS = "9,17,19,22,23,24,26,28"
.\.github\scripts\run-storage-redirect-scenarios.ps1 -Serial $Serial -SkipBasicAll
Remove-Item Env:SRT_SCENARIOS
```

在 Git Bash、WSL、Linux 或 macOS 下，也可以在测试 APP 仓库根目录使用 bash 版脚本：

```bash
ANDROID_SERIAL=<serial> bash .github/scripts/run-storage-redirect-scenarios.sh
```

常用调试开关：

| 变量或参数 | 用途 |
| --- | --- |
| `-SkipBasicAll` | 跳过 `basic/all`，只跑场景脚本。 |
| `-Scenarios 9,17` / `SRT_SCENARIOS=9,17` | 只跑指定场景，范围为 1-28。 |
| `-FreshAppPerCase` / `SRT_FRESH_APP_PER_CASE=1` | 每个服务用例前都冷启动测试 APP，排查状态污染时使用。 |
| `RUN_FUSE_DAEMON_SCENARIOS=0/1` | 强制跳过或强制运行 FUSE daemon 专属场景；默认自动探测模块是否支持。 |
| `SRT_FILE_MONITOR_ENABLED=1` | 调试非监控场景时也开启全局文件监控；正式回归通常保持默认。 |
| `SRT_RESULT_POLL_MS`、`SRT_APP_LAUNCH_SETTLE_MS`、`SRT_SERVICE_CASE_SETTLE_MS`、`SRT_MOUNT_CONFIRM_TIMEOUT_MS` | 调整结果轮询、启动缓冲、用例间缓冲和等待 mount 日志的时间。 |

完整脚本覆盖以下场景：

| 场景 | 目的 |
| --- | --- |
| `basic/all` | 默认开启重定向后，验证 MediaStore 创建、读、写，以及文件 API 的 list/create/read/write/stat/access/truncate/ftruncate；不自动执行 MediaStore 删除、缩略图、query-path、chmod、link 或 symlink 用例。 |
| 1 | 无应用配置时保持真实路径写入。 |
| 2 | 默认重定向写入应用私有空间。 |
| 3 | `path_mappings` 将 `Download/SrtProbe` 映射到 `Download/Test`。 |
| 4 | 路径映射叠加真实路径放行时，映射优先。 |
| 5 | `allowed_real_paths=["Download"]` 时保持真实路径写入。 |
| 6 | `mapping_mode_only=true` 且未命中映射时保持真实路径写入。 |
| 7 | `mapping_mode_only=true` 且命中映射时写入映射目标。 |
| 8 | `mapping_mode_only` 加 `sandboxed_paths=[".xlDownload"]` 时验证 `.xlDownload`/`.xldownload` 沙盒化。 |
| 9 | `read_only_paths` 允许读取、`stat`、`access`，但拒绝写入、truncate、ftruncate、chmod、fchmod、link、symlink、删除、mkdir、rename。 |
| 10 | 映射目标为只读路径时，映射请求写入被拒绝。 |
| 11 | `allowed_real_paths` 内联排除和通配符排除：放行路径保持真实写入，排除目录和 `*.part` 写入应用私有空间。 |
| 12 | `excluded_real_paths` 旧字段兼容：并入真实路径放行的排除规则。 |
| 13 | `allowed_real_paths` 的 `?` 通配符：单字符匹配放行，多字符不匹配时进入应用私有空间。 |
| 14 | 多条 `path_mappings` 同时命中时使用最长前缀映射。 |
| 15 | 字符串形式 `sandboxed_paths` 与同路径 `path_mappings` 同时命中时，映射优先于局部沙盒。 |
| 16 | FUSE daemon 混合模式下，普通放行和 `*`/`?` 通配放行并存；普通应用和 MediaStore 系统代写命中时保持真实路径，不命中时进入应用私有空间。 |
| 17 | FUSE daemon 混合模式下，`read_only_paths` 支持 `!` 排除优先：父路径只读，排除子路径可写，未排除子路径拒绝写入。 |
| 18 | FUSE daemon 混合模式下，路径映射和只读规则共同存在时，写权限由映射最终目标决定。 |
| 19 | FUSE daemon 混合模式下，同一父级多个通配规则互不污染：分别放行、只读和未命中路径按各自规则处理。 |
| 20 | 关闭 FUSE daemon 时，默认 mount namespace 对 `allowed_real_paths` 的 `*`/`?` 通配规则执行回退，普通应用和 MediaStore 系统代写命中时保持真实路径。 |
| 21 | 关闭 FUSE daemon 时，默认 mount namespace 对 `read_only_paths` 通配规则执行回退，并保持读取允许、写入拒绝语义。 |
| 22 | 关闭 FUSE daemon 时，路径映射和只读规则共同存在时仍由映射最终目标决定写权限。 |
| 23 | 启用 `file_monitor_enabled` 且测试 APP 配置 `enabled=false` 时，普通公共路径写入和 MediaStore 系统代写成功后仍应记录成功监控日志。 |
| 24 | 启用文件监控且 FUSE daemon 关闭时，普通应用直写覆盖放行成功、映射成功、最终只读失败、只读排除成功。 |
| 25 | 启用文件监控且 FUSE daemon 开启时，普通应用直写覆盖放行成功、映射成功、最终只读失败、只读排除成功。 |
| 26 | 启用文件监控且 FUSE daemon 关闭时，MediaStore 系统代写覆盖放行成功、映射成功、最终只读失败、只读排除成功。 |
| 27 | 启用文件监控且 FUSE daemon 开启时，MediaStore 系统代写覆盖放行成功、映射成功、最终只读失败、只读排除成功。 |
| 28 | 启用 `read_only_paths=["Pictures/SrtReadOnlyMedia"]`，预置真实图片并扫描进 MediaStore，验证测试 APP 通过 MediaStore 查询仍能看到只读真实路径下的图片行。 |

场景脚本会同时检查测试 APP 视角和 root 视角的物理落点或拒绝结果；文件监控场景还会检查 `/data/adb/modules/storage.redirect.x/logs/file_monitor.log` 中的成功或失败记录。默认情况下，非文件监控场景会关闭 `file_monitor_enabled`，文件监控场景会显式开启它。

## 手动运行用例

最小配置表示为测试 APP 开启完整隔离：

```powershell
$config = '{"users":{"0":{"enabled":true}}}'
adb -s $Serial shell "su -c 'mkdir -p /data/adb/modules/storage.redirect.x/config/apps'"
$config | adb -s $Serial shell "su -c 'cat > /data/adb/modules/storage.redirect.x/config/apps/me.fakerqu.test.storageredirect.json'"
adb -s $Serial shell am force-stop me.fakerqu.test.storageredirect
adb -s $Serial shell am start -W -n me.fakerqu.test.storageredirect/.MainActivity
```

运行默认回归用例：

```powershell
adb -s $Serial shell am start-foreground-service -n me.fakerqu.test.storageredirect/.TestService -a me.fakerqu.test.storageredirection.TEST_CASE --es test_case all
```

常用单个用例：

```text
all
mediastore_query_image
mediastore_query_video
mediastore_query_audio
mediastore_query_file
mediastore_query_download
mediastore_query_read_only_image
mediastore_query_path_image
mediastore_query_path_video
mediastore_query_path_audio
mediastore_query_path_file
mediastore_query_path_download
mediastore_create_image
mediastore_create_video
mediastore_create_audio
mediastore_create_file
mediastore_create_download
mediastore_create_image_denied
mediastore_create_video_denied
mediastore_create_audio_denied
mediastore_create_file_denied
mediastore_create_download_denied
mediastore_read_image
mediastore_read_video
mediastore_read_audio
mediastore_read_file
mediastore_read_download
mediastore_write_image
mediastore_write_video
mediastore_write_audio
mediastore_write_file
mediastore_write_download
mediastore_delete_image
mediastore_delete_video
mediastore_delete_audio
mediastore_delete_file
mediastore_delete_download
mediastore_thumbnail_image
mediastore_thumbnail_video
file_list_dir
file_create
file_read
file_write
file_write_denied
file_delete
file_delete_denied
file_mkdir
file_mkdir_denied
file_rename
file_rename_denied
file_stat
file_access
file_readlink
file_truncate
file_truncate_denied
file_ftruncate
file_ftruncate_denied
file_chmod
file_chmod_denied
file_fchmod
file_fchmod_denied
file_link
file_link_denied
file_symlink
file_symlink_denied
```

File API 示例：

```powershell
adb -s $Serial shell am start-foreground-service -n me.fakerqu.test.storageredirect/.TestService -a me.fakerqu.test.storageredirection.TEST_CASE --es test_case file_write --es file_path /storage/emulated/0/Download/SrtProbe/srt_ci_probe.txt --es payload "storage-redirect-test:file:manual" --es expected_payload "storage-redirect-test:file:manual"
```

拒绝类用例用于验证只读规则：

```powershell
adb -s $Serial shell am start-foreground-service -n me.fakerqu.test.storageredirect/.TestService -a me.fakerqu.test.storageredirection.TEST_CASE --es test_case file_write_denied --es file_path /storage/emulated/0/Download/SrtReadOnly/write_denied.txt --es payload "blocked"

adb -s $Serial shell am start-foreground-service -n me.fakerqu.test.storageredirect/.TestService -a me.fakerqu.test.storageredirection.TEST_CASE --es test_case file_rename_denied --es file_path /storage/emulated/0/Download/SrtReadOnly/srt_read_only_seed.txt --es target_file_path /storage/emulated/0/Download/SrtReadOnly/renamed.txt
```

MediaStore 读写类用例需要先运行对应 create 用例，从结果中的 `uri=` 取出 `content://...`，再作为 `media_uri` 传入：

```powershell
adb -s $Serial shell am start-foreground-service -n me.fakerqu.test.storageredirect/.TestService -a me.fakerqu.test.storageredirection.TEST_CASE --es test_case mediastore_create_image

adb -s $Serial shell am start-foreground-service -n me.fakerqu.test.storageredirect/.TestService -a me.fakerqu.test.storageredirection.TEST_CASE --es test_case mediastore_read_image --es media_uri "content://media/external/images/media/12345"
```

支持参数：

| 参数 | 用途 |
| --- | --- |
| `media_uri` | MediaStore 读、写、删、缩略图用例的目标 URI。 |
| `file_path` | 文件读、写、删、创建、重命名源路径等用例的目标路径。 |
| `target_file_path` | `file_rename` / `file_rename_denied` 的目标路径。 |
| `file_dir` | 目录列表和 mkdir 类用例的目标目录。 |
| `file_name` | MediaStore 创建用例的文件名，或 `mediastore_query_path_*` / `mediastore_query_read_only_image` 用例用于定位目标行的文件名。 |
| `relative_path` | MediaStore 创建用例写入的相对目录，例如 `Download/SrtMonitor`；未传入时使用媒体类型默认目录。 |
| `keep_pending` | MediaStore 创建用例是否保留 `IS_PENDING=1`；支持 `1`、`true`、`yes`，用于创建后立即由同一测试 APP 继续读写 URI。 |
| `payload` | 写入内容。 |
| `expected_payload` | 读回校验内容。 |
| `expected_path` | `file_readlink` 的期望链接目标，或 `mediastore_query_path_*` / `mediastore_query_read_only_image` 的期望 cursor `DATA` 路径。 |
| `length` | `file_truncate` / `file_ftruncate` 的目标长度。 |
| `mode` | `file_access`、`file_chmod`、`file_fchmod` 的访问模式或权限模式；支持十进制、`0600` 八进制和 `0o600` 八进制写法。 |

## 查看结果和日志

测试 APP 结果文件可能出现在以下位置，场景脚本会自动查找：

```text
/sdcard/Android/data/me.fakerqu.test.storageredirect/files/test_case_result
/data/data/me.fakerqu.test.storageredirect/files/test_case_result
/data/media/0/Android/data/me.fakerqu.test.storageredirect/sdcard/Android/data/me.fakerqu.test.storageredirect/files/test_case_result
```

手动查看最近结果：

```powershell
$ResultPath = adb -s $Serial shell "su -c 'ls -t /sdcard/Android/data/me.fakerqu.test.storageredirect/files/test_case_result/result_*.txt /data/data/me.fakerqu.test.storageredirect/files/test_case_result/result_*.txt /data/media/0/Android/data/me.fakerqu.test.storageredirect/sdcard/Android/data/me.fakerqu.test.storageredirect/files/test_case_result/result_*.txt 2>/dev/null | head -1'"
adb -s $Serial shell "su -c 'cat $ResultPath'"
```

常用日志：

```powershell
# 测试 APP 日志
adb -s $Serial logcat -d -s StorageRedirectTest

# 模块相关 logcat
adb -s $Serial logcat -d -s StorageRedirect FileMonitorOp Stats srx_core

# 模块文件日志
adb -s $Serial shell "su -c 'echo ---running.log---; tail -120 /data/adb/modules/storage.redirect.x/logs/running.log 2>/dev/null || true; echo ---app_status.log---; tail -120 /data/adb/modules/storage.redirect.x/logs/app_status.log 2>/dev/null || true; echo ---file_monitor.log---; tail -120 /data/adb/modules/storage.redirect.x/logs/file_monitor.log 2>/dev/null || true; echo ---media_provider_state.log---; tail -120 /data/adb/modules/storage.redirect.x/logs/media_provider_state.log 2>/dev/null || true'"
```

卡住或 ANR 时抓栈：

```powershell
adb -s $Serial shell pidof me.fakerqu.test.storageredirect
adb -s $Serial shell pidof com.android.providers.media.module
adb -s $Serial shell "su -c 'kill -3 <pid>'"
adb -s $Serial shell "su -c 'ls -t /data/anr/trace_* | head -1'"
adb -s $Serial pull /data/anr/trace_XX trace_storage_redirect.txt
```

## 判断失败归属

先按失败类型区分问题来源：

| 现象 | 优先怀疑 |
| --- | --- |
| `TestService` 未产出结果、脚本找不到结果文件、单个场景互相污染 | 测试 APP 或场景脚本隔离问题。 |
| `file_list_dir` 超时但文件实际存在 | 测试 APP 列目录方式、服务生命周期或 FUSE 状态污染。 |
| 直接 File API 写入路径落点错误 | `srx_core` 路径重定向、映射、放行或只读规则。 |
| `file_write_denied` / `file_mkdir_denied` 等拒绝类用例意外成功 | `srx_core` 只读规则或映射目标只读判断。 |
| `mediastore_read_*` 出现 `open failed: EAGAIN`、MediaProvider FUSE 线程卡住 | `srx_core` MediaProvider/系统代写读写回退链路。 |
| 脚本中后段场景失败，但单独 `-SkipBasicAll` 能过 | 前置 MediaStore 或服务卡住后的状态污染，先修根因再判断后续场景。 |

分类时不要只看测试 APP 的 PASS/FAIL。至少同时看：

- 结果文件中的失败用例和 metadata。
- `/data/adb/modules/storage.redirect.x/logs/running.log` 中的 redirect/open/read-only 日志。
- 目标路径在 `/data/media/0` 和 `/data/media/0/Android/data/me.fakerqu.test.storageredirect/sdcard` 下的实际落点。
- MediaProvider 是否有 ANR 或 FUSE 工作线程堆积。

## 修改目标与重启

| 修改目标 | 需重启 | 备注 |
| --- | --- | --- |
| 测试 APP | 否 | `adb install -r` 后 force-stop 再启动即可。 |
| SRX 模块 .so / daemon | 是 | 重新打包 zip 并 `ksud module install`，重启后生效。 |
| SRX 应用配置 JSON | 通常否 | 配置热重载会生效；已运行目标 APP 建议 force-stop 后重启。 |
| 全局设置 `fuse_fix_enabled` | 建议是 | MediaProvider FUSE 状态可能需要重启进程或设备才完全刷新。 |

## 常见问题排查

### APP 无法打开或启动超时

1. 检查模块是否出现重复目录：

   ```powershell
   adb -s $Serial shell "su -c 'ls /data/adb/modules/ | grep storage | od -c'"
   ```

   正常只应有一个 `storage.redirect.x`。如果出现带 `\r` 的重复目录，按下面“模块目录出现重复”处理。

2. 检查模块运行状态：

   ```powershell
   adb -s $Serial shell "su -c 'ps -A | grep srx_daemon'"
   adb -s $Serial shell "su -c 'cat /data/adb/modules/storage.redirect.x/module.prop'"
   ```

3. 临时删除测试 APP 配置，判断是否是模块 hook 影响 APP 启动：

   ```powershell
   adb -s $Serial shell "su -c 'rm -f /data/adb/modules/storage.redirect.x/config/apps/me.fakerqu.test.storageredirect.json'"
   adb -s $Serial shell am force-stop me.fakerqu.test.storageredirect
   adb -s $Serial shell am start -W -n me.fakerqu.test.storageredirect/.MainActivity
   adb -s $Serial shell ps -A | findstr storageredirect
   ```

### 服务用例无响应或超时

1. 先停止测试 APP，清理结果目录后重跑：

   ```powershell
   adb -s $Serial shell am force-stop me.fakerqu.test.storageredirect
   adb -s $Serial shell "su -c 'rm -rf /sdcard/Android/data/me.fakerqu.test.storageredirect/files/test_case_result /data/data/me.fakerqu.test.storageredirect/files/test_case_result /data/media/0/Android/data/me.fakerqu.test.storageredirect/sdcard/Android/data/me.fakerqu.test.storageredirect/files/test_case_result'"
   ```

2. 确认前台服务启动日志：

   ```powershell
   adb -s $Serial logcat -d -s StorageRedirectTest ActivityManager
   ```

3. 如果卡在 MediaStore 用例，抓 MediaProvider 栈并查看 FUSE 线程是否卡在 native `read`。

### 模块目录出现重复

**现象**：`ls /data/adb/modules/` 显示两个 `storage.redirect.x` 目录，其中一个实际名称带 `\r`。

**原因**：Windows 下错误打包 zip 时 `module.prop` 使用 CRLF，`ksud` 把 `\r` 读进了 module id。

**修复**：

```powershell
adb -s $Serial shell "su -c 'find /data/adb/modules -maxdepth 1 -name \"*redirect*\" -exec stat -c \"%i %n\" {} \;'"
adb -s $Serial shell "su -c 'find /data/adb/modules -maxdepth 1 -inum <inode> -exec rm -rf {} \;'"
```

之后用 `scripts/build-local-module.ps1` 或 CI 同款脚本重新打完整 zip。

### `ksud module install` 报目录或缺少 arm64-v8a.so

不要用目录安装模块，也不要用 PowerShell `Compress-Archive` 直接打包。应使用 `scripts/build-local-module.ps1` 生成 zip，再安装：

```powershell
adb -s $Serial push build\storage.redirect.x-v<version>.zip /data/local/tmp/storage.redirect.x-local.zip
adb -s $Serial shell "su -c 'rm -rf /data/adb/modules_update/storage.redirect.x'"
adb -s $Serial shell "su -c '/data/adb/ksu/bin/ksud module install /data/local/tmp/storage.redirect.x-local.zip'"
```

如果安装器输出 `Module installed successfully!` 和 `reboot required`，说明 zip 已被接受；之后即使宿主机 ADB server 短暂断开，也应先恢复 ADB 并继续重启验证。

### 安装成功后 ADB server 断开

这通常是宿主机 ADB server 临时断开，不代表模块安装失败：

```powershell
adb devices
adb reboot
adb wait-for-device
adb shell 'while [ x$(getprop sys.boot_completed) != x1 ]; do sleep 1; done; echo booted'
adb shell "su -c 'cat /data/adb/modules/storage.redirect.x/module.prop'"
```

如 `adb devices` 无法恢复：

```powershell
adb kill-server
adb start-server
adb devices
```

### 图库类 APP 仍能看到相册

`StorageRedirectTest` 主要验证模块回归路径。排查第三方图库类 APP 时，仍需区分文件挂载隔离和 MediaStore 查询链路：

```powershell
adb -s $Serial shell "su -c 'cat /data/adb/modules/storage.redirect.x/config/apps/<package>.json'"
adb -s $Serial shell "su -c 'cmd package list packages -U | grep <package>'"
adb -s $Serial shell "su -c 'logcat -c'"
adb -s $Serial shell am force-stop <package>
# 手动打开目标 APP 并进入相册页后：
adb -s $Serial logcat -d -s SRX StorageRedirect | findstr /i "<package> MediaProvider java hook java query java cursor java open DCIM Camera"
```

如果目标配置里存在 `allowed_real_paths`，例如 `DCIM`、`Pictures`、`Android`，看到或写入这些路径是符合配置的，不应误判为 MediaProvider hook 失败。
