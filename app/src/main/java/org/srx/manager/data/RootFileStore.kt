package org.srx.manager.data

import kotlinx.coroutines.delay
import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.base64Utf8
import org.srx.manager.root.shellQuote

data class DiagnosticArchiveProgress(
    val percent: Int,
    val phase: String,
    val message: String,
)

class RootFileStore(
    private val shell: ShellExecutor,
) {
  suspend fun read(path: String): String = shell.exec("cat ${shellQuote(path)} 2>/dev/null").stdout

  suspend fun readTail(path: String, lines: Int): String {
    val safeLines = lines.coerceIn(1, MaxTailLines)
    return shell.exec("tail -n $safeLines ${shellQuote(path)} 2>/dev/null").stdout
  }

  suspend fun readConfiguredAppConfigDump(): String =
      shell
          .exec(
              "mkdir -p ${shellQuote(AppsDir)}; " +
                  "for f in ${shellQuote(AppsDir)}/*.json; do " +
                  "[ -f \"\$f\" ] || continue; b=\"\${f##*/}\"; p=\"\${b%.json}\"; " +
                  "printf '\\n$ConfiguredAppConfigMarker%s\\n' \"\$p\"; cat \"\$f\"; printf '\\n'; done 2>/dev/null"
          )
          .stdout

  suspend fun write(path: String, content: String, touchAfter: Boolean = false): Boolean {
    if (!isManagedWritePath(path)) return false
    val targetDir = path.substringBeforeLast('/', ConfigDir)
    val encoded = base64Utf8(content)
    val touchCommand = if (touchAfter) " && ${touchConfigCommand()}" else ""
    val result =
        shell.exec(
            "target=${shellQuote(path)}; target_dir=${shellQuote(targetDir)}; apps=${shellQuote(AppsDir)}; " +
                "tmp=\$(mktemp /data/local/tmp/srx_manager.XXXXXX) || exit 1; " +
                "cleanup() { rm -f \"\$tmp\"; }; trap cleanup EXIT; " +
                "mkdir -p \"\$target_dir\" \"\$apps\" && " +
                "printf %s ${shellQuote(encoded)} | base64 -d > \"\$tmp\" && " +
                "chmod 644 \"\$tmp\" && mv \"\$tmp\" \"\$target\"$touchCommand; " +
                "rc=\$?; trap - EXIT; [ \$rc -eq 0 ] || rm -f \"\$tmp\"; exit \$rc"
        )
    return result.isSuccess
  }

  suspend fun writeConfig(path: String, content: String): Boolean =
      write(path, content, touchAfter = true)

  suspend fun deleteConfig(path: String): Boolean {
    if (!isManagedAppConfigPath(path)) return false
    val result =
        shell.exec(
            "mkdir -p ${shellQuote(AppsDir)} && " +
                "rm -f ${shellQuote(path)} && " +
                touchConfigCommand()
        )
    return result.isSuccess
  }

  suspend fun removeFile(path: String) {
    if (!isManagedTempPath(path)) return
    shell.exec("rm -f ${shellQuote(path)}")
  }

  suspend fun removeTree(vararg paths: String) {
    if (paths.isEmpty()) return
    if (paths.any { !isManagedTempPath(it) }) return
    shell.exec("rm -rf ${paths.joinToString(" ") { shellQuote(it) }}")
  }

  suspend fun prepareCleanDir(path: String): Boolean {
    if (!isManagedTempPath(path)) return false
    val result = shell.exec("rm -rf ${shellQuote(path)}; mkdir -p ${shellQuote(path)}")
    return result.isSuccess
  }

  suspend fun publishStagedAppConfigs(stage: String): Boolean {
    if (!isManagedTempPath(stage)) return false
    val result =
        shell.exec(
            "stage=${shellQuote(stage)}; apps=${shellQuote(AppsDir)}; global=${shellQuote(GlobalConfigPath)}; " +
                "mkdir -p \"\$apps\" || exit 1; " +
                "for f in \"\$stage\"/*.json; do [ -f \"\$f\" ] || continue; cp \"\$f\" \"\$apps/\${f##*/}\" || exit 1; done; " +
                "chmod 755 \"\$apps\" || exit 1; chmod 644 \"\$apps\"/*.json 2>/dev/null || true; " +
                "touch \"\$apps\" \"\$global\" || exit 1; rm -rf \"\$stage\""
        )
    return result.isSuccess
  }

  suspend fun restoreConfigStage(stage: String, rollback: String): Boolean {
    if (!isManagedTempPath(stage) || !isManagedTempPath(rollback)) return false
    val result =
        shell.exec(
            "config=${shellQuote(ConfigDir)}; stage=${shellQuote(stage)}; rollback=${shellQuote(rollback)}; " +
                "restore_prev() { rm -rf \"\$config/apps\"; if [ -d \"\$rollback/apps\" ]; then mv \"\$rollback/apps\" \"\$config/apps\"; else mkdir -p \"\$config/apps\"; fi; rm -f \"\$config/global.json\" \"\$config/templates.json\" \"\$config/file_monitor_filters.json\"; if [ -f \"\$rollback/global.json\" ]; then mv \"\$rollback/global.json\" \"\$config/global.json\"; fi; if [ -f \"\$rollback/templates.json\" ]; then mv \"\$rollback/templates.json\" \"\$config/templates.json\"; fi; if [ -f \"\$rollback/file_monitor_filters.json\" ]; then mv \"\$rollback/file_monitor_filters.json\" \"\$config/file_monitor_filters.json\"; fi; }; " +
                "fail_restore() { rc=\$?; restore_prev; rm -rf \"\$stage\" \"\$rollback\"; exit \$rc; }; " +
                "mkdir -p \"\$config\" \"\$rollback\" || exit 1; " +
                "if [ -d \"\$config/apps\" ]; then mv \"\$config/apps\" \"\$rollback/apps\" || fail_restore; fi; " +
                "if [ -f \"\$config/global.json\" ]; then mv \"\$config/global.json\" \"\$rollback/global.json\" || fail_restore; fi; " +
                "if [ -f \"\$config/templates.json\" ]; then mv \"\$config/templates.json\" \"\$rollback/templates.json\" || fail_restore; fi; " +
                "if [ -f \"\$config/file_monitor_filters.json\" ]; then mv \"\$config/file_monitor_filters.json\" \"\$rollback/file_monitor_filters.json\" || fail_restore; fi; " +
                "mv \"\$stage/apps\" \"\$config/apps\" || fail_restore; mv \"\$stage/global.json\" \"\$config/global.json\" || fail_restore; mv \"\$stage/templates.json\" \"\$config/templates.json\" || fail_restore; mv \"\$stage/file_monitor_filters.json\" \"\$config/file_monitor_filters.json\" || fail_restore; " +
                "chmod 755 \"\$config\" \"\$config/apps\" || fail_restore; chmod 644 \"\$config/global.json\" \"\$config/templates.json\" \"\$config/file_monitor_filters.json\" || fail_restore; chmod 644 \"\$config/apps\"/*.json 2>/dev/null || true; " +
                "touch \"\$config/apps\" \"\$config/global.json\" \"\$config/templates.json\" \"\$config/file_monitor_filters.json\" || fail_restore; rm -rf \"\$rollback\" \"\$stage\""
        )
    return result.isSuccess
  }

  suspend fun clearFileMonitorLog(): Boolean {
    val result =
        shell.exec("mkdir -p ${shellQuote(LogsDir)} && : > ${shellQuote(FileMonitorLogPath)}")
    return result.isSuccess
  }

  suspend fun createDiagnosticArchive(
      onProgress: (suspend (DiagnosticArchiveProgress) -> Unit)? = null
  ): String? {
    val token = "${System.currentTimeMillis()}_${(0..99999).random()}"
    val stage = "/data/local/tmp/srx_diag_$token"
    val archive = "/data/local/tmp/srx_diag_archive_$token.tar.gz"
    if (onProgress != null) {
      return createDiagnosticArchiveWithProgress(token, stage, archive, onProgress)
    }
    val result = shell.exec(buildDiagnosticArchiveCommand(stage, archive), timeoutMs = 180_000L)
    return archive.takeIf { result.isSuccess }
  }

  private suspend fun createDiagnosticArchiveWithProgress(
      token: String,
      stage: String,
      archive: String,
      onProgress: suspend (DiagnosticArchiveProgress) -> Unit,
  ): String? {
    val progress = "/data/local/tmp/srx_diag_progress_$token"
    val done = "$progress.done"
    val runLog = "$progress.log"
    val pid = "$progress.pid"
    val worker = "$progress.worker.sh"
    var keepArchive = false
    try {
      val start =
          shell.exec(
              buildDiagnosticArchiveStartCommand(
                  stage,
                  archive,
                  progress,
                  done,
                  runLog,
                  pid,
                  worker,
              ),
              timeoutMs = 10_000L,
          )
      if (!start.isSuccess) return null

      onProgress(DiagnosticArchiveProgress(1, "start", "正在准备日志包"))
      if (!waitForDiagnosticArchive(done, progress, pid, onProgress)) return null

      onProgress(DiagnosticArchiveProgress(99, "verify", "正在确认日志包"))
      val exists =
          shell
              .exec("[ -s ${shellQuote(archive)} ] && echo 1 || echo 0", timeoutMs = 10_000L)
              .stdout
              .trim() == "1"
      keepArchive = exists
      if (exists) onProgress(DiagnosticArchiveProgress(99, "verify", "日志包已生成，正在准备写入目标文件"))
      return archive.takeIf { exists }
    } finally {
      val cleanupPaths =
          if (keepArchive) {
            arrayOf(stage, progress, "$progress.tmp", done, runLog, pid, worker)
          } else {
            arrayOf(stage, archive, progress, "$progress.tmp", done, runLog, pid, worker)
          }
      runCatching { shell.exec(managedTempCleanupCommand(*cleanupPaths), timeoutMs = 10_000L) }
    }
  }

  private suspend fun waitForDiagnosticArchive(
      done: String,
      progress: String,
      pid: String,
      onProgress: suspend (DiagnosticArchiveProgress) -> Unit,
  ): Boolean {
    val deadline = System.currentTimeMillis() + 180_000L
    var lastProgressLine = ""
    while (System.currentTimeMillis() < deadline) {
      val poll = shell.exec(buildDiagnosticArchivePollCommand(progress, done), timeoutMs = 10_000L)
      val pollState = parseDiagnosticArchivePoll(poll.stdout)
      val progressState = pollState.progress?.asArchiveBuildProgress()
      if (progressState != null && pollState.progressLine != lastProgressLine) {
        lastProgressLine = pollState.progressLine
        onProgress(progressState)
      }
      val doneCode = pollState.doneCode
      if (doneCode != null) return doneCode == 0
      delay(600)
    }

    shell.exec(
        "pid_file=${shellQuote(pid)}; if [ -f \"\$pid_file\" ]; then kill \$(cat \"\$pid_file\") 2>/dev/null || true; fi",
        timeoutMs = 10_000L,
    )
    return false
  }

  private fun buildDiagnosticArchiveCommand(
      stage: String,
      archive: String,
      progress: String? = null,
  ): String {
    val stageQ = shellQuote(stage)
    val archiveQ = shellQuote(archive)
    val progressArg = progress?.let { " ${shellQuote(it)}" }.orEmpty()
    val scriptCommand =
        "if [ -r ${shellQuote(DiagnosticArchiveScriptPath)} ]; then " +
            "/system/bin/sh ${shellQuote(DiagnosticArchiveScriptPath)} $stageQ $archiveQ$progressArg; " +
            "rc=\$?; [ \$rc -eq 0 ] && exit 0; fi; "
    return scriptCommand + buildLegacyDiagnosticArchiveCommand(stage, archive, progress)
  }

  private fun buildDiagnosticArchiveStartCommand(
      stage: String,
      archive: String,
      progress: String,
      done: String,
      runLog: String,
      pid: String,
      worker: String,
  ): String {
    val script = shellQuote(DiagnosticArchiveScriptPath)
    return managedTempCleanupCommand(
        stage,
        archive,
        progress,
        "$progress.tmp",
        done,
        runLog,
        pid,
        worker,
    ) +
        "; " +
        "stage=${shellQuote(stage)}; archive=${shellQuote(archive)}; progress=${shellQuote(progress)}; " +
        "done=${shellQuote(done)}; run_log=${shellQuote(runLog)}; pid_file=${shellQuote(pid)}; " +
        "worker=${shellQuote(worker)}; script=$script; " +
        "printf '%s|%s|%s\\n' '1' 'start' '正在启动日志导出' > \"\$progress\" 2>/dev/null || true; " +
        "cat > \"\$worker\" <<'SRX_DIAG_WORKER'\n" +
        diagnosticArchiveWorkerScript() +
        "\nSRX_DIAG_WORKER\n" +
        "chmod 700 \"\$worker\" || exit 1; " +
        "export stage archive progress done script; " +
        "if command -v setsid >/dev/null 2>&1; then " +
        "setsid /system/bin/sh \"\$worker\" > \"\$run_log\" 2>&1 < /dev/null & worker_pid=\$!; " +
        "else /system/bin/sh \"\$worker\" > \"\$run_log\" 2>&1 < /dev/null & worker_pid=\$!; fi; " +
        "printf '%s\\n' \"\$worker_pid\" > \"\$pid_file\"; exit 0"
  }

  private fun diagnosticArchiveWorkerScript(): String =
      listOf(
              "#!/system/bin/sh",
              "rc=1",
              "if [ -r \"\$script\" ]; then",
              "  /system/bin/sh \"\$script\" \"\$stage\" \"\$archive\" \"\$progress\"",
              "  rc=\$?",
              "else",
              "  echo \"diagnostic_archive: script missing: \$script\" >&2",
              "  rc=127",
              "fi",
              "printf '%s\\n' \"\$rc\" > \"\$done\"",
          )
          .joinToString("\n")

  private fun buildDiagnosticArchivePollCommand(progress: String, done: String): String =
      "progress=${shellQuote(progress)}; done=${shellQuote(done)}; " +
          "if [ -f \"\$progress\" ]; then tail -n 1 \"\$progress\"; fi; " +
          "printf '\\n__SRX_DONE__='; if [ -f \"\$done\" ]; then cat \"\$done\"; fi"

  private fun buildLegacyDiagnosticArchiveCommand(
      stage: String,
      archive: String,
      progress: String? = null,
  ): String {
    val stageQ = shellQuote(stage)
    val archiveQ = shellQuote(archive)
    return "stage=$stageQ; archive=$archiveQ; module=${shellQuote(ModuleDir)}; logs=${shellQuote(LogsDir)}; config=${shellQuote(ConfigDir)}; " +
        progressCommand(progress, 5, "legacy", "正在使用兼容模式导出日志") +
        "rm -rf \"\$stage\" \"\$archive\"; mkdir -p \"\$stage/logs\" \"\$stage/config\" \"\$stage/state\" || exit 1; " +
        progressCommand(progress, 18, "files", "正在复制模块日志和配置") +
        "if [ -d \"\$logs\" ]; then find \"\$logs\" -maxdepth 1 -type f ! -name '.*.pid' ! -name '.uid_map_last_refresh' -exec cp -p {} \"\$stage/logs/\" \\; 2>/dev/null; fi; " +
        "cp -p \"\$module/module.prop\" \"\$stage/module.prop\" 2>/dev/null || true; " +
        "cp -p \"\$module/stats\" \"\$stage/stats\" 2>/dev/null || true; " +
        "cp -p \"\$config/global.json\" \"\$stage/config/global.json\" 2>/dev/null || true; " +
        "cp -p \"\$config/file_monitor_filters.json\" \"\$stage/config/file_monitor_filters.json\" 2>/dev/null || true; " +
        "cp -p \"\$config/templates.json\" \"\$stage/config/templates.json\" 2>/dev/null || true; " +
        progressCommand(progress, 45, "state", "正在采集基础状态") +
        "{ date; id; uname -a; getprop ro.build.fingerprint 2>/dev/null; getprop ro.product.model 2>/dev/null; getprop ro.build.version.release 2>/dev/null; } > \"\$stage/state/device.txt\" 2>&1; " +
        "{ /system/bin/sh \"\$module/bin/srxctl\" status 2>/dev/null || true; ls -la \"\$module\" 2>/dev/null; ls -la \"\$logs\" 2>/dev/null; } > \"\$stage/state/module.txt\" 2>&1; " +
        "{ ps -A 2>/dev/null | grep -E 'srx|zygisk|media|storage' || true; } > \"\$stage/state/processes.txt\" 2>&1; " +
        "{ for p in com.android.providers.media.module com.google.android.providers.media.module com.android.providers.media android.process.media; do echo \"## pidof \$p\"; pidof \"\$p\" 2>/dev/null || true; done; } > \"\$stage/state/media-pids.txt\" 2>&1; " +
        progressCommand(progress, 72, "logcat", "正在截取系统日志") +
        "logcat -d -t 2000 -v threadtime -s StorageRedirect:V SRX:V FileMonitorOp:I Stats:I AndroidRuntime:E DEBUG:F libc:F > \"\$stage/logcat-threadtime.txt\" 2>&1 || true; " +
        "dmesg 2>/dev/null | tail -n 1000 > \"\$stage/dmesg-tail.txt\" 2>/dev/null || true; " +
        progressCommand(progress, 95, "archive", "正在压缩日志包") +
        "(cd \"\$stage\" && tar -czf \"\$archive\" *) || exit 1; chmod 644 \"\$archive\"; rm -rf \"\$stage\""
  }

  private fun progressCommand(
      progress: String?,
      percent: Int,
      phase: String,
      message: String,
  ): String {
    if (progress == null) return ""
    return "printf '%s|%s|%s\\n' ${shellQuote(percent.toString())} ${shellQuote(phase)} ${shellQuote(message)} > ${shellQuote(progress)} 2>/dev/null || true; "
  }

  private fun managedTempCleanupCommand(vararg paths: String): String {
    if (paths.isEmpty() || paths.any { !isManagedTempPath(it) }) {
      throw IllegalArgumentException("unsafe managed temp path")
    }
    return "rm -rf ${paths.joinToString(" ") { shellQuote(it) }}"
  }

  private data class DiagnosticArchivePoll(
      val progress: DiagnosticArchiveProgress?,
      val progressLine: String,
      val doneCode: Int?,
  )

  private fun parseDiagnosticArchivePoll(stdout: String): DiagnosticArchivePoll {
    var progressLine = ""
    var doneCode: Int? = null
    stdout.lineSequence().forEach { line ->
      if (line.startsWith("__SRX_DONE__=")) {
        doneCode = line.removePrefix("__SRX_DONE__=").trim().toIntOrNull()
      } else if ('|' in line) {
        progressLine = line.trim()
      }
    }
    return DiagnosticArchivePoll(parseDiagnosticProgress(progressLine), progressLine, doneCode)
  }

  private fun parseDiagnosticProgress(line: String): DiagnosticArchiveProgress? {
    if (line.isBlank()) return null
    val parts = line.split('|', limit = 3)
    if (parts.size < 3) return null
    val percent = parts[0].toIntOrNull()?.coerceIn(0, 100) ?: return null
    val phase = parts[1].take(32)
    val message = parts[2].take(80).ifBlank { "正在导出日志" }
    return DiagnosticArchiveProgress(percent, phase, message)
  }

  private fun DiagnosticArchiveProgress.asArchiveBuildProgress(): DiagnosticArchiveProgress {
    if (percent >= 100 && phase == "done") {
      return copy(percent = 98, message = "日志包已生成，正在准备写入目标文件")
    }
    return this
  }

  suspend fun touchConfig() {
    shell.exec(touchConfigCommand())
  }

  fun touchConfigCommand(): String =
      "mkdir -p ${shellQuote(AppsDir)} && touch ${shellQuote(AppsDir)} ${shellQuote(GlobalConfigPath)} 2>/dev/null"

  private fun isManagedAppConfigPath(path: String): Boolean {
    val clean = path.trimEnd('/')
    if (!clean.startsWith("$AppsDir/") || !clean.endsWith(".json") || clean.contains("..")) {
      return false
    }
    val fileName = clean.substringAfterLast('/')
    val packageName = fileName.removeSuffix(".json")
    return packageName.isNotBlank() && packageName.matches(SafePackageNameRegex)
  }

  private fun isManagedWritePath(path: String): Boolean {
    val clean = path.trimEnd('/')
    return !clean.contains("..") && (clean.startsWith("$ConfigDir/") || isManagedTempPath(clean))
  }

  private fun isManagedTempPath(path: String): Boolean {
    val clean = path.trimEnd('/')
    return clean.length > ManagedTempPrefix.length &&
        clean.startsWith(ManagedTempPrefix) &&
        !clean.contains("..")
  }

  private companion object {
    const val MaxTailLines = 10_000
    const val ManagedTempPrefix = "/data/local/tmp/srx_"
    val SafePackageNameRegex = Regex("^[A-Za-z0-9_.-]+$")
  }
}
