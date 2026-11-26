package org.srx.manager.data

import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.srx.manager.root.ShellResult
import org.srx.manager.root.shellQuote

class RootAppQueryTest {
  @Test
  fun listUsersReadsSystemServerInventory() = runBlocking {
    val shell = CapturingShell(ShellResult(0, "0\n10\n10\n", ""))
    val query = RootAppQuery(shell)

    assertEquals(listOf("0", "10"), query.listUsers())
    assertEquals(
        "${shellQuote(SrxDaemonPath)} package-query T",
        shell.invocations.single().command,
    )
  }

  @Test
  fun listUsersFallsBackToOwnerWhenQueryFailsOrIsEmpty() = runBlocking {
    assertEquals(listOf("0"), RootAppQuery(CapturingShell()).listUsers())
    assertEquals(
        listOf("0"),
        RootAppQuery(CapturingShell(ShellResult(0, "", ""))).listUsers(),
    )
  }

  @Test
  fun listsOnlyTheRequestedUsersApplicationsAndMetadata() = runBlocking {
    val shell =
        CapturingShell(
            ShellResult(
                0,
                "10\t1010123\t1\tcom.example.system\tSystem%20App\t%2Fsystem%2Fapp%2Fsystem%2Fbase.apk\n" +
                    "10\t1010456\t0\tcom.example.user\tUser%09App\t%2Fdata%2Fapp%2Fuser%2Fbase.apk\n",
                "",
            ),
        )

    val apps = RootAppQuery(shell).listUserPackages("10")

    assertEquals(listOf("com.example.system", "com.example.user"), apps.packageNames)
    assertEquals(setOf("com.example.system"), apps.systemPackages)
    assertEquals(mapOf("com.example.system" to 1010123, "com.example.user" to 1010456), apps.uids)
    assertEquals("User\tApp", apps.labels["com.example.user"])
    assertEquals(
        "/data/app/user/base.apk",
        apps.apkPaths["com.example.user"],
    )
    assertEquals(
        "${shellQuote(SrxDaemonPath)} package-query A 10",
        shell.invocations.single().command,
    )
  }

  @Test
  fun parsesVersionedSevenColumnInventory() {
    // v2 清单：首行协议版本 + 每行末尾 icon 资源 ID。旧模块（无首行、6 列）也要继续可用。
    val text =
        "#srx-inventory\t2\n" +
            "10\t1010123\t1\tcom.example.system\tSystem%20App\t%2Fsystem%2Fapp.apk\t2130903040\n" +
            "10\t1010456\t0\tcom.example.user\tUser\t%2Fdata%2Fapp.apk\t2130903041\n"
    val apps = parseSystemServerAppList(text, "10")
    assertEquals(listOf("com.example.system", "com.example.user"), apps.packageNames)
    assertEquals(2130903040, apps.icons["com.example.system"])
    assertEquals(2130903041, apps.icons["com.example.user"])
    assertEquals(setOf("com.example.system"), apps.systemPackages)
  }

  @Test
  fun keepsNegativeIconResourceIds() {
    // 资源包 ID 高位为 1 时 `ApplicationInfo.icon` 作为有符号 int 是负数，
    // 例如 B 站（资源包 ID 0x90）的启动器图标 0x900E0000 即 -1878130688。
    // 这是有效资源 ID，归零会换成系统默认图标；同时它绝不能拉垮整份清单。
    val text =
        "#srx-inventory\t2\n" +
            "0\t10393\t0\tcom.padi.pilipili\tPILIPILI\t%2Fdata%2Fapp%2Fbili%2Fbase.apk\t-1878130688\n"
    val apps = parseSystemServerAppList(text, "0")
    assertEquals(listOf("com.padi.pilipili"), apps.packageNames)
    assertEquals(-1878130688, apps.icons["com.padi.pilipili"])
    assertEquals("PILIPILI", apps.labels["com.padi.pilipili"])

    // 边界：i32::MIN 保真；超出 Int 范围必须拒绝，不能静默截断。
    val minRow =
        "#srx-inventory\t2\n" +
            "0\t10393\t0\tcom.example.min\tMin\t%2Fdata%2Fapp.apk\t-2147483648\n"
    assertEquals(Int.MIN_VALUE, parseSystemServerAppList(minRow, "0").icons["com.example.min"])
    val underflow =
        "#srx-inventory\t2\n" +
            "0\t10393\t0\tcom.example.min\tMin\t%2Fdata%2Fapp.apk\t-2147483649\n"
    assertEquals(UserPackageList.Empty, parseSystemServerAppList(underflow, "0"))
  }

  @Test
  fun rejectsMalformedIconFields() {
    // 放宽符号位不等于放弃校验：畸形 icon 仍应使整份清单被判空。
    for (icon in listOf("", "-", "--1", "1-", "abc", "1.5", " 1", "0x90", "+1")) {
      val text =
          "#srx-inventory\t2\n" + "0\t10393\t0\tcom.example.app\tApp\t%2Fdata%2Fapp.apk\t$icon\n"
      assertEquals(
          "malformed icon accepted: $icon",
          UserPackageList.Empty,
          parseSystemServerAppList(text, "0"),
      )
    }
  }

  @Test
  fun rejectsUnsafeUserAndMixedUserOrMalformedRows() = runBlocking {
    val shell = CapturingShell()
    assertEquals(UserPackageList.Empty, RootAppQuery(shell).listUserPackages("0;rm"))
    assertTrue(shell.invocations.isEmpty())

    assertEquals(
        UserPackageList.Empty,
        parseSystemServerAppList("11\t1010123\t0\tcom.example.app\tApp\t%2Fdata%2Fapp.apk\n", "10"),
    )
    assertEquals(
        UserPackageList.Empty,
        parseSystemServerAppList(
            "10\t1010123\t0\tcom.example.app\tBad%2\t%2Fdata%2Fapp.apk\n",
            "10",
        ),
    )
    assertEquals(
        UserPackageList.Empty,
        parseSystemServerAppList("10\t1010123\t0\tcom.example.app\tApp\trelative.apk\n", "10"),
    )
    assertEquals(
        UserPackageList.Empty,
        parseSystemServerAppList("10\t+10123\t0\tcom.example.app\tApp\t%2Fdata%2Fapp.apk\n", "10"),
    )
    assertEquals(
        UserPackageList.Empty,
        parseSystemServerAppList("10\t10123\t0\tcom.example.app\tApp\t%2Fdata%2Fapp.apk\n", "10"),
    )
    assertEquals(
        UserPackageList.Empty,
        parseSystemServerAppList(
            "10\t2147483648\t0\tcom.example.app\tApp\t%2Fdata%2Fapp.apk\n",
            "10",
        ),
    )
  }
}
