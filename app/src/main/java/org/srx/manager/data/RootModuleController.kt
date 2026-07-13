package org.srx.manager.data

import kotlinx.coroutines.delay
import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.shellQuote

class RootModuleController(
    private val shell: ShellExecutor,
) {
  suspend fun status(): ModuleStatus {
    val out = shell.exec(buildStatusCommand()).stdout.trim()
    return when (out) {
      "enabled" -> ModuleStatus.Enabled
      "disabled" -> ModuleStatus.Disabled
      "reboot_required" -> ModuleStatus.RebootRequired
      else -> ModuleStatus.Unknown
    }
  }

  suspend fun setEnabled(enabled: Boolean): Boolean {
    val before = mediaProviderPids()
    val result = shell.exec(buildSetEnabledCommand(enabled))
    if (!result.isSuccess) return false
    return waitForMediaProviderRestart(before, timeoutMs = 10_000L, intervalMs = 250L)
  }

  suspend fun restartMediaProvider(): Boolean {
    val before = mediaProviderPids()
    val result = shell.exec(buildRestartMediaProviderCommand())
    return result.isSuccess &&
        waitForMediaProviderRestart(before, timeoutMs = 15_000L, intervalMs = 250L)
  }

  suspend fun ensureLogCollectors(): Boolean =
      shell.exec(buildEnsureLogCollectorsCommand()).isSuccess

  suspend fun version(): String = shell.exec(readModuleVersionCommand()).stdout.trim()

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
          "for p in ${mediaProviderPackages()}; do " +
              "pids=\$(pidof \"\$p\" 2>/dev/null); for pid in \$pids; do kill -9 \"\$pid\" 2>/dev/null || true; done; done; " +
              "content query --uri content://media/external/file --projection _id --limit 1 >/dev/null 2>&1 || true; " +
              "content query --uri content://media/internal/file --projection _id --limit 1 >/dev/null 2>&1 || true",
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
