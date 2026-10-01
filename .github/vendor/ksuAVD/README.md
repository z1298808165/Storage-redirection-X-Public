# 内置第三方脚本：KernelSU-AVD

本目录存放 CI 测试流使用的第三方 root 工具，用途是给 Android Studio 虚拟设备（AVD）
打上 KernelSU，替代 rootAVD + Magisk 的 root 方案。

## 来源

| 项目 | 内容 |
| --- | --- |
| 上游仓库 | <https://github.com/HChenX/KernelSU-AVD> |
| 引入提交 | `e11d7cab89ed74d5226b353c823ff23145a17fb3`（`fix: improve POSIX sh compatibility and sanitize SDK paths`，2026-09-30） |
| 引入文件 | `ksuAVD.sh`（原样内置，未改写） |
| 许可证 | GPL-3.0-or-later，与上游仓库 `LICENSE` 一致；GPL 正文见仓库根目录 [COPYING](../../../COPYING) |

上游脚本头部保留了项目与仓库信息，本目录不再重复 GPL 全文：仓库根目录的 `COPYING`
已是同一版本 GPL 正文，可满足同等协议义务。

## 工作方式

1. 宿主机侧把 `KernelSU.apk`、`ksuAVD.sh` 和目标 `ramdisk.img` 推送到运行中的模拟器
   `/data/local/tmp/ksuAVD/`。
2. 模拟器内从 APK 解出 `ksud`，用 `ksud boot-info current-kmi` 查询内核 KMI。
3. `ksud boot-patch -b ramdisk.img --ramdisk --kmi <KMI> --allow-shell` 注入与 KMI
   匹配的 KernelSU LKM，`--allow-shell` 让 adb shell 可以直接拿到 root。
4. 宿主机侧把打好的 `ramdiskpatched4AVD.img` 拉回来覆盖原 `ramdisk.img`，并保留
   `ramdisk.img.backup`；随后安装 KernelSU 管理器 APK。

因此该工具**需要一台已经在跑的模拟器**，且模拟器需要能访问网络以下载与 KMI 匹配的
LKM。CI 流程与 rootAVD 一致：先由 `android-emulator-runner` 启动模拟器，打完补丁后
关掉模拟器再用 patched ramdisk 冷启动。

## 与 rootAVD 的差异

- rootAVD 注入 Magisk（自带 Zygisk）；KernelSU 不带 Zygisk，因此 SRX 这类 Zygisk 模块
  必须另外安装独立的 Zygisk 实现（CI 使用 Zygisk Next，module id `zygisksu`）。
- rootAVD 依赖 Magisk APK 版本；KernelSU-AVD 依赖模拟器内核的 KMI 是否有对应 LKM。
  Android 17（API 37）模拟器内核为 `android16-6.12`，对应 LKM 为
  `x86_64-android16-6.12_kernelsu.ko`。
- **间歇性 sepolicy 现象（不要当成版本能力差异）**：run `36747429134`（官方 v3.3.0 / 32601）
  出现过 `Unable to apply SELinux patches! Your kernel may not support SELinux patch fully`，随后
  系统进程写 hook 状态被 `avc: denied { setattr }` 拦掉，场景 2 的 MediaProvider 代写缺失。
  但 run `36793549753` 用**同一个官方版本**、`selinux_permissive=0`，该告警不再出现且场景 2 通过，
  说明这是间歇现象而非版本缺失。ksud 的 `sepolicy` 子命令走内核接口
  （`ksucalls::set_sepolicy`），无法在用户态补救；遇到该告警时**先重跑对照**再判断，
  不要据此换内核实现或给上游提 issue。官方 v3.3.0 与 KernelSU-Next v3.4.0 均实测可用。

## 升级注意事项

- 更新时整文件替换 `ksuAVD.sh`，同步更新本文件的「引入提交」，并保持脚本头部注释完整。
- 上游为 2026-09-30 首次发布的单日新项目，API 与参数可能变动；升级后必须先在实验
  workflow `.github/workflows/ci-kernelsu-a17.yml` 上验证，再考虑进入正式测试矩阵。
