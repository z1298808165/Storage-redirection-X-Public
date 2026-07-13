package org.srx.manager.data

enum class ReleaseNoteComponent(val title: String) {
  Module("模块更新"),
  App("App 更新"),
  Other("其它更新"),
}

data class ReleaseNoteSection(
    val component: ReleaseNoteComponent,
    val markdown: String,
)

fun parseReleaseNoteSections(markdown: String): List<ReleaseNoteSection> {
  val sanitized = sanitizeReleaseNotes(markdown)
  if (sanitized.isBlank()) return emptyList()
  val sectionHeading =
      Regex(
          pattern = "(?im)^##\\s*(模块更新|App\\s*更新|其它更新|其他更新)\\s*$",
      )
  val matches = sectionHeading.findAll(sanitized).toList()
  if (matches.isEmpty()) {
    return listOf(ReleaseNoteSection(ReleaseNoteComponent.Other, sanitized))
  }
  return matches.mapIndexedNotNull { index, match ->
    val component =
        when (match.groupValues[1].replace(" ", "").lowercase()) {
          "模块更新" -> ReleaseNoteComponent.Module
          "app更新" -> ReleaseNoteComponent.App
          else -> ReleaseNoteComponent.Other
        }
    val nextStart = matches.getOrNull(index + 1)?.range?.first
    val body = sanitized.substring(match.range.last + 1, nextStart ?: sanitized.length).trim()
    body.takeIf(String::isNotBlank)?.let { ReleaseNoteSection(component, it) }
  }
}

fun sanitizeReleaseNotes(markdown: String): String {
  var normalized = markdown.replace("\r\n", "\n").replace('\r', '\n').trim()
  val commitHeading = Regex("(?im)^#{1,6}\\s*提交列表\\s*$").find(normalized)
  if (commitHeading != null)
      normalized = normalized.substring(0, commitHeading.range.first).trimEnd()
  normalized =
      normalized
          .replace(
              Regex("(?im)^\\*\\*完整变更对比\\*\\*\\s*:\\s*https?://\\S+\\s*$"),
              "",
          )
          .trim()
  return normalized
}
