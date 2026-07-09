package org.srx.manager.root

import java.io.BufferedReader
import java.io.Closeable
import java.io.IOException
import java.io.InputStreamReader
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.withContext

data class ShellResult(
    val code: Int,
    val stdout: String,
    val stderr: String,
) {
  val isSuccess: Boolean
    get() = code == 0
}

interface ShellExecutor {
  suspend fun exec(
      command: String,
      timeoutMs: Long = 120_000L,
      mountMaster: Boolean = false,
  ): ShellResult
}

class RootShell : Closeable, ShellExecutor {
  suspend fun checkRoot(): Boolean =
      withContext(Dispatchers.IO) {
        val result = runCatching { exec("id") }.getOrNull()
        result?.isSuccess == true && result.stdout.contains("uid=0")
      }

  override suspend fun exec(
      command: String,
      timeoutMs: Long,
      mountMaster: Boolean,
  ): ShellResult =
      withContext(Dispatchers.IO) {
        val args =
            if (mountMaster) {
              listOf("su", "-M", "-c", command)
            } else {
              listOf("su", "-c", command)
            }
        val proc =
            try {
              ProcessBuilder(args).redirectErrorStream(false).start()
            } catch (canceled: CancellationException) {
              throw canceled
            } catch (error: SecurityException) {
              return@withContext ShellResult(126, "", error.message ?: "su permission denied")
            } catch (error: IOException) {
              return@withContext ShellResult(126, "", error.message ?: "su unavailable")
            }

        val outReader = BufferedReader(InputStreamReader(proc.inputStream))
        val errReader = BufferedReader(InputStreamReader(proc.errorStream))
        val stdoutJob = async(Dispatchers.IO) { outReader.readRemaining() }
        val stderrJob = async(Dispatchers.IO) { errReader.readRemaining() }
        val completed = proc.waitFor(timeoutMs, TimeUnit.MILLISECONDS)
        if (!completed) {
          proc.destroyForcibly()
          proc.waitFor(1, TimeUnit.SECONDS)
          val stderrText = stderrJob.await()
          val timeoutText =
              if (stderrText.isBlank()) {
                "Command timed out"
              } else {
                "$stderrText\nCommand timed out"
              }
          return@withContext ShellResult(124, stdoutJob.await(), timeoutText)
        }
        ShellResult(proc.exitValue(), stdoutJob.await(), stderrJob.await())
      }

  override fun close() = Unit
}

private fun BufferedReader.readRemaining(): String =
    buildString {
          while (true) {
            val line = readLine() ?: break
            append(line).append('\n')
          }
        }
        .trim()
