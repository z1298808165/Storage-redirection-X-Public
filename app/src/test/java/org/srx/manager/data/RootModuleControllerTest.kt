package org.srx.manager.data

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.srx.manager.root.ShellResult
import org.srx.manager.root.shellQuote

class RootModuleControllerTest {
    @Test
    fun statusParsesKnownSrxctlOutputs() = runBlocking {
        val cases = mapOf(
            "enabled" to ModuleStatus.Enabled,
            "disabled" to ModuleStatus.Disabled,
            "reboot_required" to ModuleStatus.RebootRequired,
            "something_else" to ModuleStatus.Unknown,
        )

        cases.forEach { (stdout, expected) ->
            val shell = CapturingShell(ShellResult(0, stdout, ""))
            val controller = RootModuleController(shell)

            assertEquals(expected, controller.status())
        }
    }

    @Test
    fun statusKeepsSrxctlPreferredFallbackCommand() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "enabled", ""))
        val controller = RootModuleController(shell)

        controller.status()

        val command = shell.invocations.single().command
        assertTrue(command, command.startsWith("if [ -r ${shellQuote(SrxCtlPath)} ]; then /system/bin/sh ${shellQuote(SrxCtlPath)} status; else "))
        assertTrue(command, command.contains("cat /proc/sys/kernel/random/boot_id"))
        assertTrue(command, command.contains("echo reboot_required"))
    }

    @Test
    fun setEnabledUsesSrxctlStartWithLegacyFallback() = runBlocking {
        val shell = CapturingShell(
            ShellResult(0, "123", ""),
            ShellResult(0, "", ""),
            ShellResult(0, "456", ""),
        )
        val controller = RootModuleController(shell)

        assertTrue(controller.setEnabled(true))

        val command = shell.invocations[1].command
        assertTrue(command, command.startsWith("if [ -r ${shellQuote(SrxCtlPath)} ]; then /system/bin/sh ${shellQuote(SrxCtlPath)} start; else "))
        assertTrue(command, command.contains("rm -f ${shellQuote(RuntimeDisablePath)}"))
        assertTrue(command, command.contains("printf '{\"runtime_disabled\":false}\\n' > ${shellQuote("$ConfigDir/runtime_state.json")}"))
    }

    @Test
    fun setDisabledUsesSrxctlStopWithLegacyFallback() = runBlocking {
        val shell = CapturingShell(
            ShellResult(0, "123", ""),
            ShellResult(0, "", ""),
            ShellResult(0, "456", ""),
        )
        val controller = RootModuleController(shell)

        assertTrue(controller.setEnabled(false))

        val command = shell.invocations[1].command
        assertTrue(command, command.startsWith("if [ -r ${shellQuote(SrxCtlPath)} ]; then /system/bin/sh ${shellQuote(SrxCtlPath)} stop; else "))
        assertTrue(command, command.contains("touch ${shellQuote(RuntimeDisablePath)}"))
        assertTrue(command, command.contains("printf '{\"runtime_disabled\":true}\\n' > ${shellQuote("$ConfigDir/runtime_state.json")}"))
    }

    @Test
    fun restartMediaProviderUsesSrxctlRestartMediaWithLegacyFallback() = runBlocking {
        val shell = CapturingShell(
            ShellResult(0, "123", ""),
            ShellResult(0, "", ""),
            ShellResult(0, "456", ""),
        )
        val controller = RootModuleController(shell)

        assertTrue(controller.restartMediaProvider())

        val command = shell.invocations[1].command
        assertTrue(command, command.startsWith("if [ -r ${shellQuote(SrxCtlPath)} ]; then /system/bin/sh ${shellQuote(SrxCtlPath)} restart-media; else "))
        assertTrue(command, command.contains("pidof \"\$p\""))
        assertTrue(command, command.contains("content query --uri content://media/external/file"))
        assertTrue(command, command.contains("content query --uri content://media/internal/file"))
    }

    @Test
    fun ensureLogCollectorsUsesSrxctlWithLegacyFallback() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "", ""))
        val controller = RootModuleController(shell)

        assertTrue(controller.ensureLogCollectors())

        val command = shell.invocations.single().command
        assertTrue(command, command.startsWith("if [ -r ${shellQuote(SrxCtlPath)} ]; then /system/bin/sh ${shellQuote(SrxCtlPath)} ensure-collectors; else "))
        assertTrue(command, command.contains("/system/bin/sh ${shellQuote("$ModuleDir/service.sh")}"))
    }

    @Test
    fun versionReadsModulePropVersion() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "1.2.3", ""))
        val controller = RootModuleController(shell)

        assertEquals("1.2.3", controller.version())
        assertEquals(
            "sed -n 's/^version=//p' ${shellQuote("$ModuleDir/module.prop")} 2>/dev/null | head -n 1",
            shell.invocations.single().command,
        )
    }
}
