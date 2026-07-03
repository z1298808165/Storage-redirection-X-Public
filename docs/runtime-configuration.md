# 运行时配置说明

本文描述模块的核心配置项与路径规则。打包和编译说明请看 [构建流程](build-process.md) 与 [模块打包](module-packaging.md)。

## 全局配置

`/data/adb/modules/storage.redirect.x/config/global.json` 支持以下全局开关：

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

- `file_monitor_enabled`：启用文件创建监控；覆盖已配置的普通应用，以及 MediaProvider、DownloadProvider 等系统代写链路。普通应用即使关闭“启用重定向”，也会由 `srx_daemon` 在进程外通过 inotify 监控公共存储写入；这类公共根记录只在文件 owner uid 能明确反查到同一个包名时写入，避免误归因。启用重定向的普通应用会监控隔离根、放行路径、`read_only_paths` 和路径映射目标。普通应用不安装进程内 monitor/PLT hook；这是为了避免普通应用因 native/图形/加固运行时兼容问题出现无法打开或闪退。真实 MediaProvider/FUSE 服务端仍使用进程内 hook 保留调用方识别；DownloadProvider、ExternalStorageProvider、MTP、DocumentsUI、PhotoPicker 和厂商文件管理 UI 不再安装进程内 PLT hook，避免安装应用、设置、文件管理器和导出日志等系统存储链路被卡住。缺失、格式错误或不可读时，沿用历史默认值 `false`。
- `fuse_fix_enabled`：启用 SRX 内置的 FuseFixer 兼容保护，用于处理 MediaProvider FUSE 路径检查中的默认可忽略 Unicode 码点。缺失、格式错误或不可读时，默认值为 `true`。
- `fuse_daemon_redirect_enabled`：启用混合 FUSE 重定向增强。普通路径仍使用默认 mount namespace，只有通配规则前缀会挂载模块内 FUSE daemon；开启后 `!`、`*`、`?` 规则按路径匹配精确生效。关闭或 FUSE 启动失败时，普通应用仍使用默认 mount namespace 方案，通配符规则会退化为已存在的具体匹配目录，必要时退化到最近具体父目录。缺失、格式错误或不可读时，默认值为 `false`。
- `verbose_logging_enabled`：启用详细日志。打开后立即允许普通 `StorageRedirect` / `SRX` / `Stats` logcat 输出，并启动 `running.log`、`media_provider_state.log`、`app_status.log`、`stats` 等诊断采集；关闭后立即停止这些记录。文件监视记录由 `file_monitor_enabled` 和文件监视过滤配置单独控制。缺失、格式错误或不可读时，默认值为 `false`。
- `auto_enable_redirect_for_new_apps`：通过 Zygisk 在 `system_server` 注册系统包事件接收器；收到新的第三方用户应用安装事件并完成 PackageManager 校验后，自动为该应用生成配置。模块会维护 `/data/adb/modules/storage.redirect.x/config/auto_new_apps_baseline` 作为基线，避免升级、重启或重复事件把旧应用误判为新应用。缺失、格式错误或不可读时，默认值为 `false`。
- `auto_enable_new_apps_template_id`：新应用自动重定向启用时使用的配置模板 ID。为空时，新安装应用默认只开启重定向，不附加允许路径、沙盒路径或映射规则。若模板文件被外部修改导致该 ID 不再可用，运行脚本会先按仅开启重定向生成新应用配置；APP 和 WebUI 进入设置页时会清空失效引用，并在自动配置模板状态行提示已回退。缺失、格式错误或不可读时，默认值为空字符串。
- `app_config_auto_save`：控制 WebUI 应用配置页是否在每次配置操作结束后自动保存。缺失、格式错误或不可读时，默认值为 `false`，即仍需点击保存按钮。

## 仅映射模式

每个 `user` 节点都支持 `mapping_mode_only`：

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

- `mapping_mode_only=true`：不再把整个 `/storage/emulated/<user>/` fallback 重定向到应用沙盒，只应用 `path_mappings` 中显式配置的映射；未命中映射的路径保持原样。
- `mapping_mode_only=false`、字段缺失、字段为空或不是 bool：保持旧行为，即 `enabled=true` 时继续执行完整隔离重定向，并在其上叠加 `path_mappings`。
- 该模式同样覆盖 `com.android.providers.media.module` 等系统代写进程的按调用方重定向逻辑，适合只希望媒体服务代写特定映射目录、不希望整包路径被隔离的场景。

## 局部沙盒路径

`mapping_mode_only=true` 时，默认只有 `path_mappings` 会被重定向，未命中映射的路径会保持原样。若某些应用或系统服务仍会在 `/storage/emulated/<user>/` 根目录乱创建文件或目录，可以用 `sandboxed_paths` 指定这些路径仍然进入应用沙盒。

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

- `sandboxed_paths` 支持字符串或字符串数组，例如 `"sandboxed_paths": ".xlDownload"` 或 `"sandboxed_paths": [".xlDownload"]`。
- 路径规则必须是相对存储根目录的安全路径；绝对路径、空路径、`..` 等不安全路径会被忽略。
- 映射优先级高于 `sandboxed_paths`：先匹配 `path_mappings`，未命中映射时再判断是否应进入沙盒。
- 该配置主要服务 `mapping_mode_only=true` 场景；普通完整隔离模式本来就会把未放行路径重定向到沙盒。

## 只读模式

`read_only_paths` 用于把目录保持可读但禁止写入。管理 App 和 WebUI 会把它做成独立的“只读模式”开关和路径区域；手动编辑 JSON 时也可以直接写字段：

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

- `read_only_paths` 支持字符串或字符串数组，运行时读取会按真实路径放行并禁止写入。
- 普通应用通过 mount namespace 的只读 bind mount 强制生效，不会因为只读配置额外安装 PLT hook；这是稳定性约束，避免普通应用因 native/图形/加固运行时兼容问题出现无法打开或闪退。
- 真实 MediaProvider/FUSE 服务端仍通过现有系统 writer hook 识别调用方；DownloadProvider、ExternalStorageProvider、MTP、DocumentsUI、PhotoPicker 和厂商文件管理 UI 不进入进程内 PLT hook 链路；写入只读路径会返回 `EROFS`。
- 只读正向规则会提供真实读取通道；即使没有配置 `allowed_real_paths`，应用也能读取该目录但不能写入。`!` 只读排除规则优先覆盖同组正向只读规则，命中后继续按沙盒、映射或显式允许规则处理。
- 路径映射的入口或最终目标命中只读路径时，映射入口也会继承只读，不能通过映射绕过写入限制。
- 只读路径接受相对目录、`!` 排除前缀以及 `*`、`?` 通配符；与允许路径排除规则直接冲突的正向只读路径会被忽略，只读排除规则优先于正向只读规则。

## 路径校验与映射限制

管理 App、WebUI 和 native 运行时都会对路径规则做兜底校验。手动编辑 JSON 时也应遵守以下规则：

- 只能写相对存储路径，例如 `Download/MyApp`，不能以 `/` 开头。
- 不能包含 `.`、`..` 路径段、控制字符或超过 512 字符的路径。
- 不能直接写 `sdcard`、`storage/emulated`、`storage/self/primary`、`data/media` 等存储根或根别名。
- `allowed_real_paths` 支持 `!` 排除前缀；`path_mappings` 和 `sandboxed_paths` 不支持 `!`。
- `read_only_paths` 支持 `!` 前缀表示只读排除，也支持 `*`、`?` 通配符。
- 同一路径既作为放行又作为排除时，排除规则优先保留。
- `path_mappings` 的源路径和目标路径不能相同，目标路径不能位于 `Android/data` 或 `Android/obb`，`Android/media` 目标仍然允许；不能形成循环映射，映射链深度超过 10 层的源规则会被忽略。

## 备份格式

WebUI 和管理 App 的设置页都会导出 `.srxbak.zip` 备份包。该文件是通用 ZIP 格式，内部固定包含 `backup.json`：

```text
storage-redirect-x-backup-YYYYMMDD-HHMMSS.srxbak.zip
└── backup.json
```

`backup.json` 的结构仍是 schema v2，包含模块 id、格式版本、创建时间、摘要、SHA-256 完整性校验和配置数据。配置数据包含：

- `global`：全局配置。
- `apps`：全部 `apps/<package>.json` 应用配置。
- `templates`：配置模板。
- `monitor_filters`：文件监控过滤配置。
- `ui`：管理界面本地偏好，例如预测性返回手势。

还原时会先校验 ZIP 中是否存在 `backup.json`，再校验模块 id、schema、配置字段和完整性摘要。旧版 `.srxbak.json` 文件仍可导入，用于兼容历史备份。

## 日志轮转与日志包

运行时日志位于 `/data/adb/modules/storage.redirect.x/logs/`。服务脚本会按大小轮转主要日志，默认保留 `.1`、`.2` 两级历史：

- `file_monitor.log`：默认 1 MiB。
- `running.log`：默认 2 MiB。
- `media_provider_state.log`：默认 10 MiB。
- `app_status.log`：默认 10 MiB。

设置页右上角的文件图标会导出一个 `storage-redirect-x-logs-YYYYMMDD-HHMMSS.tar.gz` 日志包，包含模块日志及轮转历史、`module.prop`、`stats`、关键配置文件、设备/进程状态快照、最近相关 logcat 和 dmesg tail。日志包用于排障分享；配置迁移应使用 `.srxbak.zip` 备份。

## 排除规则

路径过滤支持排除语法，用于在放行规则中排除特定子路径或文件。

### 语法

| 规则 | 说明 |
|------|------|
| `!` 前缀 | 在规则前加 `!` 表示排除，例如 `!Pictures/Private` |
| `*` 通配符 | 匹配任意字符（不含路径分隔符），例如 `!Download/*.tmp` |
| `?` 通配符 | 匹配单个字符，例如 `!DCIM/Camera/IMG_????.jpg` |

### 优先级

同一应用下，**排除规则优先于放行规则**。即先应用放行规则，再应用排除规则进行过滤。

### 限制

`!` 和通配符仅用于**真实路径规则**，不用于**路径映射**。

普通应用使用默认 mount namespace 方案时，内核 bind mount 不能表达通配符。运行时会先把通配规则退化为已存在的具体匹配目录；没有匹配目录时再退化到最近的具体父目录，避免整条规则完全失效。该退化可能让允许规则覆盖范围变宽，也可能让排除、沙盒和只读规则覆盖范围变严。需要严格按 `!`、`*`、`?` 精确匹配时，请开启 FUSE daemon 重定向；开启后只在通配规则前缀挂载 FUSE，普通路径继续使用 mount namespace。

### 示例

允许相册但排除私密目录：

```text
Pictures
!Pictures/Private
```

允许下载目录但排除临时文件：

```text
Download
!Download/*.tmp
!Download/*.part
```

允许相机目录但排除测试命名文件：

```text
DCIM/Camera
!DCIM/Camera/TEST_*
```
