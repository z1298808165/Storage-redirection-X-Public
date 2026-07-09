package org.srx.manager.data

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.srx.manager.root.ShellResult
import org.srx.manager.root.shellQuote

class RootAppQueryTest {
  @Test
  fun listUsersParsesDistinctUserIds() = runBlocking {
    val shell =
        CapturingShell(
            ShellResult(
                0,
                """
                Users:
                    UserInfo{0:Owner:13} running
                    UserInfo{10:Work:30} running
                    UserInfo{10:Work:30} running
                """
                    .trimIndent(),
                "",
            ),
        )
    val query = RootAppQuery(shell)

    assertEquals(listOf("0", "10"), query.listUsers())
    assertEquals(
        "cmd user list 2>/dev/null || pm list users 2>/dev/null",
        shell.invocations.single().command,
    )
  }

  @Test
  fun listUsersFallsBackToOwnerWhenNoUserOutput() = runBlocking {
    val query = RootAppQuery(CapturingShell(ShellResult(0, "", "")))

    assertEquals(listOf("0"), query.listUsers())
  }

  @Test
  fun rejectsUnsafeUserId() = runBlocking {
    val shell = CapturingShell()
    val query = RootAppQuery(shell)

    assertTrue(query.loadDexAppLabels("0;rm").isEmpty())
    assertTrue(shell.invocations.isEmpty())
  }

  @Test
  fun loadDexAppLabelsRunsDexAndFiltersUnsafePackages() = runBlocking {
    val shell =
        CapturingShell(
            ShellResult(0, "", ""),
            ShellResult(
                0,
                """
                com.example.alpha=Alpha
                bad/pkg=Bad
                # comment
                org.demo.beta Beta Label
                """
                    .trimIndent(),
                "",
            ),
        )
    val query = RootAppQuery(shell)

    val labels = query.loadDexAppLabels("10")

    assertEquals(mapOf("com.example.alpha" to "Alpha", "org.demo.beta" to "org.demo.beta"), labels)
    assertEquals(2, shell.invocations.size)
    assertTrue(
        shell.invocations[0].command.contains("--user 10 > ${shellQuote(ListAppsOutputPath)}")
    )
    assertEquals("cat ${shellQuote(ListAppsOutputPath)} 2>/dev/null", shell.invocations[1].command)
  }
}
