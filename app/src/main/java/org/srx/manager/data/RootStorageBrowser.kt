package org.srx.manager.data

import org.srx.manager.root.ShellExecutor
import org.srx.manager.root.isSafeUserId
import org.srx.manager.root.shellQuote

class RootStorageBrowser(
    private val shell: ShellExecutor,
) {
  suspend fun listDirectories(
      userId: String,
      dirRel: String,
  ): List<String> {
    if (!isSafeUserId(userId)) return emptyList()
    val clean =
        dirRel
            .replace('\\', '/')
            .trim('/')
            .split('/')
            .filter { it.isNotBlank() && it != "." && it != ".." }
            .joinToString("/")
    if (clean.isAndroidDataPrivatePath()) return emptyList()
    val target =
        if (clean.isBlank()) "/storage/emulated/$userId" else "/storage/emulated/$userId/$clean"
    val candidates =
        buildList {
              add(target)
              add(if (clean.isBlank()) "/data/media/$userId" else "/data/media/$userId/$clean")
              if (userId == "0") add(if (clean.isBlank()) "/sdcard" else "/sdcard/$clean")
            }
            .distinct()
    val command = buildString {
      append("for dir in ")
      append(candidates.joinToString(" ") { shellQuote(it) })
      append(
          "; do [ -d \"\$dir\" ] || continue; out=$(ls -1Ap \"\$dir\" 2>/dev/null); [ -z \"\$out\" ] || { printf '%s\n' \"\$out\"; exit 0; }; done",
      )
    }
    val out = shell.exec(command, mountMaster = true).stdout
    return out.lineSequence()
        .map { it.trim() }
        .filter { it.isNotBlank() && it != "./" && it != "../" }
        .distinctBy { it.trimEnd('/').lowercase() }
        .sortedWith(
            compareBy<String> { !it.endsWith("/") }
                .thenBy(String.CASE_INSENSITIVE_ORDER) { it.trimEnd('/') }
        )
        .toList()
  }

  private fun String.isAndroidDataPrivatePath(): Boolean {
    val clean = trim('/').lowercase()
    return clean == "android/data" || clean.startsWith("android/data/")
  }
}
