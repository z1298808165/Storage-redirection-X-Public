package org.srx.manager.data

import kotlinx.coroutines.delay
import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.shellQuote

private const val StatusSectionMarker = "__SRX_STATUS__"
private const val VersionSectionMarker = "__SRX_VERSION__"
private const val RunningAppMarker = "srx_restart_running_app="

data class MediaProviderRestartResult(
    val success: Boolean,
    val runningPackages: List<String>,
)

class RootModuleController(
    private val shell: ShellExecutor,
) {
  suspend fun status(): ModuleStatus = parseStatus(shell.exec(buildStatusCommand()).stdout.trim())

  suspend fun setEnabled(enabled: Boolean): Boolean {
    val before = mediaProviderPids()
    val result = shell.exec(buildSetEnabledCommand(enabled))
    if (!result.isSuccess) return false
    return waitForMediaProviderRestart(before, timeoutMs = 10_000L, intervalMs = 250L)
  }

  suspend fun restartMediaProvider(): MediaProviderRestartResult {
    val result = shell.exec(buildRestartMediaProviderCommand())
    val runningPackages =
        result.stdout
            .lineSequence()
            .mapNotNull { line -> line.trim().takeIf { it.startsWith(RunningAppMarker) } }
            .map { it.removePrefix(RunningAppMarker) }
            .filter { it.isNotBlank() }
            .distinct()
            .toList()
    return MediaProviderRestartResult(result.isSuccess, runningPackages)
  }

  suspend fun ensureLogCollectors(): Boolean =
      shell.exec(buildEnsureLogCollectorsCommand()).isSuccess

  suspend fun version(): String = shell.exec(readModuleVersionCommand()).stdout.trim()

  /**
   * 一次 root 调用同时读取模块状态与版本。
   *
   * 每次 [ShellExecutor.exec] 都要新建一个 su 进程，这是概览加载里最贵的单项开销。 两条命令各自放进子 shell 并以标记行分隔，因此保持原有语义与输出，只是省掉一次
   * 进程创建。
   */
  suspend fun statusAndVersion(): Pair<ModuleStatus, String> {
    val command =
        "printf '%s\\n' ${shellQuote(StatusSectionMarker)}; " +
            "( ${buildStatusCommand()} ); " +
            "printf '%s\\n' ${shellQuote(VersionSectionMarker)}; " +
            "( ${readModuleVersionCommand()} )"
    val out = shell.exec(command).stdout
    val statusText = sectionText(out, StatusSectionMarker, VersionSectionMarker)
    val versionText = sectionText(out, VersionSectionMarker, null)
    return parseStatus(statusText) to versionText
  }

  private fun sectionText(output: String, startMarker: String, endMarker: String?): String {
    val start = output.indexOf(startMarker)
    if (start < 0) return ""
    val bodyStart = start + startMarker.length
    val end = endMarker?.let { output.indexOf(it, bodyStart) }?.takeIf { it >= 0 } ?: output.length
    return output.substring(bodyStart, end).trim()
  }

  private fun parseStatus(text: String): ModuleStatus =
      when (text) {
        "enabled" -> ModuleStatus.Enabled
        "disabled" -> ModuleStatus.Disabled
        "reboot_required" -> ModuleStatus.RebootRequired
        else -> ModuleStatus.Unknown
      }

  private suspend fun mediaProviderPids(): Set<String> {
    val out = shell.exec(mediaProviderPidCommand()).stdout
    return out.split(Regex("\\s+")).filter { it.isNotBlank() }.toSet()
  }

  private suspend fun waitForMediaProviderRestart(
      before: Set<String>,
      timeoutMs: Long = 15_000L,
      intervalMs: Long = 500L,
  ): Boolean {
    val deadline = System.currentTimeMillis() + timeoutMs
    while (System.currentTimeMillis() < deadline) {
      val current = mediaProviderPids()
      if (current.isNotEmpty() && (before.isEmpty() || current.any { it !in before })) return true
      delay(intervalMs)
    }
    return false
  }

  private fun buildStatusCommand(): String =
      "if [ -d ${shellQuote(PendingModuleDir)} ]; then echo reboot_required; else " +
          withSrxCtlFallback(
              "status",
              "boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null); " +
                  "boot_ok=\$(cat ${shellQuote("$ModuleDir/.boot_ok")} 2>/dev/null); " +
                  "boot_module_version=\$(cat ${shellQuote(BootModuleVersionPath)} 2>/dev/null); " +
                  "module_version=\$(sed -n 's/^versionCode=//p; s/^version=//p' ${shellQuote("$ModuleDir/module.prop")} 2>/dev/null | tr '\\n' ' '); " +
                  "boot_marker=${shellQuote(LogsDir)}/boot_\${boot_id}.marker; " +
                  "if [ ! -d ${shellQuote(ModuleDir)} ]; then echo unknown; " +
                  "elif [ -f ${shellQuote(RuntimeDisablePath)} ] || [ -f ${shellQuote("$ModuleDir/disable")} ]; then echo disabled; " +
                  "elif [ -n \"\$module_version\" ] && [ \"\$boot_module_version\" != \"\$module_version\" ]; then echo reboot_required; " +
                  "elif [ -n \"\$boot_id\" ] && { [ \"\$boot_ok\" = \"\$boot_id\" ] || [ -f \"\$boot_marker\" ]; }; then echo enabled; " +
                  "else echo reboot_required; fi",
          ) +
          "; fi"

  private fun buildSetEnabledCommand(enabled: Boolean): String {
    val action = if (enabled) "start" else "stop"
    val runtimeDisabled = if (enabled) "false" else "true"
    val fallback =
        if (enabled) {
          "mkdir -p ${shellQuote(ConfigDir)} ${shellQuote(LogsDir)} && rm -f ${shellQuote(RuntimeDisablePath)} && "
        } else {
          "mkdir -p ${shellQuote(ConfigDir)} && touch ${shellQuote(RuntimeDisablePath)} && "
        } +
            "printf '{\"runtime_disabled\":$runtimeDisabled}\\n' > ${shellQuote("$ConfigDir/runtime_state.json")}"
    return withSrxCtlFallback(action, fallback)
  }

  private fun buildRestartMediaProviderCommand(): String =
      withSrxCtlFallback(
          "restart-media",
          "apps=${shellQuote("$ConfigDir/apps")}; " +
              "for config in \"\$apps\"/*.json; do [ -f \"\$config\" ] || continue; package=\${config##*/}; package=\${package%.json}; " +
              "case \"\$package\" in com.storage.redirect.x|com.topjohnwu.magisk|io.github.huskydg.magisk|io.github.vvb2060.magisk|me.weishu.kernelsu|me.weishu.kernelsu.next|io.github.rifsxd.ksunext|com.sukisu.ultra|me.bmax.apatch|me.garfieldhan.apatch.next|io.github.a13e300.ksuwebui|com.dergoogler.mmrl) continue;; esac; " +
              "pidof \"\$package\" >/dev/null 2>&1 && printf 'srx_restart_running_app=%s\\n' \"\$package\"; done; " +
              "previous=\$(for p in ${mediaProviderPackages()}; do pidof \"\$p\" 2>/dev/null || true; done); " +
              "for p in ${mediaProviderPackages()}; do pids=\$(pidof \"\$p\" 2>/dev/null); for pid in \$pids; do kill -9 \"\$pid\" 2>/dev/null || true; done; done; " +
              "for i in \$(seq 1 100); do boot_id=\$(cat /proc/sys/kernel/random/boot_id 2>/dev/null); ready=0; for p in ${mediaProviderPackages()}; do " +
              "pids=\$(pidof \"\$p\" 2>/dev/null); for pid in \$pids; do case \" \$previous \" in *\" \$pid \"*) ;; *) state=\$(cat ${shellQuote("$LogsDir/.media_hook_install_state")} 2>/dev/null); " +
              "printf '%s\\n' \"\$state\" | grep -Fq \"stage=init_ok pid=\$pid boot_id=\$boot_id \" && ready=1;; esac; done; done; " +
              "[ \"\$ready\" -eq 1 ] && exit 0; " +
              "if command -v timeout >/dev/null 2>&1; then timeout 1 content query --uri content://media/external/file --projection _id --limit 1 >/dev/null 2>&1 & " +
              "else content query --uri content://media/external/file --projection _id --limit 1 >/dev/null 2>&1 & fi; " +
              "if command -v timeout >/dev/null 2>&1; then timeout 1 content query --uri content://media/internal/file --projection _id --limit 1 >/dev/null 2>&1 & " +
              "else content query --uri content://media/internal/file --projection _id --limit 1 >/dev/null 2>&1 & fi; sleep 0.1; done; exit 1",
      )

  private fun buildEnsureLogCollectorsCommand(): String =
      withSrxCtlFallback(
          "ensure-collectors",
          "if [ -r ${shellQuote(
                "$ModuleDir/service.sh",
            )} ]; then /system/bin/sh ${shellQuote("$ModuleDir/service.sh")} >/dev/null 2>&1 & fi",
      )

  private fun readModuleVersionCommand(): String =
      "sed -n 's/^version=//p' ${shellQuote("$ModuleDir/module.prop")} 2>/dev/null | head -n 1"

  private fun mediaProviderPidCommand(): String =
      "for p in ${mediaProviderPackages()}; do pidof \"\$p\" 2>/dev/null || true; done"

  private fun withSrxCtlFallback(
      action: String,
      fallback: String,
  ): String =
      "if [ -r ${shellQuote(SrxCtlPath)} ]; then /system/bin/sh ${shellQuote(SrxCtlPath)} $action; else $fallback; fi"

  private fun mediaProviderPackages(): String = MediaProviderPackages.joinToString(" ")

  private companion object {
    val MediaProviderPackages =
        listOf(
            "com.android.providers.media.module",
            "com.google.android.providers.media.module",
            "com.android.providers.media",
        )
  }
}
