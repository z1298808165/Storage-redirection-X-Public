# Storage Redirect X

Storage Redirect X 的核心 Zygisk 模块，负责文件系统重定向、MediaProvider query hook、系统媒体服务代写路径处理和路径过滤。

## 分支说明

本分支基于原作者在正式开源前提供给本人的早期源码版本，并在此基础上进行了大量功能性修改。由于改动范围远超常规 PR 所能覆盖，且与原作者后续开源的正式版本存在较大差异，因此本分支不适合直接向原仓库发起 Pull Request，而是作为个人深度定制版保留。

## 模块能力

- 文件系统重定向：按用户、应用和路径规则重定向存储访问。
- MediaProvider 查询 Hook：让系统媒体服务能配合重定向后的路径工作。
- 系统 writer 代写处理：覆盖 `com.android.providers.media.module` 等系统进程按调用方写入的场景。
- 路径过滤：支持路径映射、真实路径放行、`!` 排除规则和仅映射模式下的局部沙盒路径。
- FUSE 兼容保护：内置 FuseFixer-compatible 保护，可通过全局配置关闭。
- 配置模板与备份还原：WebUI 和管理 App 支持保存、应用、批量应用配置模板，并可导出和还原互通的 ZIP 备份。

## 开源协议

本仓库是基于 [Storage-redirection-X-Public](https://github.com/Kindness-Kismet/Storage-redirection-X-Public) 的修改版本。除单独标注的第三方组件或文件外，SRX Core 源码按 `GPL-3.0-or-later` 发布，详见 [LICENSE](LICENSE) 和 GPL 正文 [COPYING](COPYING)。

分发修改版或二进制构建时，必须按 `GPL-3.0-or-later` 提供对应源码并保留同等协议义务。模块刷入包会包含 `LICENSE` 和 `COPYING`，管理 App 和 WebUI 的“关于与开源协议”页面也会列出上游来源和协议。

## 映射是如何工作的

假设应用包名是 `com.example`，用户是 `0`。开启重定向后，默认重定向根目录是：

```text
/storage/emulated/0/Android/data/com.example/sdcard
```

默认情况下，应用访问：

```text
/storage/emulated/0/DCIM/MyApp/a.jpg
```

会落到：

```text
/storage/emulated/0/Android/data/com.example/sdcard/DCIM/MyApp/a.jpg
```

在默认重定向基础上，模块按以下优先级处理例外规则：

| 优先级 | 规则 | 行为 |
| --- | --- | --- |
| 1 | `read_only_paths` 写入保护 | 命中未被排除规则覆盖的只读目录时，写入返回只读文件系统错误；读取继续按后续规则处理。 |
| 2 | `path_mappings` | 按最长前缀匹配，把虚拟路径改写到指定真实路径。 |
| 3 | `mapping_mode_only` + `sandboxed_paths` | 仅映射模式下，命中 `sandboxed_paths` 的路径仍进入应用沙盒。 |
| 4 | `mapping_mode_only` 未命中 | 仅映射模式下，未命中映射和局部沙盒规则的路径保持原样。 |
| 5 | `allowed_real_paths` 中的 `!` 排除规则 | 完整隔离模式下，命中排除规则的路径重新进入应用沙盒。 |
| 6 | `allowed_real_paths` | 完整隔离模式下，命中后恢复同路径真实目录，不走默认重定向目录。 |
| 7 | 默认重定向 | 完整隔离模式下，其余存储路径进入默认重定向根目录。 |

用户输入的相对路径会先拼接到当前用户目录：

- 当前用户是 `0` 时，`DCIM/MyApp` 会解析为 `/storage/emulated/0/DCIM/MyApp`。
- 当前用户是 `10` 时，同样配置会解析为 `/storage/emulated/10/DCIM/MyApp`。

`path_mappings` 示例：

```text
映射规则：DCIM/MyApp -> Pictures/MyApp
输入路径：/storage/emulated/0/DCIM/MyApp/a.jpg
输出路径：/storage/emulated/0/Pictures/MyApp/a.jpg
```

若未命中任何规则且处于完整隔离模式：

```text
输入路径：/storage/emulated/0/Download/a.txt
输出路径：/storage/emulated/0/Android/data/com.example/sdcard/Download/a.txt
```

若命中 `allowed_real_paths`：

```text
允许路径：Download/Public
输入路径：/storage/emulated/0/Download/Public/a.txt
输出路径：保持不变
```

`allowed_real_paths` 和 `path_mappings` 可以同时存在。路径映射优先级更高，所以允许 `DCIM` 同时映射 `DCIM/MyApp -> Pictures/MyApp` 时，`DCIM` 下其他内容走真实路径，`DCIM/MyApp` 走映射目标；同一路径既允许又映射时，最终走映射目标。

## 手动配置

设备上的配置目录：

```text
/data/adb/modules/storage.redirect.x/config/
├─ global.json
├─ templates.json
└─ apps/
   └─ <包名>.json
```

配置支持多用户，每个用户对应 `users.<userId>`。常见用户 id 是主用户 `0`，工作资料或多开用户可能是 `10`、`11` 等。

### 全局配置 `global.json`

```json
{
  "file_monitor_enabled": true,
  "fuse_fix_enabled": true,
  "fuse_daemon_redirect_enabled": false,
  "verbose_logging_enabled": false,
  "auto_enable_redirect_for_new_apps": false,
  "auto_enable_new_apps_template_id": "",
  "app_config_auto_save": false
}
```

- `file_monitor_enabled`：启用文件创建监控；普通应用由 `srx_daemon` 在进程外监控隔离目录、放行真实路径和路径映射目标，`read_only_paths` 也会被纳入 daemon 监控。普通应用不安装进程内 PLT hook，因为不同应用的 native/图形/加固运行时兼容性不可控，安装后可能导致应用无法打开或闪退；真实 MediaProvider/FUSE 服务端仍使用 hook 保留调用方识别。DownloadProvider、ExternalStorageProvider、MTP、DocumentsUI、PhotoPicker 和厂商文件管理 UI 不再安装进程内 PLT hook，避免安装应用、设置、文件管理器和导出日志等系统存储链路被卡住。缺失、格式错误或不可读时，默认值为 `false`。
- `fuse_fix_enabled`：启用 SRX 内置 FuseFixer-compatible 保护，用于处理 MediaProvider/FUSE 路径检查中的默认可忽略 Unicode 码点。缺失、格式错误或不可读时，默认值为 `true`。
- `fuse_daemon_redirect_enabled`：启用混合 FUSE 重定向增强。普通路径仍使用 mount namespace；只有包含 `!`、`*`、`?` 的通配规则会在通配符前的最小具体父目录挂载模块内 FUSE daemon 精确匹配。关闭或 FUSE 启动失败时，默认 mount namespace 方案会退化通配规则。缺失、格式错误或不可读时，默认值为 `false`。
- `verbose_logging_enabled`：启用详细日志；打开后立即输出普通 Rust/Java/Stats logcat 日志并启动 `running.log`、`media_provider_state.log`、`app_status.log`、`stats` 等诊断采集，关闭后立即停止这些记录。文件监视记录不受该开关影响。缺失、格式错误或不可读时，默认值为 `false`。
- `auto_enable_redirect_for_new_apps`：通过 Zygisk 在 `system_server` 注册系统包事件接收器；收到新的第三方用户应用安装事件并完成 PackageManager 校验后，自动为该应用生成仅开启重定向的默认配置。模块会维护 `/data/adb/modules/storage.redirect.x/config/auto_new_apps_baseline` 作为基线，避免升级、重启或重复事件把旧应用误判为新应用。缺失、格式错误或不可读时，默认值为 `false`。
- `auto_enable_new_apps_template_id`：新应用自动重定向启用时使用的配置模板 ID。为空时使用仅开启重定向的默认配置。
- `app_config_auto_save`：控制 WebUI 应用配置页是否在每次配置操作结束后自动保存。缺失、格式错误或不可读时，默认值为 `false`，即仍需点击保存按钮。

### 应用配置 `apps/<包名>.json`

完整配置示例：

```json
{
  "users": {
    "0": {
      "enabled": true,
      "mapping_mode_only": false,
      "allowed_real_paths": [
        "Download/Public",
        "!Download/Public/tmp"
      ],
      "sandboxed_paths": [
        ".xlDownload"
      ],
      "read_only_paths": [
        "Documents/MyApp"
      ],
      "path_mappings": {
        "DCIM/MyApp": "Pictures/MyApp",
        "Download/Cache": "Android/media/com.example/cache"
      }
    }
  }
}
```

字段说明：

| 字段 | 类型 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | 是否启用当前用户的重定向配置。配置文件中存在该用户但未写此字段时会按启用处理。 |
| `mapping_mode_only` | bool | `false` | 仅映射模式。启用后不再执行完整沙盒 fallback，只应用 `path_mappings` 和 `sandboxed_paths`。 |
| `allowed_real_paths` | string array | `[]` | 完整隔离模式下的真实路径放行列表，命中后保持原路径。支持 `!` 排除规则，普通放行和排除规则都可使用 `*`、`?` 通配符；默认 mount namespace 会退化通配规则，FUSE daemon 可精确匹配。 |
| `excluded_real_paths` | string array | `[]` | 旧版兼容字段。读取时会并入 `allowed_real_paths` 的 `!` 排除规则；新配置建议直接写 `allowed_real_paths`。 |
| `sandboxed_paths` | string 或 string array | `[]` | 仅映射模式下的局部沙盒路径，命中后仍进入应用沙盒。 |
| `read_only_paths` | string 或 string array | `[]` | 只读模式目录。支持具体相对目录、`!` 排除前缀以及 `*`、`?` 通配符；运行时读取会按真实路径放行并禁止写入。 |
| `path_mappings` | object 或 array | `{}` | 路径映射规则，把虚拟路径改写到指定真实路径。 |

`path_mappings` 推荐写成对象：

```json
{
  "path_mappings": {
    "DCIM/MyApp": "Pictures/MyApp"
  }
}
```

也兼容数组形式：

```json
{
  "path_mappings": [
    {
      "request_path": "DCIM/MyApp",
      "final_path": "Pictures/MyApp"
    }
  ]
}
```

### 最小可用样板

```json
{
  "users": {
    "0": {
      "enabled": true
    }
  }
}
```

这个配置会让应用进入完整隔离模式：除明确放行或映射的路径外，其余 `/storage/emulated/<user>/` 下的访问都会进入应用沙盒。

### 多用户样板

```json
{
  "users": {
    "0": {
      "enabled": true,
      "allowed_real_paths": ["Download/OwnerOnly"]
    },
    "10": {
      "enabled": true,
      "mapping_mode_only": true,
      "sandboxed_paths": [".xlDownload"],
      "path_mappings": {
        "Movies/Input": "Movies/WorkMapped"
      }
    }
  }
}
```

### 手动改配置后如何生效

- 推荐：在模块操作里执行 `重载重定向`，或通过管理应用保存配置。
- 备选：手动停止目标应用、重启相关媒体服务进程，或直接重启设备。
- 只修改 `/data/adb/modules/storage.redirect.x/config/apps/*.json` 通常不需要刷入模块；WebUI 和管理 App 保存配置或批量应用模板后会触发配置热重载，但已运行进程可能仍需要重启后才完全使用新配置。

## 功能使用说明

### 完整隔离模式

默认模式是完整隔离模式，即 `mapping_mode_only=false`。适合希望减少应用在公共目录乱写的场景。

```json
{
  "users": {
    "0": {
      "enabled": true,
      "allowed_real_paths": ["Download/Public"],
      "path_mappings": {
        "DCIM/MyApp": "Pictures/MyApp"
      }
    }
  }
}
```

行为总结：

- 命中 `path_mappings`：改写到映射目标。
- 命中 `allowed_real_paths`：保持真实路径。
- 命中 `allowed_real_paths` 中的 `!` 排除规则：进入应用沙盒。
- 其他路径：进入 `/storage/emulated/<user>/Android/data/<package>/sdcard/...`。

### 仅映射模式 `mapping_mode_only`

仅映射模式适合“只想改写几个目录，不想把整包存储访问隔离”的场景。

```json
{
  "users": {
    "0": {
      "enabled": true,
      "mapping_mode_only": true,
      "path_mappings": {
        "DCIM/Camera": "Pictures/CameraBackup"
      }
    }
  }
}
```

行为总结：

- 命中 `path_mappings`：改写到映射目标。
- 未命中映射：保持原路径，不进入默认应用沙盒。
- `allowed_real_paths` 在这个模式下不再承担放行意义，因为未命中映射本来就会放行。

### 局部沙盒路径 `sandboxed_paths`

`mapping_mode_only=true` 时，如果某些应用仍会在存储根目录创建杂乱目录，可以用 `sandboxed_paths` 指定这些路径继续进入应用沙盒。

```json
{
  "users": {
    "0": {
      "enabled": true,
      "mapping_mode_only": true,
      "sandboxed_paths": [".xlDownload"],
      "path_mappings": {
        "Download/DLManager": "Download/第三方下载/DLManager"
      }
    }
  }
}
```

`sandboxed_paths` 支持字符串或字符串数组：

```json
{
  "sandboxed_paths": ".xlDownload"
}
```

```json
{
  "sandboxed_paths": [".xlDownload", "Tencent/MicroMsg"]
}
```

映射优先级高于 `sandboxed_paths`。如果同一路径既命中 `path_mappings` 又命中 `sandboxed_paths`，最终走映射目标。

### 只读模式 `read_only_paths`

`read_only_paths` 用来把某些真实目录保持可读但禁止写入。管理 App 和 WebUI 会把它做成独立的“只读模式”开关和路径区域；打开后只读路径写入 `read_only_paths`，不会混入“允许路径”列表展示。

```json
{
  "users": {
    "0": {
      "enabled": true,
      "read_only_paths": ["Documents/MyApp"]
    }
  }
}
```

- 普通应用不因为只读配置安装 PLT hook；这是稳定性约束，避免普通应用因 native/图形/加固运行时兼容问题出现无法打开或闪退。运行时通过应用 mount namespace 对目标目录做只读 bind mount。
- 真实 MediaProvider/FUSE 服务端仍在现有系统 writer hook 链路里判断调用方配置；DownloadProvider、ExternalStorageProvider、MTP、DocumentsUI、PhotoPicker 和厂商文件管理 UI 不进入进程内 PLT hook 链路；写入只读目录会返回 `EROFS`。
- 只读正向规则会提供真实读取通道；即使没有配置 `allowed_real_paths`，应用也能读取该目录但不能写入。`!` 只读排除规则优先覆盖同组正向只读规则，命中后继续按沙盒、映射或显式允许规则处理。
- 路径映射的入口或最终目标命中只读路径时，映射入口也会继承只读，不能通过映射绕过写入限制。
- 只读路径接受相对目录、`!` 排除前缀以及 `*` / `?` 通配符；与允许路径排除规则直接冲突的正向只读路径会被忽略，只读排除规则优先于正向只读规则。
- 普通应用使用默认 mount namespace 方案时，通配符规则会先退化为已存在的具体匹配目录；没有具体匹配时再退化到最近的具体父目录。该退化会尽量避免整条规则失效，但允许规则可能变宽，排除、沙盒和只读规则可能变严。需要严格按 `!`、`*`、`?` 精确匹配时，请开启 FUSE daemon 重定向；开启后只在通配规则前缀挂载 FUSE，普通路径继续使用 mount namespace。
- 只读 mount 会在允许路径、排除路径和显式映射之后应用，确保只读层尽量成为最终生效层。

### FuseFixer-compatible 保护

SRX 内置 FUSE 兼容保护由 `global.json` 中的 `fuse_fix_enabled` 控制。它主要用于减少 MediaProvider/FUSE 路径检查中默认可忽略 Unicode 码点带来的访问异常。

```json
{
  "fuse_fix_enabled": true
}
```

使用建议：

- 默认保持开启。
- 该能力由 SRX 内置实现，和 MediaProvider/FUSE 调用方识别 hook 并行工作。

### 文件监控 `file_monitor_enabled`

`file_monitor_enabled=true` 会启用文件创建监控，便于观察已启用应用和系统代写链路的文件操作、发起应用与最终落地路径。普通应用自身写入由 `srx_daemon` 通过 inotify 在进程外采集，不给普通应用安装 monitor/PLT hook；这是为了避免普通应用因 native/图形/加固运行时兼容问题出现无法打开或闪退。系统代写链路只在真实 MediaProvider/FUSE 服务端保留进程内 hook，以便识别 MediaProvider/FUSE 的真实调用方；DownloadProvider、ExternalStorageProvider、MTP 等桥接进程交由 daemon 和 MediaProvider 侧记录兜底。

```json
{
  "file_monitor_enabled": true
}
```

默认普通调试日志较少；需要更详细日志和采集脚本时，在设置页“模块设置”中打开“详细日志”，定位结束后关闭即可。

### 配置模板

配置模板用于复用一整份应用配置，内容等同于 `apps/<包名>.json` 的 `users` 配置集合。模板保存在：

```text
/data/adb/modules/storage.redirect.x/config/templates.json
```

使用入口：

- 应用配置页：点击右上角“配置模板”按钮，可以把当前应用配置保存为一个命名模板，也可以选择已有模板覆盖当前应用配置。
- 应用列表页：长按应用进入多选模式；管理 App 会用底部批量操作栏临时替代底部导航，WebUI 会在底部导航上方显示批量操作栏。选择多个应用后点击“应用模板”，选择已有模板并确认后会批量覆盖这些应用的配置。
- 设置页：在“配置模板”区域可以查看已添加的模板、直接添加空模板、重命名模板或删除模板。

批量应用模板会先写入临时暂存目录，再一次性提交到对应的 `apps/<包名>.json` 并触发一次配置热重载，避免大量应用逐个写入时重复触发热重载。执行期间 WebUI 和管理 App 会显示等待动画并阻止其它操作；WebUI 还会显示批量写入进度。模板覆盖的是应用配置本身，不会合并到旧配置；覆盖后如目标应用已经在运行，仍建议重启目标应用或相关媒体进程，确保进程完全使用新配置。

### WebUI / 管理 App 备份还原

设置页的“备份还原”区域可以导出一个 `.srxbak.zip` 单文件备份。备份包是通用 ZIP 格式，内部固定包含 `backup.json`，内容覆盖 `global.json`、`templates.json` 和所有 `apps/*.json` 应用配置；管理 App 导出的备份还会包含预测性返回手势这类本地 UI 偏好。WebUI 和管理 App 使用同一套备份格式，导出的备份可以相互还原。备份文件带有模块标识、格式版本和完整性校验，还原时会先校验文件格式、模块 id、配置字段和校验值，避免把其它模块或损坏文件误写入配置目录。旧版 `.srxbak.json` 备份仍可导入。

使用建议：

- 迁移设备或升级前，先点“备份”导出备份文件。
- 还原时点“还原”，通过管理器或系统文件选择器选择 `.srxbak.zip`；旧 `.srxbak.json` 也可以选择还原。
- 还原会覆盖当前全局设置、配置模板和全部应用配置；管理 App 还原时也会恢复备份中的预测性返回手势偏好。还原后建议重启已配置应用、相关媒体进程或设备，让已运行进程完全使用新配置。
- 备份只包含 SRX 配置和少量管理界面偏好，不包含应用私有文件、日志、模块二进制或临时运行状态。
- 需要分享排障材料时使用设置页右上角的文件图标导出“日志包”，不要用备份文件代替日志包。日志包会导出模块日志、轮转历史、运行状态和最近 logcat。

## 路径规则

所有配置路径都遵循以下规则：

- 只能使用相对路径，例如 `Download/MyApp`。
- 不能以 `/` 开头。
- 不能包含 `..`。
- 不能包含控制字符，单条路径长度不能超过 512 字符。
- 不能直接写 `sdcard`、`storage/emulated`、`storage/self/primary`、`data/media` 等存储根或根别名。
- 空路径会被忽略。
- `path_mappings` 的源路径和目标路径不能相同，相同会被忽略。
- `path_mappings` 不能形成循环映射，映射链深度超过 10 层的源规则会被忽略。
- `allowed_real_paths` 支持 `!` 前缀表示排除规则。
- `excluded_real_paths` 是旧版兼容字段，新配置请优先在 `allowed_real_paths` 中使用 `!` 前缀。
- `path_mappings` 和 `sandboxed_paths` 不支持 `!` 排除前缀。
- `read_only_paths` 支持 `!` 前缀表示只读排除，也支持 `*`、`?` 通配符。

`allowed_real_paths` 排除规则示例：

```json
{
  "allowed_real_paths": [
    "Pictures",
    "!Pictures/Private",
    "Download",
    "!Download/*.tmp",
    "!Download/*.part"
  ]
}
```

同一应用下，排除规则优先于普通放行规则。也就是说，先允许 `Pictures`，再排除 `Pictures/Private`，最终 `Pictures/Private` 仍会进入应用沙盒。

普通应用使用默认 mount namespace 方案时，通配符规则会先退化为已存在的具体匹配目录；没有匹配目录时再退化到最近的具体父目录，避免整条规则完全失效。该退化可能让允许规则范围变宽，也可能让排除、沙盒和只读规则范围变严。需要严格按 `!`、`*`、`?` 精确匹配时，请开启 FUSE daemon 重定向；开启后只在通配规则前缀挂载 FUSE，普通路径继续使用 mount namespace。

通配符说明：

| 规则 | 说明 |
| --- | --- |
| `!` 前缀 | 在 `allowed_real_paths` 规则前加 `!` 表示排除，例如 `!Pictures/Private`。 |
| `*` | 匹配任意字符，例如 `!Download/*.tmp`。 |
| `?` | 匹配单个字符，例如 `!DCIM/Camera/IMG_????.jpg`。 |

## 紧急恢复

如果模块导致设备 bootloop，可创建 `disable` 文件禁用模块：

```bash
# 通过 adb（如果可用）
adb shell "su -c 'touch /data/adb/modules/storage.redirect.x/disable'"
adb reboot

# 通过 Termux（设备上）
su -c 'touch /data/adb/modules/storage.redirect.x/disable && reboot'
```

更多日志和崩溃排查见 [排障说明](docs/troubleshooting.md)。

## 开发文档

- [构建环境](docs/build-environment.md)
- [构建流程](docs/build-process.md)
- [模块打包](docs/module-packaging.md)
- [运行时配置说明](docs/runtime-configuration.md)
- [上游 Hook 依赖说明](docs/upstream-hook-dependencies.md)
- [设备侧测试说明](docs/device-testing.md)

## 测试流

设备侧回归测试 APP 已集成在 `tests/storage-redirect-test/`，场景脚本位于 `.github/tests/`。公开仓库的 PR、CI Build 和 Release workflow 会运行测试流门禁；CI/Release 会先构建一次 x86_64 测试模块和测试 APK，再在 Android 13/14/15/16 模拟器上按 5 个场景组并行执行 scenario 1-29，全部场景通过后才会发布 CI 资产、更新 `update.json` 或创建正式 Release。合并 PR 时建议把 `Test-flow required gate` 配成必需检查。本地需要预检或复现时，可运行 `scripts/verify-test-flow.sh`，Windows PowerShell 环境可运行 `scripts/verify-test-flow.ps1`。详见 [设备侧测试说明](docs/device-testing.md)。
