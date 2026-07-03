package org.srx.manager.data

import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.ShellResult

internal class CapturingShell(
    vararg results: ShellResult,
) : ShellExecutor {
    private val pendingResults = results.toMutableList()
    val invocations = mutableListOf<ShellInvocation>()

    override suspend fun exec(command: String, timeoutMs: Long, mountMaster: Boolean): ShellResult {
        invocations += ShellInvocation(command, timeoutMs, mountMaster)
        return if (pendingResults.isEmpty()) {
            ShellResult(0, "", "")
        } else {
            pendingResults.removeAt(0)
        }
    }
}

internal data class ShellInvocation(
    val command: String,
    val timeoutMs: Long,
    val mountMaster: Boolean,
)
