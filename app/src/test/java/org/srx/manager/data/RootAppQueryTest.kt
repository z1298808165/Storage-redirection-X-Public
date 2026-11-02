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

    assertEquals(DexAppList.Empty, query.loadDexAppList("0;rm"))
    assertTrue(shell.invocations.isEmpty())
  }

  @Test
  fun loadDexAppListParsesLabelsAndSystemMarker() = runBlocking {
    val shell =
        CapturingShell(
            ShellResult(
                0,
                """
                com.example.alpha=Alpha
                bad/pkg=Bad
                # comment
                org.demo.beta Beta Label
                #system=com.android.settings org.demo.sys bad/pkg
                """
                    .trimIndent(),
                "",
            ),
        )
    val query = RootAppQuery(shell)

    val appList = query.loadDexAppList("10")

    assertEquals(
        mapOf("com.example.alpha" to "Alpha", "org.demo.beta" to "org.demo.beta"),
        appList.labels,
    )
    assertEquals(setOf("com.android.settings", "org.demo.sys"), appList.systemPackages)
    // 拉起 app_process 与回读输出必须在同一条命令里：每次 exec 都是一次新的 su 进程。
    val command = shell.invocations.single().command
    assertTrue(command.contains("--user 10 > ${shellQuote(ListAppsOutputPath)}"))
    assertTrue(command.contains("cat ${shellQuote(ListAppsOutputPath)}"))
  }

  @Test
  fun dexAppListWithoutMarkerKeepsSystemSetEmpty() = runBlocking {
    val shell = CapturingShell(ShellResult(0, "com.example.alpha=Alpha\n", ""))

    val appList = RootAppQuery(shell).loadDexAppList("0")

    assertEquals(mapOf("com.example.alpha" to "Alpha"), appList.labels)
    assertTrue(appList.systemPackages.isEmpty())
  }

  @Test
  fun listUserPackagesSeparatesMembersFromSystemApps() = runBlocking {
    val shell =
        CapturingShell(
            ShellResult(
                0,
                """
                package:/data/app/alpha/base.apk=com.example.alpha uid:10001
                package:/data/app/bad/base.apk=bad/pkg uid:10002
                package:/system/app/beta/base.apk=org.demo.beta uid:10003
                package:/data/app/~~MPMDBgih6vWQNh_eSekUiA==/com.google.android.webview-6SH5==/base.apk=com.google.android.webview uid:99910173
                $PackageSectionMarker
                package:com.example.alpha
                package:org.demo.beta
                package:bad/pkg
                """
                    .trimIndent(),
                "",
            ),
        )
    val query = RootAppQuery(shell)

    val packages = query.listUserPackages("999")

    assertEquals(
        listOf(
            "com.example.alpha",
            "org.demo.beta",
            "com.google.android.webview",
        ),
        packages.packageNames,
    )
    assertEquals(setOf("com.example.alpha", "org.demo.beta"), packages.systemPackages)
    // APK 路径自带 `==`，只有最后一个 `=` 之后才是包名，路径必须原样保留供归档解析使用。
    assertEquals(
        "/data/app/~~MPMDBgih6vWQNh_eSekUiA==/com.google.android.webview-6SH5==/base.apk",
        packages.apkPaths["com.google.android.webview"],
    )
    val command = shell.invocations.single().command
    assertTrue(command.contains("pm list packages -f -U --user 999"))
    assertTrue(command.contains("pm list packages -s --user 999"))
    // 标记必须带引号：裸写 `#srx-system` 会被 sh 当成注释，连带把后面的 `-s` 查询整条吃掉，
    // 系统应用集合就会静默变成空集（用户 0 与分身用户的系统标记随即串味）。
    assertTrue(command.contains("echo '$PackageSectionMarker';"))
  }

  @Test
  fun listUserPackagesFallsBackToCmdPackage() = runBlocking {
    val shell =
        CapturingShell(
            ShellResult(0, "", ""),
            ShellResult(0, "package:/data/app/alpha/base.apk=com.example.alpha\n", ""),
        )

    val packages = RootAppQuery(shell).listUserPackages("10")

    assertEquals(listOf("com.example.alpha"), packages.packageNames)
    assertTrue(shell.invocations[1].command.startsWith("cmd package list packages -f -U"))
  }

  @Test
  fun listUserPackagesRejectsUnsafeUserId() = runBlocking {
    val shell = CapturingShell()

    assertEquals(UserPackageList.Empty, RootAppQuery(shell).listUserPackages("0;rm"))
    assertTrue(shell.invocations.isEmpty())
  }
}
