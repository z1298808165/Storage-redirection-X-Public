package org.srx.manager.data

import java.io.ByteArrayOutputStream
import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.isSafePackageName
import org.srx.manager.root.isSafeUserId
import org.srx.manager.root.shellQuote

/** `system_server` 对单个 Android 用户返回的已安装应用清单。 */
data class UserPackageList(
    val packageNames: List<String> = emptyList(),
    val systemPackages: Set<String> = emptySet(),
    val apkPaths: Map<String, String> = emptyMap(),
    val uids: Map<String, Int> = emptyMap(),
    val labels: Map<String, String> = emptyMap(),
    /** 图标资源 ID；0 表示该包没有可用的图标资源。 */
    val icons: Map<String, Int> = emptyMap(),
) {
  val isEmpty: Boolean
    get() = packageNames.isEmpty()

  companion object {
    val Empty = UserPackageList()
  }
}

class RootAppQuery(
    private val shell: ShellExecutor,
) {
  suspend fun listUsers(): List<String> {
    val result = shell.exec("${shellQuote(SrxDaemonPath)} package-query T")
    if (!result.isSuccess) return listOf("0")
    return parsePackageUsers(result.stdout).ifEmpty { listOf("0") }
  }

  /** 通过 system_server 中的 PackageManager 读取指定用户的包、标签、系统标记和 APK 路径。 */
  suspend fun listUserPackages(userId: String): UserPackageList {
    if (!isSafeUserId(userId)) return UserPackageList.Empty
    val result = shell.exec("${shellQuote(SrxDaemonPath)} package-query A $userId")
    if (!result.isSuccess) return UserPackageList.Empty
    return parseSystemServerAppList(result.stdout, userId)
  }
}

internal fun parsePackageUsers(text: String): List<String> =
    text.lineSequence().map(String::trim).filter { it.matches(Regex("[0-9]+")) }.distinct().toList()

/** 解析 `user\tuid\tsystem\tpackage\tlabel\tapk_path`，并拒绝跨用户或不完整的结果。 */
internal fun parseSystemServerAppList(text: String, expectedUser: String): UserPackageList {
  if (!isSafeUserId(expectedUser)) return UserPackageList.Empty
  val packages = LinkedHashSet<String>()
  val systems = LinkedHashSet<String>()
  val paths = HashMap<String, String>()
  val uids = HashMap<String, Int>()
  val labels = HashMap<String, String>()
  val icons = HashMap<String, Int>()
  // 首行可带协议版本（#srx-inventory<TAB>2）：v2 = 7 列（含 icon）；没有首行 = 旧模块 6 列。
  // 按版本定列数，而不是靠「列数猜」，避免只升级一侧时整份清单被判空。
  var expectedFields = 6
  text.lineSequence().forEachIndexed { index, rawLine ->
    if (rawLine.isEmpty()) return@forEachIndexed
    if (index == 0 && rawLine.startsWith("#srx-inventory\t")) {
      expectedFields =
          when (rawLine.substringAfter('\t')) {
            "2" -> 7
            else -> return UserPackageList.Empty
          }
      return@forEachIndexed
    }
    val line = rawLine
    val fields = line.split('\t')
    if (fields.size != expectedFields || fields[0] != expectedUser) return UserPackageList.Empty
    if (fields[1].isEmpty() || fields[1].any { it !in '0'..'9' }) return UserPackageList.Empty
    val uid = fields[1].toIntOrNull() ?: return UserPackageList.Empty
    if (uid / 100000 != expectedUser.toIntOrNull()) return UserPackageList.Empty
    if (fields[2] != "0" && fields[2] != "1") return UserPackageList.Empty
    val icon =
        if (expectedFields == 7) {
          if (fields[6].isEmpty() || fields[6].any { it !in '0'..'9' }) return UserPackageList.Empty
          fields[6].toIntOrNull() ?: return UserPackageList.Empty
        } else {
          0
        }
    val packageName = fields[3]
    if (!isSafePackageName(packageName) || !packages.add(packageName)) return UserPackageList.Empty
    uids[packageName] = uid
    val label = decodePackageField(fields[4]) ?: return UserPackageList.Empty
    val apkPath = decodePackageField(fields[5]) ?: return UserPackageList.Empty
    if (label.isEmpty() || (apkPath.isNotEmpty() && !apkPath.startsWith('/')))
        return UserPackageList.Empty
    labels[packageName] = label
    icons[packageName] = icon
    if (fields[2] == "1") systems.add(packageName)
    if (apkPath.isNotEmpty()) paths[packageName] = apkPath
  }
  return UserPackageList(packages.toList(), systems, paths, uids, labels, icons)
}

private fun decodePackageField(value: String): String? {
  val bytes = ByteArrayOutputStream(value.length)
  var index = 0
  while (index < value.length) {
    val char = value[index]
    if (char == '%') {
      if (index + 2 >= value.length) return null
      val byte = value.substring(index + 1, index + 3).toIntOrNull(16) ?: return null
      bytes.write(byte)
      index += 3
    } else {
      if (!(char.isLetterOrDigit() && char.code < 128) && char !in "._-~") return null
      bytes.write(char.code)
      index++
    }
  }
  val decoded = bytes.toByteArray().toString(Charsets.UTF_8)
  return decoded.takeIf { it.toByteArray(Charsets.UTF_8).contentEquals(bytes.toByteArray()) }
}
