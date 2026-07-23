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
}

private fun commandStarter(command: String): RootProcessStarter = RootProcessStarter {
  commandProcess(command)
}

private fun outputCommand(): String =
    if (isWindows()) "<nul set /p=out & <nul set /p=err 1>&2 & exit /b 7"
    else "printf 'out'; printf 'err' >&2; exit 7"

private fun longRunningCommand(): String =
    if (isWindows()) "ping -n 11 127.0.0.1 >nul" else "sleep 10"

private fun commandProcess(command: String): Process =
    if (isWindows()) ProcessBuilder("cmd.exe", "/c", command).start()
    else ProcessBuilder("sh", "-c", command).start()

private fun isWindows(): Boolean =
    System.getProperty("os.name").startsWith("Windows", ignoreCase = true)
