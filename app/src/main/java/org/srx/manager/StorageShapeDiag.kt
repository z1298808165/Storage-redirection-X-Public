package org.srx.manager

import android.os.StatFs
import android.system.Os
import android.system.OsConstants
import android.util.Log
import java.io.File

/**
 * 存储形态自检：在**应用自身 uid 与 SELinux 域**下采集"应用能观测到的存储事实"。
 *
 * 存在的理由：adb 侧无法复现应用视角。`su` + `nsenter` 会走 FUSE 的 fail-closed 兜底、 `nsenter` + `su <app uid>` 的
 * SELinux 域仍是 shell、正式包又不允许 `run-as`，三种口径都 会得到与真实应用不同的结果（连基线应用都会被拒）。只有应用进程自己跑，量出来的东西才
 * 能用来判断"重定向是否可被应用识破"。
 *
 * 触发方式（零 UI）：用 adb 以 `am start` 启动主界面并带上 `--es srx_diag storage_shape` 参数，随后用 `adb logcat -s
 * SRXDIAG` 读取输出即可。
 *
 * 同一台设备上把本应用分别设为"重定向"和"不重定向"各跑一次，两份输出 diff 即真实差异。
 */
object StorageShapeDiag {
  private const val TAG = "SRXDIAG"
  private const val VIEW_ROOT = "/storage/emulated"
  private const val USER_ROOT = "/storage/emulated/0"

  fun run() {
    Log.w(TAG, "==== 存储形态自检开始 uid=${Os.getuid()} pid=${Os.getpid()} ====")
    dumpMountInfo()
    dumpAttr(VIEW_ROOT)
    dumpAttr(USER_ROOT)
    dumpEntries(VIEW_ROOT, 8)
    dumpEntries(USER_ROOT, 12)
    dumpStatFs(USER_ROOT)
    probe("obb", "/storage/emulated/obb")
    probe("999", "/storage/emulated/999")
    // 逐级探测"能否以应用身份打开"，用于区分两种失败：路径权限问题（应用也打不开）
    // 还是挂载工作进程的凭据问题（应用能打开、工作进程打不开）。
    probeOpen("android", "/storage/emulated/0/Android")
    probeOpen("android_media", "/storage/emulated/0/Android/media")
    probeOpen("own_media", "/storage/emulated/0/Android/media/org.srx.manager")
    probeOpen("wechat_media", "/storage/emulated/0/Android/media/com.tencent.mm")
    Log.w(TAG, "==== 存储形态自检结束 ====")
  }

  /** 只打印与存储相关的挂载行，避免把整份 mountinfo 灌进 logcat。 */
  private fun dumpMountInfo() {
    val lines =
        runCatching { File("/proc/self/mountinfo").readLines() }
            .getOrElse {
              Log.w(TAG, "mountinfo 读取失败 ${it.message}")
              return
            }
            .filter { it.contains(" /storage") }
    Log.w(TAG, "mountinfo.storage.lines=${lines.size}")
    lines.forEach { line ->
      val fields = line.split(' ')
      val mountPoint = fields.getOrNull(4) ?: "-"
      val dev = fields.getOrNull(2) ?: "-"
      val root = fields.getOrNull(3) ?: "-"
      val fsType = fields.getOrNull(fields.indexOf("-") + 1) ?: "-"
      val master = fields.firstOrNull { it.startsWith("master:") } ?: "master=-"
      Log.w(TAG, "mount point=$mountPoint dev=$dev root=$root fs=$fsType $master")
    }
  }

  /** 属性用 `Os.lstat` 取原始 st_mode/st_uid/st_gid/st_ino/st_dev，不做任何加工。 */
  private fun dumpAttr(path: String) {
    runCatching { Os.lstat(path) }
        .onSuccess { st ->
          val isDir = (st.st_mode and OsConstants.S_IFMT) == OsConstants.S_IFDIR
          Log.w(
              TAG,
              "attr path=$path exists=true dir=$isDir perm=${(st.st_mode and 0xFFF).toString(8)} " +
                  "uid=${st.st_uid} gid=${st.st_gid} ino=${st.st_ino} dev=${st.st_dev}",
          )
        }
        .onFailure {
          Log.w(TAG, "attr path=$path exists=false err=${it.javaClass.simpleName}:${it.message}")
        }
  }

  private fun dumpEntries(path: String, limit: Int) {
    val names =
        runCatching { File(path).list()?.sorted() }
            .getOrElse {
              Log.w(TAG, "list path=$path 失败 ${it.javaClass.simpleName}:${it.message}")
              return
            }
            ?: run {
              Log.w(TAG, "list path=$path 返回 null")
              return
            }
    Log.w(TAG, "list path=$path count=${names.size} head=${names.take(limit)}")
  }

  private fun dumpStatFs(path: String) {
    runCatching { StatFs(path) }
        .onSuccess { fs ->
          Log.w(
              TAG,
              "statfs path=$path blockSize=${fs.blockSizeLong} blocks=${fs.blockCountLong} " +
                  "free=${fs.freeBytes} available=${fs.availableBytes}",
          )
        }
        .onFailure { Log.w(TAG, "statfs path=$path 失败 ${it.javaClass.simpleName}:${it.message}") }
  }

  /**
   * 以 `O_PATH | O_DIRECTORY` 打开指定路径，只验证"应用能不能解析到它"。
   *
   * 这一项专门用来定位恢复 bind 失败：挂载工作进程用应用 fsuid 打开同一路径拿到 EACCES， 若应用自己（真实 uid + 真实 SELinux
   * 域）能打开，说明是工作进程的凭据/命名空间问题； 若应用也打不开，说明是视图本身的属性问题。
   */
  private fun probeOpen(label: String, path: String) {
    // Android 的 `OsConstants` 不暴露 `O_PATH`/`O_DIRECTORY`（隐藏 API），这里写死
    // Linux 通用值：O_PATH=0x200000、O_DIRECTORY=0x10000、O_CLOEXEC=0x80000。
    val flags = 0x200000 or 0x10000 or 0x80000
    val result =
        runCatching {
              val fd = Os.open(path, flags, 0)
              Os.close(fd)
              "ok"
            }
            .getOrElse { "${it.javaClass.simpleName}:${it.message}" }
    Log.w(TAG, "open label=$label path=$path result=$result")
  }

  /** 子目录探针：只读列举 + 真实写一次再删，验证"能力"而不是只验证"存在"。 */
  private fun probe(label: String, path: String) {
    val file = File(path)
    val listResult = runCatching { file.list()?.size }.getOrElse { -1 }
    val probeFile = File(file, ".srx_diag_probe")
    val writeResult =
        runCatching {
              probeFile.writeText("probe")
              probeFile.delete()
              "ok"
            }
            .getOrElse { "${it.javaClass.simpleName}:${it.message}" }
    Log.w(TAG, "probe label=$label path=$path entries=$listResult write=$writeResult")
  }
}
