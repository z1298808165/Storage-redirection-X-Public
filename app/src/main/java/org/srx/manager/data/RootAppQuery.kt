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
        val runDex = shell.exec(
            "mkdir -p /data/Namespace-Proxy; " +
                "if [ -f ${shellQuote(ListAppsDexPath)} ]; then " +
                "/system/bin/app_process64 -Djava.class.path=${shellQuote(ListAppsDexPath)} / Main --user $userId > ${shellQuote(ListAppsOutputPath)} 2>/dev/null; fi"
        )
        val text = if (runDex.isSuccess) shell.exec("cat ${shellQuote(ListAppsOutputPath)} 2>/dev/null").stdout else ""
        return text.lineSequence()
            .mapNotNull { line ->
                val trimmed = line.trim()
                if (trimmed.isBlank() || trimmed.startsWith("#")) return@mapNotNull null
                val split = trimmed.indexOf('=')
                val pkg = if (split >= 0) trimmed.substring(0, split).trim() else trimmed.substringBefore(' ')
                if (!isSafePackageName(pkg)) null else pkg to (if (split >= 0) trimmed.substring(split + 1).trim() else pkg)
            }
            .toMap()
    }
}
