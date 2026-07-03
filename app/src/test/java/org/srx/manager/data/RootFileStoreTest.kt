package org.srx.manager.data

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.srx.manager.root.ShellResult
import org.srx.manager.root.shellQuote

class RootFileStoreTest {
    @Test
    fun writeConfigUsesStagedTempFileAndAtomicMove() = runBlocking {
        val shell = CapturingShell()
        val store = RootFileStore(shell)

        val ok = store.writeConfig("$ConfigDir/global.json", "hello")

        assertTrue(ok)
        val command = shell.invocations.single().command
        assertTrue(command, command.contains("tmp=\$(mktemp /data/local/tmp/srx_manager.XXXXXX) || exit 1"))
        assertTrue(command, command.contains("cleanup() { rm -f \"\$tmp\"; }; trap cleanup EXIT;"))
        assertTrue(command, command.contains("printf %s 'aGVsbG8=' | base64 -d > \"\$tmp\""))
        assertTrue(command, command.contains("chmod 644 \"\$tmp\" && mv \"\$tmp\" \"\$target\""))
        assertTrue(command, command.contains("touch ${shellQuote(AppsDir)} ${shellQuote(GlobalConfigPath)}"))
    }

    @Test
    fun writeReturnsFalseWhenShellCommandFails() = runBlocking {
        val shell = CapturingShell(ShellResult(1, "", "permission denied"))
        val store = RootFileStore(shell)

        assertFalse(store.write("$ConfigDir/global.json", "hello"))
    }

    @Test
    fun writeRejectsUnmanagedPathsWithoutShellCall() = runBlocking {
        val shell = CapturingShell()
        val store = RootFileStore(shell)

        assertFalse(store.write("/sdcard/Download/out.json", "hello"))
        assertFalse(store.write("$ConfigDir/../bad.json", "hello"))
        assertTrue(shell.invocations.isEmpty())
    }

    @Test
    fun readTailClampsLineCountAndQuotesPath() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "log", ""))
        val store = RootFileStore(shell)
        val path = "/data/local/tmp/a'b.log"

        val output = store.readTail(path, 50_000)

        assertEquals("log", output)
        assertEquals(
            "tail -n 10000 ${shellQuote(path)} 2>/dev/null",
            shell.invocations.single().command,
        )
    }

    @Test
    fun readConfiguredAppConfigDumpListsJsonFilesWithMarkers() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "dump", ""))
        val store = RootFileStore(shell)

        val output = store.readConfiguredAppConfigDump()

        assertEquals("dump", output)
        val command = shell.invocations.single().command
        assertTrue(command, command.startsWith("mkdir -p ${shellQuote(AppsDir)}; "))
        assertTrue(command, command.contains("for f in ${shellQuote(AppsDir)}/*.json; do "))
        assertTrue(command, command.contains("printf '\\n$ConfiguredAppConfigMarker%s\\n' \"\$p\"; cat \"\$f\"; printf '\\n'; done 2>/dev/null"))
    }

    @Test
    fun deleteConfigOnlyAllowsManagedAppConfigFiles() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "", ""))
        val store = RootFileStore(shell)
        val path = "$AppsDir/com.example.app.json"

        assertTrue(store.deleteConfig(path))
        assertEquals(
            "mkdir -p ${shellQuote(AppsDir)} && rm -f ${shellQuote(path)} && ${store.touchConfigCommand()}",
            shell.invocations.single().command,
        )

        assertFalse(store.deleteConfig("$ConfigDir/global.json"))
        assertFalse(store.deleteConfig("$AppsDir/../../bad.json"))
        assertFalse(store.deleteConfig("$AppsDir/not json.txt"))
        assertEquals(1, shell.invocations.size)
    }

    @Test
    fun prepareCleanDirOnlyAllowsManagedTempTrees() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "", ""))
        val store = RootFileStore(shell)

        assertTrue(store.prepareCleanDir("/data/local/tmp/srx_restore_stage_123/apps"))
        assertEquals(
            "rm -rf ${shellQuote("/data/local/tmp/srx_restore_stage_123/apps")}; mkdir -p ${shellQuote("/data/local/tmp/srx_restore_stage_123/apps")}",
            shell.invocations.single().command,
        )

        assertFalse(store.prepareCleanDir(ConfigDir))
        assertFalse(store.prepareCleanDir("/data/local/tmp/srx_"))
        assertFalse(store.prepareCleanDir("/data/local/tmp/srx_restore_stage_123/../bad"))
        assertEquals(1, shell.invocations.size)
    }

    @Test
    fun removeTreeOnlyAllowsManagedTempTrees() = runBlocking {
        val shell = CapturingShell()
        val store = RootFileStore(shell)

        store.removeTree("/data/local/tmp/srx_bulk_apps_123", "/data/local/tmp/srx_restore_rollback_123")
        assertEquals(
            "rm -rf ${shellQuote("/data/local/tmp/srx_bulk_apps_123")} ${shellQuote("/data/local/tmp/srx_restore_rollback_123")}",
            shell.invocations.single().command,
        )

        store.removeTree("/data/local/tmp/srx_bulk_apps_123", ConfigDir)
        store.removeTree("/data/local/tmp/srx_")
        store.removeTree("/data/local/tmp/srx_bulk_apps_123/../bad")
        assertEquals(1, shell.invocations.size)
    }

    @Test
    fun removeFileOnlyAllowsManagedTempPaths() = runBlocking {
        val shell = CapturingShell()
        val store = RootFileStore(shell)

        store.removeFile("/data/local/tmp/srx_diag_archive_123.tar.gz")
        assertEquals(
            "rm -f ${shellQuote("/data/local/tmp/srx_diag_archive_123.tar.gz")}",
            shell.invocations.single().command,
        )

        store.removeFile("/data/local/tmp/storage-redirect-x-logs-123.tar.gz")
        store.removeFile("$ConfigDir/global.json")
        assertEquals(1, shell.invocations.size)
    }

    @Test
    fun publishStagedAppConfigsOnlyAllowsManagedTempStage() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "", ""))
        val store = RootFileStore(shell)
        val stage = "/data/local/tmp/srx_bulk_apps_123"

        assertTrue(store.publishStagedAppConfigs(stage))
        val command = shell.invocations.single().command
        assertTrue(command, command.startsWith("stage=${shellQuote(stage)}; apps=${shellQuote(AppsDir)}; global=${shellQuote(GlobalConfigPath)}; "))
        assertTrue(command, command.contains("for f in \"\$stage\"/*.json; do"))
        assertTrue(command, command.contains("rm -rf \"\$stage\""))

        assertFalse(store.publishStagedAppConfigs(ConfigDir))
        assertFalse(store.publishStagedAppConfigs("/data/local/tmp/srx_bulk_apps_123/../bad"))
        assertEquals(1, shell.invocations.size)
    }

    @Test
    fun restoreConfigStageOnlyAllowsManagedTempTrees() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "", ""))
        val store = RootFileStore(shell)
        val stage = "/data/local/tmp/srx_restore_stage_123"
        val rollback = "/data/local/tmp/srx_restore_rollback_123"

        assertTrue(store.restoreConfigStage(stage, rollback))
        val command = shell.invocations.single().command
        assertTrue(command, command.startsWith("config=${shellQuote(ConfigDir)}; stage=${shellQuote(stage)}; rollback=${shellQuote(rollback)}; "))
        assertTrue(command, command.contains("restore_prev()"))
        assertTrue(command, command.contains("mv \"\$stage/apps\" \"\$config/apps\""))
        assertTrue(command, command.contains("rm -rf \"\$rollback\" \"\$stage\""))

        assertFalse(store.restoreConfigStage(ConfigDir, rollback))
        assertFalse(store.restoreConfigStage(stage, "/data/local/tmp/srx_restore_rollback_123/../bad"))
        assertEquals(1, shell.invocations.size)
    }

    @Test
    fun clearFileMonitorLogTruncatesConfiguredLogFile() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "", ""))
        val store = RootFileStore(shell)

        assertTrue(store.clearFileMonitorLog())

        assertEquals(
            "mkdir -p ${shellQuote(LogsDir)} && : > ${shellQuote(FileMonitorLogPath)}",
            shell.invocations.single().command,
        )
    }

    @Test
    fun touchConfigTouchesAppsDirAndGlobalConfig() = runBlocking {
        val shell = CapturingShell()
        val store = RootFileStore(shell)

        store.touchConfig()

        assertEquals(store.touchConfigCommand(), shell.invocations.single().command)
        assertEquals(
            "mkdir -p ${shellQuote(AppsDir)} && touch ${shellQuote(AppsDir)} ${shellQuote(GlobalConfigPath)} 2>/dev/null",
            store.touchConfigCommand(),
        )
    }

    @Test
    fun createDiagnosticArchiveBuildsExpectedArchiveCommand() = runBlocking {
        val shell = CapturingShell(ShellResult(0, "", ""))
        val store = RootFileStore(shell)

        val archive = store.createDiagnosticArchive()

        assertTrue(archive, archive?.startsWith("/data/local/tmp/srx_diag_archive_") == true)
        assertTrue(archive, archive?.endsWith(".tar.gz") == true)
        val invocation = shell.invocations.single()
        assertEquals(180_000L, invocation.timeoutMs)
        val command = invocation.command
        assertTrue(command, command.contains("archive=${shellQuote(archive!!)};"))
        assertTrue(command, command.startsWith("if [ -r ${shellQuote(DiagnosticArchiveScriptPath)} ]; then "))
        assertTrue(command, command.contains("/system/bin/sh ${shellQuote(DiagnosticArchiveScriptPath)}"))
        assertTrue(command, command.contains("mkdir -p \"\$stage/logs\" \"\$stage/config\" \"\$stage/state\""))
        assertTrue(command, command.contains("/system/bin/sh \"\$module/bin/srxctl\" status"))
        assertTrue(command, command.contains("logcat -d -t 2000 -v threadtime"))
        assertTrue(command, command.contains("(cd \"\$stage\" && tar -czf \"\$archive\" *)"))
    }

    @Test
    fun createDiagnosticArchiveWithProgressRunsBackgroundAndPollsProgress() = runBlocking {
        val shell = CapturingShell(
            ShellResult(0, "", ""),
            ShellResult(0, "22|device|正在采集设备和模块状态\n__SRX_DONE__=0", ""),
            ShellResult(0, "1", ""),
            ShellResult(0, "", ""),
        )
        val store = RootFileStore(shell)
        val progressEvents = mutableListOf<DiagnosticArchiveProgress>()

        val archive = store.createDiagnosticArchive { progressEvents += it }

        assertTrue(archive, archive?.startsWith("/data/local/tmp/srx_diag_archive_") == true)
        assertTrue(archive, archive?.endsWith(".tar.gz") == true)
        assertEquals(listOf(1, 22, 99, 99), progressEvents.map { it.percent })
        assertEquals("日志包已生成，正在准备写入目标文件", progressEvents.last().message)

        val startCommand = shell.invocations[0].command
        assertTrue(startCommand, startCommand.contains(DiagnosticArchiveScriptPath))
        assertTrue(startCommand, startCommand.contains("\"\$stage\" \"\$archive\" \"\$progress\""))
        assertTrue(startCommand, startCommand.contains("cat > \"\$worker\" <<'SRX_DIAG_WORKER'"))
        assertTrue(startCommand, startCommand.contains("export stage archive progress done script"))
        assertTrue(startCommand, startCommand.contains("setsid /system/bin/sh \"\$worker\""))
        assertTrue(startCommand, startCommand.contains("> \"\$run_log\" 2>&1 < /dev/null & worker_pid=\$!"))
        assertTrue(startCommand, startCommand.contains("printf '%s\\n' \"\$worker_pid\" > \"\$pid_file\"; exit 0"))

        val pollCommand = shell.invocations[1].command
        assertTrue(pollCommand, pollCommand.contains("__SRX_DONE__="))
        assertTrue(shell.invocations[2].command, shell.invocations[2].command.contains("[ -s ${shellQuote(archive!!)} ]"))
        assertTrue(shell.invocations[3].command, shell.invocations[3].command.contains("srx_diag_progress_"))
        assertTrue(shell.invocations[3].command, shell.invocations[3].command.contains(".worker.sh"))
    }

    @Test
    fun createDiagnosticArchiveWithProgressTreatsScriptDoneAsArchiveReady() = runBlocking {
        val shell = CapturingShell(
            ShellResult(0, "", ""),
            ShellResult(0, "100|done|日志包已生成\n__SRX_DONE__=0", ""),
            ShellResult(0, "1", ""),
            ShellResult(0, "", ""),
        )
        val store = RootFileStore(shell)
        val progressEvents = mutableListOf<DiagnosticArchiveProgress>()

        store.createDiagnosticArchive { progressEvents += it }

        assertEquals(listOf(1, 98, 99, 99), progressEvents.map { it.percent })
        assertEquals("日志包已生成，正在准备写入目标文件", progressEvents[1].message)
    }
}
