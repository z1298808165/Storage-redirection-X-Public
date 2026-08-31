package org.srx.manager.data

import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.isSafePackageName
import org.srx.manager.root.isSafeUserId
import org.srx.manager.root.shellQuote

class RootAppQuery(
    private val shell: ShellExecutor,
) {
  suspend fun listUsers(): List<String> {
    val out = shell.exec("cmd user list 2>/dev/null || pm list users 2>/dev/null").stdout
    val ids = Regex("UserInfo\\{([0-9]+):").findAll(out).map { it.groupValues[1] }.toMutableList()
    if (ids.isEmpty()) Regex("\\{([0-9]+):").findAll(out).mapTo(ids) { it.groupValues[1] }
    return ids.distinct().ifEmpty { listOf("0") }
  }

  suspend fun loadDexAppLabels(userId: String): Map<String, String> {
    if (!isSafeUserId(userId)) return emptyMap()
    val runDex =
        shell.exec(
            "mkdir -p /data/Namespace-Proxy; " +
                "if [ -f ${shellQuote(ListAppsDexPath)} ]; then " +
                "/system/bin/app_process64 -Djava.class.path=${shellQuote(
                        ListAppsDexPath,
                    )} / Main --user $userId > ${shellQuote(ListAppsOutputPath)} 2>/dev/null; fi",
        )
    val text =
        if (runDex.isSuccess) shell.exec("cat ${shellQuote(ListAppsOutputPath)} 2>/dev/null").stdout
        else ""
    return text
        .lineSequence()
        .mapNotNull { line ->
          val trimmed = line.trim()
          if (trimmed.isBlank() || trimmed.startsWith("#")) return@mapNotNull null
          val split = trimmed.indexOf('=')
          val pkg =
              if (split >= 0) trimmed.substring(0, split).trim() else trimmed.substringBefore(' ')
          if (!isSafePackageName(pkg)) null
          else pkg to (if (split >= 0) trimmed.substring(split + 1).trim() else pkg)
        }
        .toMap()
  }

  /**
   * 通过 root shell 枚举指定用户的包名，绕过 Android 17 上普通应用的 package visibility 过滤。 这里只返回包名；标签和 ApplicationInfo
   * 仍由 PackageManager/Dex 通道解析。
   */
  suspend fun listInstalledPackages(userId: String): List<String> {
    if (!isSafeUserId(userId)) return emptyList()
    val userArg = " --user $userId"
    val commands =
        listOf(
            "pm list packages -f -U$userArg 2>/dev/null",
            "cmd package list packages -f -U$userArg 2>/dev/null",
        )
    for (command in commands) {
      val result = shell.exec(command)
      if (!result.isSuccess) continue
      val packages =
          result.stdout
              .lineSequence()
              .mapNotNull { line ->
                val value = line.trim().removePrefix("package:")
                val pathEnd = value.indexOf('=')
                val packageName =
                    if (pathEnd >= 0) value.substring(pathEnd + 1).substringBefore(' ') else value
                packageName.takeIf(::isSafePackageName)
              }
              .distinct()
              .toList()
      if (packages.isNotEmpty()) return packages
    }
    return emptyList()
  }
}
