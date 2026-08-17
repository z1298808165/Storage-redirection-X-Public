package org.srx.manager.data

import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.shellQuote

private const val StatusSectionMarker = "__SRX_STATUS__"
private const val VersionSectionMarker = "__SRX_VERSION__"

data class MediaProviderRestartResult(
    val success: Boolean,
)

class RootModuleController(
    private val shell: ShellExecutor,
) {
  suspend fun status(): ModuleStatus = parseStatus(shell.exec(buildStatusCommand()).stdout.trim())

  suspend fun setEnabled(enabled: Boolean): Boolean {
    val result = shell.exec(buildSetEnabledCommand(enabled))
    return result.isSuccess
  }

  suspend fun restartMediaProvider(): MediaProviderRestartResult {
    val result = shell.exec(buildRestartMediaProviderCommand())
    return MediaProviderRestartResult(result.isSuccess)
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
      withSrxCtlFallback("remount-running", "exit 1")

  private fun buildEnsureLogCollectorsCommand(): String =
      withSrxCtlFallback(
          "ensure-collectors",
          "if [ -r ${shellQuote(
                "$ModuleDir/service.sh",
            )} ]; then /system/bin/sh ${shellQuote("$ModuleDir/service.sh")} >/dev/null 2>&1 & fi",
      )

  private fun readModuleVersionCommand(): String =
      "sed -n 's/^version=//p' ${shellQuote("$ModuleDir/module.prop")} 2>/dev/null | head -n 1"

  private fun withSrxCtlFallback(
      action: String,
      fallback: String,
  ): String =
      "if [ -r ${shellQuote(SrxCtlPath)} ]; then /system/bin/sh ${shellQuote(SrxCtlPath)} $action; else $fallback; fi"
}
