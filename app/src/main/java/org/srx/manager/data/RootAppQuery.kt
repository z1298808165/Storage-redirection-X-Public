package org.srx.manager.data

import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.isSafePackageName
import org.srx.manager.root.isSafeUserId
import org.srx.manager.root.shellQuote

/** dex 侧应用清单：包名到标签的映射，以及「该用户视角下的系统应用」集合。 */
data class DexAppList(
    val labels: Map<String, String> = emptyMap(),
    val systemPackages: Set<String> = emptySet(),
) {
  companion object {
    val Empty = DexAppList()
  }
}

/** `tools/list_apps/Main.java` 输出的系统应用标记行前缀。 旧版 dex 没有该行，读取端按「无标记」处理并退回调用方的包信息判断。 */
private const val DexSystemMarker = "#system="

class RootAppQuery(
    private val shell: ShellExecutor,
) {
  suspend fun listUsers(): List<String> {
    val out = shell.exec("cmd user list 2>/dev/null || pm list users 2>/dev/null").stdout
    val ids = Regex("UserInfo\\{([0-9]+):").findAll(out).map { it.groupValues[1] }.toMutableList()
    if (ids.isEmpty()) Regex("\\{([0-9]+):").findAll(out).mapTo(ids) { it.groupValues[1] }
    return ids.distinct().ifEmpty { listOf("0") }
  }

  /**
   * 通过 dex 通道按用户枚举应用：标签与系统应用标记都来自该用户的 PackageManager 结果。 相比 shell 的 `pm list packages
   * --user`，这里一次进程调用即可拿到标签与系统标记。
   */
  suspend fun loadDexAppList(userId: String): DexAppList {
    if (!isSafeUserId(userId)) return DexAppList.Empty
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
    return parseDexAppList(text)
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

/** 解析 dex 输出：`包名=标签` 行加上末尾的 `#system=` 系统应用标记行。 */
internal fun parseDexAppList(text: String): DexAppList {
  val labels = LinkedHashMap<String, String>()
  val systemPackages = LinkedHashSet<String>()
  text.lineSequence().forEach { line ->
    val trimmed = line.trim()
    if (trimmed.isEmpty()) return@forEach
    if (trimmed.startsWith(DexSystemMarker)) {
      trimmed
          .substring(DexSystemMarker.length)
          .split(' ', '\t')
          .filter { it.isNotEmpty() && isSafePackageName(it) }
          .forEach { systemPackages.add(it) }
      return@forEach
    }
    if (trimmed.startsWith("#")) return@forEach
    val split = trimmed.indexOf('=')
    val pkg = if (split >= 0) trimmed.substring(0, split).trim() else trimmed.substringBefore(' ')
    if (!isSafePackageName(pkg)) return@forEach
    labels[pkg] = if (split >= 0) trimmed.substring(split + 1).trim() else pkg
  }
  return DexAppList(labels = labels, systemPackages = systemPackages)
}
