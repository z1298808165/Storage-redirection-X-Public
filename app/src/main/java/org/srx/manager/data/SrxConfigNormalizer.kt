package org.srx.manager.data

import java.security.MessageDigest
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.srx.manager.root.isSafePackageName
import org.srx.manager.root.isSafeUserId

internal object SrxConfigNormalizer {
  private val LegacyDefaultMonitorOperations = listOf("open:read", "rename*", "unlink*", "delete*")
  private val LegacyFullDefaultMonitorOperations =
      listOf(
          "open:read",
          "open*:read",
          "rename*",
          "unlink*",
          "delete*",
          "rmdir*",
          "link*",
          "symlink*",
          "truncate*",
          "ftruncate*",
          "chmod*",
          "fchmod*",
          "utimens*",
          "futimens*",
          "attrib*",
      )
  private const val MaxPathMappingDepth = 10

  fun normalizeGlobalConfig(config: GlobalConfig): GlobalConfig =
      config.copy(
          autoEnableNewAppsTemplateId =
              config.autoEnableNewAppsTemplateId.trim().takeIf(::isSafeTemplateId).orEmpty(),
      )

  fun normalizeAppConfig(config: AppConfig): AppConfig {
    return config.copy(
        users =
            config.users
                .filterKeys(::isSafeUserId)
                .toSortedMap(
                    compareBy<String> { it.toLongOrNull() ?: Long.MAX_VALUE }.thenBy { it }
                )
                .mapValues { (_, profile) -> normalizeUserProfile(profile) },
    )
  }

  fun normalizeBackupData(data: BackupData): BackupData =
      BackupData(
          global = normalizeGlobalConfig(data.global),
          apps =
              data.apps.filterKeys(::isSafePackageName).toSortedMap().mapValues { (_, config) ->
                normalizeAppConfig(config)
              },
          templates = normalizeTemplates(data.templates),
          monitorFilters = normalizeFileMonitorFilters(data.monitorFilters),
          ui = data.ui,
      )

  fun normalizeTemplateStore(store: ConfigTemplateStore): ConfigTemplateStore =
      ConfigTemplateStore(normalizeTemplates(store.templates))

  fun normalizeTemplates(templates: List<ConfigTemplate>): List<ConfigTemplate> {
    val seen = linkedSetOf<String>()
    return templates
        .mapNotNull { template ->
          val id = template.id.trim().takeIf(::isSafeTemplateId) ?: return@mapNotNull null
          val name =
              template.name.trim().take(48).takeIf { it.isNotBlank() } ?: return@mapNotNull null
          if (!seen.add(id)) return@mapNotNull null
          ConfigTemplate(id = id, name = name, config = normalizeAppConfig(template.config))
        }
        .sortedWith(compareBy<ConfigTemplate> { it.name.lowercase() }.thenBy { it.id })
  }

  fun normalizeFileMonitorFilters(filters: FileMonitorFilters): FileMonitorFilters =
      FileMonitorFilters(
          excludedPaths = normalizeMonitorFilterPathList(filters.excludedPaths),
          excludedOperations = normalizeMonitorFilterOperations(filters.excludedOperations),
      )

  fun stableJson(
      json: Json,
      data: BackupData,
      includeAutoEnableNewApps: Boolean = true,
      includeAutoEnableNewAppsTemplateId: Boolean = true,
      includeVerboseLogging: Boolean = true,
      includeUiPreferences: Boolean = true,
      includeTemplates: Boolean = true,
      includeMonitorFilters: Boolean = true,
  ): String =
      stableJson(
          json,
          stripBackupCompatibilityFields(
              json.parseToJsonElement(json.encodeToString(data)),
              includeAutoEnableNewApps,
              includeAutoEnableNewAppsTemplateId,
              includeVerboseLogging,
              includeUiPreferences,
              includeTemplates,
              includeMonitorFilters,
          ),
      )

  fun backupDigestCandidates(json: Json, data: BackupData): Set<String> = buildSet {
    backupDigestDataVariants(data).forEach { candidate ->
      listOf(true, false).forEach { includeTemplates ->
        listOf(true, false).forEach { includeMonitorFilters ->
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeVerboseLogging = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeUiPreferences = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeVerboseLogging = false,
                      includeUiPreferences = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewAppsTemplateId = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewAppsTemplateId = false,
                      includeVerboseLogging = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewAppsTemplateId = false,
                      includeUiPreferences = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewAppsTemplateId = false,
                      includeVerboseLogging = false,
                      includeUiPreferences = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewApps = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewApps = false,
                      includeVerboseLogging = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewApps = false,
                      includeUiPreferences = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
          add(
              sha256Hex(
                  stableJson(
                      json,
                      candidate,
                      includeAutoEnableNewApps = false,
                      includeVerboseLogging = false,
                      includeUiPreferences = false,
                      includeTemplates = includeTemplates,
                      includeMonitorFilters = includeMonitorFilters,
                  )
              )
          )
        }
      }
    }
  }

  private fun backupDigestDataVariants(data: BackupData): List<BackupData> {
    val variants = mutableListOf(data)
    if (data.monitorFilters.excludedOperations == FileMonitorFilters().excludedOperations) {
      variants +=
          data.copy(
              monitorFilters =
                  data.monitorFilters.copy(excludedOperations = LegacyDefaultMonitorOperations),
          )
      variants +=
          data.copy(
              monitorFilters =
                  data.monitorFilters.copy(excludedOperations = LegacyFullDefaultMonitorOperations),
          )
    }
    return variants.distinct()
  }

  fun sha256Hex(value: String): String {
    val digest = MessageDigest.getInstance("SHA-256").digest(value.toByteArray(Charsets.UTF_8))
    return digest.joinToString("") { "%02x".format(it) }
  }

  fun isSafeTemplateId(value: String): Boolean = Regex("^[A-Za-z0-9_.-]{1,80}$").matches(value)

  fun sanitizeEditablePath(
      raw: String,
      allowRuleSyntax: Boolean,
      allowWildcards: Boolean = allowRuleSyntax,
  ): String = sanitizeConfigPath(raw, allowRuleSyntax, allowWildcards).orEmpty()

  fun sanitizeMonitorFilterPath(raw: String, allowLegacyAbsolute: Boolean = true): String =
      sanitizeMonitorFilterPathOrNull(raw, allowLegacyAbsolute).orEmpty()

  private fun stripBackupCompatibilityFields(
      element: JsonElement,
      includeAutoEnableNewApps: Boolean,
      includeAutoEnableNewAppsTemplateId: Boolean,
      includeVerboseLogging: Boolean,
      includeUiPreferences: Boolean,
      includeTemplates: Boolean,
      includeMonitorFilters: Boolean,
  ): JsonElement =
      when (element) {
        is JsonObject ->
            JsonObject(
                element.entries
                    .mapNotNull { (key, value) ->
                      if (!includeUiPreferences && key == "ui") return@mapNotNull null
                      if (!includeTemplates && key == "templates") return@mapNotNull null
                      if (!includeMonitorFilters && key == "monitor_filters") return@mapNotNull null
                      val nextValue =
                          if (
                              (!includeAutoEnableNewApps ||
                                  !includeAutoEnableNewAppsTemplateId ||
                                  !includeVerboseLogging) && key == "global" && value is JsonObject
                          ) {
                            JsonObject(
                                value.filterKeys {
                                  if (
                                      !includeAutoEnableNewApps &&
                                          it == "auto_enable_redirect_for_new_apps"
                                  )
                                      return@filterKeys false
                                  if (
                                      (!includeAutoEnableNewApps ||
                                          !includeAutoEnableNewAppsTemplateId) &&
                                          it == "auto_enable_new_apps_template_id"
                                  )
                                      return@filterKeys false
                                  if (!includeVerboseLogging && it == "verbose_logging_enabled")
                                      return@filterKeys false
                                  true
                                },
                            )
                          } else {
                            stripBackupCompatibilityFields(
                                value,
                                includeAutoEnableNewApps,
                                includeAutoEnableNewAppsTemplateId,
                                includeVerboseLogging,
                                includeUiPreferences,
                                includeTemplates,
                                includeMonitorFilters,
                            )
                          }
                      key to nextValue
                    }
                    .toMap(),
            )
        is JsonArray ->
            JsonArray(
                element.map {
                  stripBackupCompatibilityFields(
                      it,
                      includeAutoEnableNewApps,
                      includeAutoEnableNewAppsTemplateId,
                      includeVerboseLogging,
                      includeUiPreferences,
                      includeTemplates,
                      includeMonitorFilters,
                  )
                }
            )
        else -> element
      }

  private fun stableJson(json: Json, element: JsonElement): String =
      when (element) {
        is JsonObject ->
            element.entries
                .sortedBy { it.key }
                .joinToString(prefix = "{", postfix = "}") { (key, value) ->
                  json.encodeToString(key) + ":" + stableJson(json, value)
                }
        is JsonArray -> element.joinToString(prefix = "[", postfix = "]") { stableJson(json, it) }
        is JsonPrimitive -> element.toString()
      }

  private fun normalizeMonitorFilterList(values: List<String>): List<String> =
      values
          .map { it.trim() }
          .filter { it.isNotBlank() && '\u0000' !in it && it.length <= 512 }
          .distinct()
          .take(200)
          .sortedWith(compareBy<String> { it.lowercase() }.thenBy { it })

  private fun normalizeMonitorFilterOperations(values: List<String>): List<String> {
    val normalized = normalizeMonitorFilterList(values)
    val sorted = normalized.map { it.lowercase() }.sorted()
    return if (
        sorted == LegacyDefaultMonitorOperations.sorted() ||
            sorted == LegacyFullDefaultMonitorOperations.sorted()
    ) {
      FileMonitorFilters().excludedOperations
    } else {
      normalized
    }
  }

  private fun normalizeMonitorFilterPathList(values: List<String>): List<String> =
      values
          .mapNotNull { sanitizeMonitorFilterPathOrNull(it, allowLegacyAbsolute = true) }
          .distinct()
          .take(200)
          .sortedWith(compareBy<String> { it.lowercase() }.thenBy { it })

  private fun sanitizeMonitorFilterPathOrNull(raw: String, allowLegacyAbsolute: Boolean): String? {
    val text = raw.trim().replace('\\', '/').replace(Regex("/+"), "/")
    if (text.isBlank() || text.length > 512 || '\u0000' in text) return null
    if (text.startsWith("!")) return null
    val withoutLeadingSlash = text.trimStart('/')
    if (hasStorageRootPrefix(withoutLeadingSlash)) return null
    if (text.startsWith("/") && !allowLegacyAbsolute) return null
    val path = (if (text.startsWith("/")) withoutLeadingSlash else text).trim('/')
    if (path.isBlank()) return null
    if (path.split('/').any { it == "." || it == ".." }) return null
    if (hasStorageRootPrefix(path)) return null
    val unsafe = Regex("[<>:\"|\\x00-\\x1F]")
    if (unsafe.containsMatchIn(path)) return null
    return path
  }

  private fun hasStorageRootPrefix(path: String): Boolean {
    val lower = path.lowercase()
    return lower == "sdcard" ||
        lower.startsWith("sdcard/") ||
        lower == "storage/emulated" ||
        lower.startsWith("storage/emulated/") ||
        lower == "storage/self/primary" ||
        lower.startsWith("storage/self/primary/") ||
        lower == "data/media" ||
        lower.startsWith("data/media/")
  }

  private fun mergeAllowedRules(allowed: List<String>, excluded: List<String>): List<String> {
    val merged = linkedSetOf<String>()
    allowed.forEach { if (it.isNotBlank()) merged += it.trim() }
    excluded.forEach {
      val value = it.trim()
      if (value.isNotBlank()) merged += if (value.startsWith("!")) value else "!$value"
    }
    return merged.toList()
  }

  private fun normalizeUserProfile(profile: UserProfile): UserProfile {
    val readOnlyPaths =
        sortPathRules(
            profile.readOnlyPaths,
            allowRuleSyntax = true,
            allowWildcards = true,
        )
    val mergedPaths =
        sortPathRules(
            mergeAllowedRules(profile.allowedRealPaths, profile.excludedRealPaths),
            allowRuleSyntax = true,
            allowWildcards = true,
        )
    val conflicts = detectPathRuleConflicts(mergedPaths)
    val cleanedPaths =
        if (conflicts.isEmpty()) {
          mergedPaths
        } else {
          mergedPaths.filterNot { path -> !path.startsWith("!") && path in conflicts }
        }
    val cleanedReadOnlyPaths =
        readOnlyPaths.filter { path -> path.startsWith("!") || "!$path" !in cleanedPaths }

    return profile.copy(
        allowedRealPaths = cleanedPaths,
        excludedRealPaths = emptyList(),
        sandboxedPaths =
            sortPathRules(
                profile.sandboxedPaths,
                allowRuleSyntax = false,
                allowWildcards = false,
            ),
        readOnlyPaths = cleanedReadOnlyPaths,
        pathMappings = normalizePathMappings(profile.pathMappings),
    )
  }

  private fun detectPathRuleConflicts(rules: List<String>): Set<String> {
    val allowed = mutableSetOf<String>()
    val excluded = mutableSetOf<String>()

    rules.forEach { rule ->
      if (rule.startsWith("!")) {
        excluded.add(rule.removePrefix("!"))
      } else {
        allowed.add(rule)
      }
    }

    return allowed.intersect(excluded)
  }

  private fun normalizePathMappings(values: Map<String, String>): Map<String, String> {
    val cleaned =
        values
            .mapNotNull { (request, target) ->
              val cleanRequest =
                  sanitizeConfigPath(request, allowRuleSyntax = false, allowWildcards = false)
              val cleanTarget =
                  sanitizeConfigPath(target, allowRuleSyntax = false, allowWildcards = false)
              if (
                  cleanRequest.isNullOrBlank() ||
                      cleanTarget.isNullOrBlank() ||
                      cleanRequest == cleanTarget ||
                      isAndroidDataOrObbPath(cleanTarget)
              ) {
                null
              } else {
                cleanRequest to cleanTarget
              }
            }
            .toMap()

    val cycles = detectMappingCycles(cleaned)
    val overDepth = detectMappingDepth(cleaned).filterValues { it > MaxPathMappingDepth }.keys
    val invalidSources = cycles + overDepth
    val validMappings =
        if (invalidSources.isEmpty()) {
          cleaned
        } else {
          cleaned.filterKeys { it !in invalidSources }
        }

    return validMappings.toSortedMap(compareBy<String> { it.lowercase() }.thenBy { it })
  }

  private fun detectMappingCycles(mappings: Map<String, String>): Set<String> {
    val cycles = mutableSetOf<String>()
    val visitState = mutableMapOf<String, Int>()
    val stack = mutableListOf<String>()

    fun visit(path: String) {
      when (visitState[path]) {
        1 -> {
          val index = stack.indexOf(path)
          if (index >= 0) cycles += stack.subList(index, stack.size)
          return
        }
        2 -> return
      }

      visitState[path] = 1
      stack += path
      mappings[path]?.let(::visit)
      stack.removeAt(stack.lastIndex)
      visitState[path] = 2
    }

    mappings.keys.forEach(::visit)

    return cycles
  }

  private fun isAndroidDataOrObbPath(path: String): Boolean {
    val parts = path.trim('/').split('/').filter(String::isNotBlank)
    return parts.size >= 2 &&
        parts[0].equals("Android", ignoreCase = true) &&
        (parts[1].equals("data", ignoreCase = true) || parts[1].equals("obb", ignoreCase = true))
  }

  private fun detectMappingDepth(mappings: Map<String, String>): Map<String, Int> {
    val depths = mutableMapOf<String, Int>()

    fun computeDepth(path: String, visited: Set<String> = emptySet()): Int {
      if (path in visited) return MaxPathMappingDepth + 1
      depths[path]?.let {
        return it
      }

      val target = mappings[path] ?: return 0
      val depth = 1 + computeDepth(target, visited + path)
      depths[path] = depth
      return depth
    }

    mappings.keys.forEach { path ->
      if (!depths.containsKey(path)) {
        computeDepth(path)
      }
    }

    return depths
  }

  private fun sortPathRules(
      values: List<String>,
      allowRuleSyntax: Boolean,
      allowWildcards: Boolean = allowRuleSyntax,
  ): List<String> =
      values
          .mapNotNull { sanitizeConfigPath(it, allowRuleSyntax, allowWildcards) }
          .distinct()
          .sortedWith(compareBy({ it.removePrefix("!").lowercase() }, { it }))

  private fun sanitizeConfigPath(
      raw: String,
      allowRuleSyntax: Boolean,
      allowWildcards: Boolean,
  ): String? {
    val text = raw.trim().replace('\\', '/').replace(Regex("/+"), "/")
    if (text.isBlank() || text.length > 512 || '\u0000' in text) return null
    if (!allowRuleSyntax && text.startsWith("!")) return null
    val excluded = allowRuleSyntax && text.startsWith("!")
    var path = if (excluded) text.removePrefix("!").trim() else text
    path =
        path
            .trimStart('/')
            .replace(Regex("^storage/emulated/\\d+/?"), "")
            .replace(Regex("^data/media/\\d+/?"), "")
            .removePrefix("sdcard/")
            .trim('/')
    if (path.isBlank()) return null
    if (path.length > 512) return null
    if (hasStorageRootPrefix(path)) return null
    if (path.split('/').any { it == "." || it == ".." }) return null
    val unsafe =
        if (allowWildcards) {
          Regex("[<>:\"|\\x00-\\x1F]")
        } else {
          Regex("[<>:\"|?*\\x00-\\x1F]")
        }
    if (unsafe.containsMatchIn(path)) return null
    return if (excluded) "!$path" else path
  }
}
