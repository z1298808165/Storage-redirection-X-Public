package org.srx.manager.data

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.srx.manager.root.ShellResult
import org.srx.manager.root.shellQuote

class RootStorageBrowserTest {
  @Test
  fun rejectsUnsafeUserId() = runBlocking {
    val shell = CapturingShell()
    val browser = RootStorageBrowser(shell)

    assertTrue(browser.listDirectories("0;rm", "Pictures").isEmpty())
    assertTrue(shell.invocations.isEmpty())
  }

  @Test
  fun rejectsAndroidDataPrivatePath() = runBlocking {
    val shell = CapturingShell()
    val browser = RootStorageBrowser(shell)

    assertTrue(browser.listDirectories("0", "/Android/data/com.example").isEmpty())
    assertTrue(shell.invocations.isEmpty())
  }

  @Test
  fun usesCleanedCandidatesWithMountMaster() = runBlocking {
    val shell =
        CapturingShell(
            ShellResult(
                0,
                """
                Pictures/
                alpha.txt
                DCIM/
                pictures/
                ./
                ../
                """
                    .trimIndent(),
                "",
            ),
        )
    val browser = RootStorageBrowser(shell)

    val entries = browser.listDirectories("0", "..\\Pictures//./Camera")

    assertEquals(listOf("DCIM/", "Pictures/", "alpha.txt"), entries)
    val invocation = shell.invocations.single()
    assertTrue(invocation.mountMaster)
    assertTrue(invocation.command.contains(shellQuote("/storage/emulated/0/Pictures/Camera")))
    assertTrue(invocation.command.contains(shellQuote("/data/media/0/Pictures/Camera")))
    assertTrue(invocation.command.contains(shellQuote("/sdcard/Pictures/Camera")))
  }

  @Test
  fun listDirectoriesOmitsSdcardAliasForNonOwnerUser() = runBlocking {
    val shell = CapturingShell(ShellResult(0, "Docs/\n", ""))
    val browser = RootStorageBrowser(shell)

    assertEquals(listOf("Docs/"), browser.listDirectories("10", "Documents"))
    val command = shell.invocations.single().command
    assertTrue(command.contains(shellQuote("/storage/emulated/10/Documents")))
    assertTrue(command.contains(shellQuote("/data/media/10/Documents")))
    assertTrue(!command.contains("/sdcard"))
  }
}
