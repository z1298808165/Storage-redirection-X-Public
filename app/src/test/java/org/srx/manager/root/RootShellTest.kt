package org.srx.manager.root

import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class RootShellTest {
  @Test
  fun execCollectsOutputAndExitCode() = runBlocking {
    val shell = RootShell(commandStarter(outputCommand()))

    val result = shell.exec("ignored")

    assertEquals(7, result.code)
    assertEquals("out", result.stdout)
    assertEquals("err", result.stderr)
  }

  @Test
  fun execTimeoutReturnsWhenChildKeepsPipesOpen() = runBlocking {
    val shell = RootShell(commandStarter(longRunningCommand()))
    val started = System.nanoTime()

    val result = shell.exec("ignored", timeoutMs = 100L)
    val elapsedMs = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)

    assertEquals(124, result.code)
    assertTrue(result.stderr.contains("命令执行超时"))
    assertTrue("超时清理耗时 $elapsedMs ms", elapsedMs < 3_000L)
  }

  @Test
  fun execCancellationTerminatesProcessAndPropagates() = runBlocking {
    lateinit var process: Process
    val shell =
        RootShell(
            RootProcessStarter {
              process = commandProcess(longRunningCommand())
              process
            }
        )
    val task = async { shell.exec("ignored", timeoutMs = 30_000L) }
    delay(100L)

    task.cancel()
    val error = runCatching { task.await() }.exceptionOrNull()

    assertTrue(error is CancellationException)
    process.waitFor(2, TimeUnit.SECONDS)
    assertFalse(process.isAlive)
  }

  @Test
  fun execStreamingDeliversOutputBeforeProcessExit() = runBlocking {
    val shell = RootShell(commandStarter(streamingCommand()))
    var firstLineAtNanos = 0L
    val started = System.nanoTime()

    val result =
        shell.execStreaming("ignored", timeoutMs = 5_000L) { line ->
          if (line == "first") firstLineAtNanos = System.nanoTime()
        }
    val completedAtNanos = System.nanoTime()

    assertTrue(result.isSuccess)
    assertEquals("first\nsecond", result.stdout)
    assertTrue(firstLineAtNanos > started)
    assertTrue(
        "首行应在进程退出前到达",
        TimeUnit.NANOSECONDS.toMillis(completedAtNanos - firstLineAtNanos) >= 100L,
    )
  }

  @Test
  fun execStreamingPropagatesCallbackFailure() = runBlocking {
    val shell = RootShell(commandStarter(callbackFailureCommand()))
    val started = System.nanoTime()

    val error =
        runCatching { shell.execStreaming("ignored") { throw IllegalStateException("回调失败") } }
            .exceptionOrNull()

    assertTrue(error is IllegalStateException)
    assertEquals("回调失败", error?.message)
    assertTrue(
        "回调失败后应立即终止命令",
        TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started) < 2_000L,
    )
  }
}

private fun commandStarter(command: String): RootProcessStarter = RootProcessStarter {
  commandProcess(command)
}

private fun outputCommand(): String =
    if (isWindows()) "<nul set /p=out & <nul set /p=err 1>&2 & exit /b 7"
    else "printf 'out'; printf 'err' >&2; exit 7"

private fun longRunningCommand(): String =
    if (isWindows()) "ping -n 11 127.0.0.1 >nul" else "sleep 10"

private fun streamingCommand(): String =
    if (isWindows()) "echo first& ping -n 2 127.0.0.1 >nul & echo second"
    else "printf 'first\\n'; sleep 0.2; printf 'second\\n'"

private fun callbackFailureCommand(): String =
    if (isWindows()) "echo first& ping -n 11 127.0.0.1 >nul" else "printf 'first\\n'; sleep 10"

private fun commandProcess(command: String): Process =
    if (isWindows()) ProcessBuilder("cmd.exe", "/c", command).start()
    else ProcessBuilder("sh", "-c", command).start()

private fun isWindows(): Boolean =
    System.getProperty("os.name").orEmpty().startsWith("Windows", ignoreCase = true)
