package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.glassSurfaceColor
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

internal data class PathSuggestion(
    val relativePath: String,
    val displayPath: String,
    val isParent: Boolean = false,
    val isDirectory: Boolean = true,
)

@Composable
internal fun PathSuggestionBrowser(
    value: String,
    userId: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
    onPick: (String) -> Unit,
) {
    val browserState = rememberStorageBrowserState(userId, value, onListDirectories)
    val suggestions = browserState.suggestions
    val shape = RoundedCornerShape(20.dp)
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .height(188.dp)
            .clip(shape)
            .background(glassSurfaceColor(0.56f)),
    ) {
        when {
            browserState.loading -> PathBrowserMessage("正在读取目录...")
            suggestions.isEmpty() -> PathBrowserMessage("没有可补全的下一级路径")
            else -> LazyColumn(modifier = Modifier.fillMaxSize()) {
                itemsIndexed(suggestions) { index, suggestion ->
                    Column {
                        Row(
                            modifier = Modifier
                                .fillMaxWidth()
                                .clickable { onPick(applyPathPrefix(value, suggestion.relativePath)) }
                                .padding(horizontal = 12.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            PathBrowserIcon(isDirectory = suggestion.isDirectory)
                            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                                Text(
                                    text = suggestion.displayPath,
                                    color = MiuixTheme.colorScheme.onSurface,
                                    fontSize = 13.sp,
                                    lineHeight = 17.sp,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                if (!suggestion.isParent && suggestion.relativePath != suggestion.displayPath) {
                                    Text(
                                        text = suggestion.relativePath,
                                        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                                        fontSize = 11.sp,
                                        lineHeight = 14.sp,
                                        maxLines = 2,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                }
                            }
                        }
                        if (index != suggestions.lastIndex) {
                            Box(
                                Modifier
                                    .fillMaxWidth()
                                    .height(1.dp)
                                    .background(MiuixTheme.colorScheme.onSurface.copy(alpha = if (isSrxDarkTheme()) 0.03f else 0.04f)),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun PathBrowserMessage(text: String) {
    Box(
        modifier = Modifier.fillMaxSize(),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 12.sp,
            lineHeight = 18.sp,
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
private fun PathBrowserIcon(isDirectory: Boolean) {
    val color = if (isDirectory) {
        MiuixTheme.colorScheme.primary
    } else {
        MiuixTheme.colorScheme.onSurfaceVariantSummary
    }
    Box(
        modifier = Modifier
            .size(16.dp)
            .drawBehind {
                val stroke = Stroke(width = 2.dp.toPx())
                val radius = 4.dp.toPx()
                drawRoundRect(
                    color = color,
                    size = size,
                    cornerRadius = CornerRadius(radius, radius),
                    style = stroke,
                    alpha = if (isDirectory) 0.95f else 0.72f,
                )
                if (isDirectory) {
                    drawRoundRect(
                        color = color,
                        topLeft = Offset(3.dp.toPx(), 3.dp.toPx()),
                        size = Size(7.dp.toPx(), 2.dp.toPx()),
                        cornerRadius = CornerRadius(1.dp.toPx(), 1.dp.toPx()),
                        alpha = 0.95f,
                    )
                } else {
                    drawRoundRect(
                        color = color,
                        topLeft = Offset(4.dp.toPx(), 5.dp.toPx()),
                        size = Size(8.dp.toPx(), 2.dp.toPx()),
                        cornerRadius = CornerRadius(1.dp.toPx(), 1.dp.toPx()),
                        alpha = 0.72f,
                    )
                    drawRoundRect(
                        color = color,
                        topLeft = Offset(4.dp.toPx(), 9.dp.toPx()),
                        size = Size(6.dp.toPx(), 2.dp.toPx()),
                        cornerRadius = CornerRadius(1.dp.toPx(), 1.dp.toPx()),
                        alpha = 0.72f,
                    )
                }
            },
    )
}

@Composable
private fun rememberStorageBrowserState(
    userId: String,
    value: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
): PathBrowserState {
    val parsed = remember(userId, value) { splitPathBrowserInput(value, userId) }
    var entries by remember(userId, parsed.dirRel) { mutableStateOf<List<String>?>(null) }
    LaunchedEffect(userId, parsed.dirRel) {
        entries = null
        onListDirectories(userId, parsed.dirRel) { entries = it }
    }
    return remember(parsed, entries) {
        PathBrowserState(
            suggestions = pathBrowserSuggestions(parsed, entries.orEmpty()),
            loading = entries == null,
        )
    }
}

internal data class PathBrowserInput(
    val dirRel: String,
    val prefix: String,
    val query: String,
)

private data class PathBrowserState(
    val suggestions: List<PathSuggestion>,
    val loading: Boolean,
)

internal enum class MappingField {
    From,
    To,
}

internal fun splitPathBrowserInput(value: String, userId: String): PathBrowserInput {
    val clean = normalizeSuggestionInput(value, userId)
    if (clean.isBlank() || clean.endsWith('/')) {
        val dirRel = clean.trimEnd('/')
        return PathBrowserInput(dirRel = dirRel, prefix = clean, query = "")
    }
    if (clean.equals("Android", ignoreCase = true)) {
        return PathBrowserInput(
            dirRel = "Android",
            prefix = "Android/",
            query = "",
        )
    }
    val slash = clean.lastIndexOf('/')
    if (slash < 0) return PathBrowserInput(dirRel = "", prefix = "", query = clean.lowercase())
    val dirRel = clean.substring(0, slash)
    return PathBrowserInput(
        dirRel = dirRel,
        prefix = clean.substring(0, slash + 1),
        query = clean.substring(slash + 1).lowercase(),
    )
}

internal fun pathBrowserSuggestions(parsed: PathBrowserInput, entries: List<String>): List<PathSuggestion> {
    if (parsed.dirRel.isAndroidDataPrivateBrowserPath()) {
        val parentRel = parsed.dirRel.substringBeforeLast('/', missingDelimiterValue = "")
            .let { if (it.isBlank()) "" else "$it/" }
        return listOf(PathSuggestion(relativePath = parentRel, displayPath = "..", isParent = true))
    }
    val baseSuggestions = if (parsed.dirRel.isBlank()) {
        entries.distinctBy { it.trimEnd('/').lowercase() }
    } else {
        entries
    }
    val parent = if (parsed.dirRel.isBlank()) {
        emptyList()
    } else {
        val parentRel = parsed.dirRel.substringBeforeLast('/', missingDelimiterValue = "")
            .let { if (it.isBlank()) "" else "$it/" }
        listOf(PathSuggestion(relativePath = parentRel, displayPath = "..", isParent = true))
    }
    return parent + baseSuggestions
        .filter { suggestion ->
            val name = suggestion.trimEnd('/')
            parsed.query.isBlank() || name.contains(parsed.query, ignoreCase = true)
        }
        .filterNot { suggestion ->
            val relative = parsed.prefix + suggestion
            relative.equals(parsed.prefix, ignoreCase = true) ||
                relative.trimEnd('/').equals(parsed.prefix.trimEnd('/'), ignoreCase = true)
        }
        .map { entry ->
            val isDirectory = entry.endsWith("/")
            val name = entry.trimEnd('/')
            val relativePath = parsed.prefix + name + if (isDirectory) "/" else ""
            PathSuggestion(
                relativePath = relativePath,
                displayPath = name,
                isDirectory = isDirectory,
            )
        }
}

private fun normalizeSuggestionInput(value: String, userId: String): String {
    val currentUserRoot = "storage/emulated/$userId/"
    val anyUserRoot = Regex("^storage/emulated/\\d+/")
    val currentDataRoot = "data/media/$userId/"
    val anyDataRoot = Regex("^data/media/\\d+/")
    return value.trim()
        .removePrefix("!")
        .replace('\\', '/')
        .trimStart('/')
        .removePrefix(currentUserRoot)
        .removePrefix(currentDataRoot)
        .removePrefix("sdcard/")
        .replace(anyUserRoot, "")
        .replace(anyDataRoot, "")
}

private fun String.isAndroidDataPrivateBrowserPath(): Boolean {
    val clean = trim('/').lowercase()
    return clean == "android/data" || clean.startsWith("android/data/")
}

private fun hasAllowRulePrefix(value: String): Boolean = value.trimStart().startsWith("!")

internal fun normalizeEditablePathInput(value: String, userId: String, allowRuleSyntax: Boolean = false): String {
    val excluded = allowRuleSyntax && hasAllowRulePrefix(value)
    val clean = normalizeSuggestionInput(value, userId).trimStart('/')
    return if (excluded) "!$clean" else clean
}

internal fun normalizeEditablePathTextFieldValue(
    value: TextFieldValue,
    userId: String,
    allowRuleSyntax: Boolean = false,
): TextFieldValue {
    val normalized = normalizeEditablePathInput(value.text, userId, allowRuleSyntax)
    if (normalized == value.text) return value
    return TextFieldValue(normalized, selection = value.selection.clampToText(normalized.length))
}

private fun applyPathPrefix(current: String, suggestion: String): String =
    if (hasAllowRulePrefix(current)) "!$suggestion" else suggestion

internal fun pathTextFieldValue(text: String, cursor: Int = text.length): TextFieldValue =
    TextFieldValue(text, selection = TextRange(cursor.coerceIn(0, text.length)))

private fun TextRange.clampToText(textLength: Int): TextRange =
    TextRange(start.coerceIn(0, textLength), end.coerceIn(0, textLength))
