package org.srx.manager.root

import java.io.BufferedReader
import java.io.Closeable
import java.io.IOException
import java.io.InputStreamReader
import java.util.concurrent.Callable
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
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
      stdin: ByteArray? = null,
  ): ShellResult
}

internal fun interface RootProcessStarter {
  fun start(args: List<String>): Process
}

class RootShell() : Closeable, ShellExecutor {
  private var processStarter = RootProcessStarter { args ->
    ProcessBuilder(args).redirectErrorStream(false).start()
  }

  internal constructor(processStarter: RootProcessStarter) : this() {
    this.processStarter = processStarter
  }

  suspend fun checkRoot(): Boolean =
      withContext(Dispatchers.IO) {
        val result = runCatching { exec("id") }.getOrNull()
        result?.isSuccess == true && result.stdout.contains("uid=0")
      }

  override suspend fun exec(
      command: String,
      timeoutMs: Long,
      mountMaster: Boolean,
      stdin: ByteArray?,
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
              processStarter.start(args)
            } catch (canceled: CancellationException) {
              throw canceled
            } catch (error: SecurityException) {
              return@withContext ShellResult(126, "", error.message ?: "su 权限被拒绝")
            } catch (error: IOException) {
              return@withContext ShellResult(126, "", error.message ?: "su 不可用")
            }

        val stdinJob =
            startDaemonTask<Unit>("srx-shell-stdin") {
              try {
                proc.outputStream.use { output -> stdin?.let(output::write) }
              } catch (_: IOException) {
                Unit
              }
              Unit
            }
        val stdoutJob =
            startDaemonTask("srx-shell-stdout") {
              runCatching {
                    BufferedReader(InputStreamReader(proc.inputStream)).use { it.readRemaining() }
                  }
                  .getOrDefault("")
            }
        val stderrJob =
            startDaemonTask("srx-shell-stderr") {
              runCatching {
                    BufferedReader(InputStreamReader(proc.errorStream)).use { it.readRemaining() }
                  }
                  .getOrDefault("")
            }
        try {
          if (!waitForProcess(proc, timeoutMs)) {
            terminateProcess(proc, stdinJob, stdoutJob, stderrJob)
            val stdoutText = stdoutJob.completedValue().orEmpty()
            val stderrText = stderrJob.completedValue().orEmpty()
            val timeoutText =
                if (stderrText.isBlank()) {
                  "命令执行超时"
                } else {
                  "$stderrText\n命令执行超时"
                }
            return@withContext ShellResult(124, stdoutText, timeoutText)
          }
          stdinJob.get(1, TimeUnit.SECONDS)
          val stdoutText = stdoutJob.awaitOutputOrClose(proc.inputStream)
          val stderrText = stderrJob.awaitOutputOrClose(proc.errorStream)
          ShellResult(proc.exitValue(), stdoutText, stderrText)
        } catch (canceled: CancellationException) {
          terminateProcess(proc, stdinJob, stdoutJob, stderrJob)
          throw canceled
        }
      }

  override fun close() = Unit
}

private suspend fun waitForProcess(process: Process, timeoutMs: Long): Boolean {
  val deadlineNanos = System.nanoTime() + TimeUnit.MILLISECONDS.toNanos(timeoutMs.coerceAtLeast(0L))
  while (true) {
    currentCoroutineContext().ensureActive()
    val remainingNanos = deadlineNanos - System.nanoTime()
    if (remainingNanos <= 0L) return !process.isAlive
    val waitMs = TimeUnit.NANOSECONDS.toMillis(remainingNanos).coerceIn(1L, 100L)
    if (process.waitFor(waitMs, TimeUnit.MILLISECONDS)) return true
  }
}

private suspend fun terminateProcess(
    process: Process,
    stdinJob: FutureTask<Unit>,
    stdoutJob: FutureTask<String>,
    stderrJob: FutureTask<String>,
) {
  runCatching { process.destroyForcibly() }
  stdinJob.cancel(true)
  stdoutJob.cancel(true)
  stderrJob.cancel(true)
  withContext(Dispatchers.IO) { runCatching { process.waitFor(1, TimeUnit.SECONDS) } }
}

private fun FutureTask<String>.awaitOutputOrClose(stream: java.io.InputStream): String {
  runCatching { get(1, TimeUnit.SECONDS) }
      .getOrNull()
      ?.let {
        return it
      }
  cancel(true)
  startDaemonTask<Unit>("srx-shell-stream-close") {
    runCatching { stream.close() }
    Unit
  }
  return completedValue().orEmpty()
}

private fun <T> FutureTask<T>.completedValue(): T? =
    if (isDone && !isCancelled) runCatching { get() }.getOrNull() else null

private fun <T> startDaemonTask(name: String, block: () -> T): FutureTask<T> {
  val task = FutureTask(Callable(block))
  Thread(task, name).apply { isDaemon = true }.start()
  return task
}

private fun BufferedReader.readRemaining(): String =
    buildString {
          while (true) {
            val line = readLine() ?: break
            append(line).append('\n')
          }
        }
        .trim()
