package org.srx.manager.data

import kotlinx.serialization.json.Json
import org.srx.manager.root.isSafePackageName

internal const val ConfiguredAppConfigMarker = "__SRX_APP_CONFIG__"

internal fun parseConfiguredAppConfigDump(
    out: String,
    marker: String,
    json: Json,
): Map<String, AppConfig> {
    val result = linkedMapOf<String, AppConfig>()
    var current: String? = null
    val body = mutableListOf<String>()

    fun flush() {
        val pkg = current
        if (pkg != null && isSafePackageName(pkg)) {
            val text = body.joinToString("\n").trim()
            runCatching { json.decodeFromString<AppConfig>(text) }.getOrNull()?.let {
                result[pkg] = SrxConfigNormalizer.normalizeAppConfig(it)
            }
        }
        body.clear()
    }

    out.lineSequence().forEach { line ->
        if (line.startsWith(marker)) {
            flush()
            current = line.removePrefix(marker).trim()
        } else if (current != null) {
            body += line
        }
    }
    flush()

    return result
}
