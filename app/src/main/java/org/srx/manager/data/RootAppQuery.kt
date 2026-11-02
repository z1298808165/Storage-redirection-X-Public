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

/**
 * 按用户查询到的包清单。
 *
 * [packageNames] 是该用户可见的全部包名，[systemPackages] 是该用户视角下的系统应用， [apkPaths] 是 `pm list packages -f` 给出的
 * base.apk 路径 —— 分身用户独有的应用不在主用户视角里， 只能靠 APK 路径解析标签与图标。
 */
data class UserPackageList(
    val packageNames: List<String> = emptyList(),
    val systemPackages: Set<String> = emptySet(),
    val apkPaths: Map<String, String> = emptyMap(),
) {
  val isEmpty: Boolean
    get() = packageNames.isEmpty() && systemPackages.isEmpty()

  companion object {
    val Empty = UserPackageList()
  }
}

/** `pm list packages -f -U` 与 `pm list packages -s` 两段输出的分隔标记。 */
internal const val PackageSectionMarker = "#srx-system"

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
   * 通过 dex 通道按用户枚举应用：标签与系统应用标记都来自该用户的 PackageManager 结果。
   *
   * 该通道要拉起一次 app_process（实测 1.3~1.9s），只在 shell 的 `pm` 查询完全不可用时兜底， 正常路径用 [listUserPackages]。
   */
  suspend fun loadDexAppList(userId: String): DexAppList {
    if (!isSafeUserId(userId)) return DexAppList.Empty
    val outputPath = shellQuote(ListAppsOutputPath)
    val result =
        shell.exec(
            "mkdir -p /data/Namespace-Proxy; " +
                "if [ -f ${shellQuote(ListAppsDexPath)} ]; then " +
                "/system/bin/app_process64 -Djava.class.path=${shellQuote(
                        ListAppsDexPath,
                    )} / Main --user $userId > $outputPath 2>/dev/null; fi; " +
                "cat $outputPath 2>/dev/null",
        )
    return parseDexAppList(result.stdout)
  }

  /**
   * 一次 root 调用拿到指定用户的「成员 + 系统应用 + APK 路径」。
   *
   * 相比拉起 dex 进程，shell 的 `pm` 查询快一个数量级（实测 60~80ms）；标签与图标由调用方的 PackageManager 或 APK 路径补齐，因此正常路径不再需要
   * dex。
   */
  suspend fun listUserPackages(userId: String): UserPackageList {
    if (!isSafeUserId(userId)) return UserPackageList.Empty
    val userArg = " --user $userId"
    // 标记必须加引号：裸写 `echo #srx-system` 会被 sh 当成注释，连带把后面的 `-s` 查询整条吃掉，
    // 结果就是系统应用集合永远为空、退回调用方用户的 flags 判定（跨用户必然误判）。
    val marker = "echo ${shellQuote(PackageSectionMarker)}"
    val commands =
        listOf(
            "pm list packages -f -U$userArg 2>/dev/null; " +
                "$marker; pm list packages -s$userArg 2>/dev/null",
            "cmd package list packages -f -U$userArg 2>/dev/null; " +
                "$marker; cmd package list packages -s$userArg 2>/dev/null",
        )
    for (command in commands) {
      val result = shell.exec(command)
      if (!result.isSuccess) continue
      val packages = parseUserPackageList(result.stdout)
      if (!packages.isEmpty) return packages
    }
    return UserPackageList.Empty
  }
}

/**
 * 解析 [RootAppQuery.listUserPackages] 的输出。
 *
 * 前一段是 `package:<apk 路径>=<包名> uid:<uid>`，后一段是 `package:<包名>`；APK 路径本身可能含 `==`（如
 * /data/app/~~xxxx==/base.apk），因此包名只取最后一个 `=` 之后的部分。
 */
internal fun parseUserPackageList(text: String): UserPackageList {
  val members = LinkedHashSet<String>()
  val systemPackages = LinkedHashSet<String>()
  val apkPaths = HashMap<String, String>()
  var inSystemSection = false
  text.lineSequence().forEach { raw ->
    val line = raw.trim()
    if (line.isEmpty()) return@forEach
    if (line == PackageSectionMarker) {
      inSystemSection = true
      return@forEach
    }
    if (!line.startsWith("package:")) return@forEach
    val body = line.removePrefix("package:")
    if (inSystemSection) {
      val packageName = body.substringBefore(' ').trim()
      if (isSafePackageName(packageName)) systemPackages.add(packageName)
      return@forEach
    }
    val split = body.lastIndexOf('=')
    val apkPath = if (split >= 0) body.substring(0, split).trim() else ""
    val packageName =
        (if (split >= 0) body.substring(split + 1) else body).substringBefore(' ').trim()
    if (!isSafePackageName(packageName)) return@forEach
    members.add(packageName)
    if (apkPath.startsWith("/")) apkPaths[packageName] = apkPath
  }
  return UserPackageList(members.toList(), systemPackages, apkPaths)
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
