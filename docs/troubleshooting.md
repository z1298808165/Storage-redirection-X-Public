# 排障说明

本文汇总运行时日志、关键标记和常见崩溃原因。设备侧验证流程请看 [设备侧测试说明](device-testing.md)。

## 查看模块日志

详细日志开启时，native 日志优先写入 `/data/adb/modules/storage.redirect.x/logs/running.log`，文件监视记录写入 `file_monitor.log`。`adb logcat -s "SRX" "StorageRedirect"` 主要用于查看 Java 日志、native Warn/Error 镜像和私有通道不可用时的文件监视回退，不再是完整 native 日志来源。

也可以在 WebUI 或管理 App 的设置页右上角点击文件图标导出日志包。导出的 `storage-redirect-x-logs-*.tar.gz` 会包含模块日志、轮转历史、关键配置、运行状态快照、最近相关 logcat 和 dmesg tail，适合在 Issue 或排障沟通中直接提供。

设备侧日志目录：

```text
/data/adb/modules/storage.redirect.x/logs/
```

主要日志会按大小轮转并保留 `.1`、`.2` 历史，例如 `file_monitor.log`、`file_monitor.log.1`、`file_monitor.log.2`。`running.log`、`file_monitor.log` 和 `stats` 由 `srx_daemon` 私有日志接收器统一写入，默认不需要常驻 `logcat` 采集器。

日志包会在导出一开始保存分层 logcat 快照，并在末尾补抓导出期间日志。`-t 10000` 等参数表示行数而不是秒数；可查看包内 `state/logcat-capture.txt` 判断实际截取时间。为控制体积，main/system 只保留 8000 行上下文，相关标签保留 10000 行，events 保留 1500 行；只有独立且通常较小的 crash 环形缓冲区会完整导出。

## 关键日志标记

| 日志 | 含义 |
|------|------|
| `java hook init ok` | LSPlant + Hooker.dex 初始化成功 |
| `java hook init failed` | Java hook 初始化失败（检查 dex 是否完整） |
| `java hook query ok` | MediaProvider.query() hook 安装成功 |
| `hook_redirect=true` | 进程启用了 hook 重定向模式 |
| `row filter reason=hide_nonexist` | cursor 行因目标不存在被隐藏 |
| `row filter reason=hide_probe_nonexist` | MediaStore 相册桶/relative_path 探针被隐藏；用于避免缩略图、计数、聚合相册漏出 |
| `row rewrite reason=rewrite` | cursor 行路径被重写（目标存在） |
| `row rewrite reason=rewrite_pending` | cursor 行路径被重写（目标不存在，pending 记录） |

## MediaStore 查询排障

图库类 APP 看到真实相册时，不要只看目标 APP 自身是否已经 mount 到沙盒。相册、缩略图和计数常来自 `com.android.providers.media.module` 的 MediaStore 查询结果。

排查顺序：

1. 确认目标 APP 配置没有显式放行：
   ```powershell
   adb shell "su -c 'cat /data/adb/modules/storage.redirect.x/config/apps/<package>.json'"
   ```
   如果 `allowed_real_paths` 里包含 `DCIM`、`Pictures`、`Android` 等路径，看到或写入这些路径是符合配置的。
2. 重启 MediaProvider 并抓日志：
   ```powershell
   adb shell "su -c 'killall -9 com.android.providers.media.module 2>/dev/null; logcat -c'"
   adb shell "am force-stop <package>"
   adb shell "monkey -p <package> -c android.intent.category.LAUNCHER 1"
   adb logcat -d -s SRX StorageRedirect
   ```
3. 正常应看到 `java hook query ok`、目标 APP UID 对应的 `java query` / `java cursor` 日志，受限相册桶应出现 `row filter reason=hide_probe_nonexist` 或 `java cursor filter...`。若只看到目标 APP 自身 `mount confirmed`，没有 MediaProvider 中目标 UID 的 query/cursor 日志，说明 MediaStore 查询链路没有完整闭环。

## 常见崩溃原因

| 症状 | 原因 | 解决 |
|------|------|------|
| MediaProvider SIGABRT | Hooker.dex 残缺（缺少内部类） | 验证 dex 大小大于等于 10KB |
| MediaProvider FUSE 零宽修复未生效 | `fuse_fix_enabled` 被关闭，或 MediaProvider 尚未重启加载最新 hook | 保持 `fuse_fix_enabled=true`，重启 MediaProvider 或设备后重新验证 |
| android.process.media SIGBUS | Java hook 安装在非 MediaProvider 的 shared UID 进程中 | 检查 `is_media_provider_package` 逻辑 |
| 模块加载后无任何日志 | NDK/LSPlant 未正确链接 | 验证 so 大小大于等于 1.8MB |

## 禁用模块

如果模块导致设备 bootloop，可创建 `disable` 文件禁用模块：

```bash
# 通过 adb（如果可用）
adb shell "su -c 'touch /data/adb/modules/storage.redirect.x/disable'"
adb reboot

# 通过 Termux（设备上）
su -c 'touch /data/adb/modules/storage.redirect.x/disable && reboot'
```
