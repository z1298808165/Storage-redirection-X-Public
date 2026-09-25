package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassTextButton
import org.srx.manager.RoundIconAction
import org.srx.manager.data.FileMonitorFilters
import org.srx.manager.data.SrxConfigNormalizer
import org.srx.manager.glassSurfaceColor
import org.srx.manager.srxSuccessColor
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Add
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.Ok
import top.yukonga.miuix.kmp.theme.MiuixTheme

// 文件监视过滤规则的编辑入口。
//
// 这里只负责「规则怎么编辑、怎么校验」，不关心日志列表如何展示；日志页只需要在
// 需要编辑时调用 FileMonitorFilterDialog，避免在 1000+ 行的日志页里跨职责查找。

@Composable
internal fun FileMonitorFilterDialog(
    show: Boolean,
    filters: FileMonitorFilters,
    autoSave: Boolean,
    onDismiss: () -> Unit,
    onSave: (FileMonitorFilters, Boolean) -> Unit,
) {
  var paths by remember(show) { mutableStateOf(filters.excludedPaths) }
  var operationRules by remember(show) { mutableStateOf(filters.excludedOperations) }
  val splitRules = splitMonitorOperationRules(operationRules)
  val operations = splitRules.first
  val intents = splitRules.second
  var selectedType by remember(show) { mutableStateOf(MonitorFilterType.Path) }
  var pathInput by remember(show) { mutableStateOf("") }
  var pathValidation by remember(show) { mutableStateOf<MonitorFilterPathValidation?>(null) }
  var operationInput by remember(show) { mutableStateOf("") }
  var pendingRemoval by remember(show) { mutableStateOf<MonitorFilterRemoval?>(null) }
  fun saveDraft(
      nextPaths: List<String> = paths,
      nextOperationRules: List<String> = operationRules,
      silent: Boolean = true,
  ) {
    onSave(
        FileMonitorFilters(
            excludedPaths = nextPaths,
            excludedOperations = nextOperationRules,
        ),
        silent,
    )
  }
  CenteredDialog(
      show = show,
      onDismiss = onDismiss,
  ) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      Text(
          "文件监视过滤",
          modifier = Modifier.weight(1f),
          fontWeight = FontWeight.Black,
          fontSize = 17.sp,
          lineHeight = 21.sp,
      )
      Text(
          "${paths.size + operations.size + intents.size} 条规则",
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 11.sp,
          fontWeight = FontWeight.Bold,
      )
    }
    MonitorFilterTabs(
        selected = selectedType,
        counts =
            mapOf(
                MonitorFilterType.Path to paths.size,
                MonitorFilterType.Operation to operations.size,
                MonitorFilterType.Intent to intents.size,
            ),
        onSelect = { selectedType = it },
    )
    Text(
        selectedType.description,
        modifier = Modifier.fillMaxWidth().heightIn(min = 34.dp),
        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        fontSize = 12.sp,
        lineHeight = 17.sp,
    )
    when (selectedType) {
      MonitorFilterType.Path ->
          MonitorFilterEditor(
              placeholder = "Download 或 Android/cache",
              value = pathInput,
              values = paths,
              onValue = {
                pathInput = it
                pathValidation = validateMonitorFilterPathInput(it)
              },
              onAdd = {
                val result = validateMonitorFilterPathInput(pathInput)
                pathValidation = result
                val value = result.value
                when {
                  !result.valid -> Unit
                  value in paths -> pathValidation = result.copy(valid = false, message = "规则已存在")
                  else -> {
                    val nextPaths = sortMonitorFilterValues(paths + value)
                    paths = nextPaths
                    pathInput = ""
                    pathValidation = null
                    if (autoSave) saveDraft(nextPaths = nextPaths)
                  }
                }
              },
              onRemove = { pendingRemoval = MonitorFilterRemoval(MonitorFilterType.Path, it) },
              validationText = if (pathInput.isBlank()) "" else pathValidation?.message.orEmpty(),
              validationError = pathValidation?.valid == false,
          )
      MonitorFilterType.Operation ->
          MonitorFilterEditor(
              placeholder = "open* 或 open*:read",
              value = operationInput,
              values = operations,
              onValue = { operationInput = it },
              onAdd = {
                val value = operationInput.trim()
                if (value.isNotBlank() && value.length <= 512 && value !in operations) {
                  val nextRules = sortMonitorFilterValues(operationRules + value)
                  operationRules = nextRules
                  operationInput = ""
                  if (autoSave) saveDraft(nextOperationRules = nextRules)
                }
              },
              onRemove = { pendingRemoval = MonitorFilterRemoval(MonitorFilterType.Operation, it) },
          )
      MonitorFilterType.Intent ->
          MonitorIntentEditor(
              selected = intents,
              onToggle = { intent ->
                if (intent in intents) {
                  pendingRemoval = MonitorFilterRemoval(MonitorFilterType.Intent, intent)
                } else {
                  val nextRules = sortMonitorFilterValues(operationRules + "*:$intent")
                  operationRules = nextRules
                  if (autoSave) saveDraft(nextOperationRules = nextRules)
                }
              },
          )
    }
    if (!autoSave) {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton(
            "保存",
            { saveDraft(silent = false) },
            modifier = Modifier.weight(1f),
            primary = true,
        )
      }
    }
  }
  pendingRemoval?.let { removal ->
    CenteredDialog(
        title = "删除过滤规则",
        summary = "确认删除过滤规则“${removal.displayValue}”？",
        show = show,
        onDismiss = { pendingRemoval = null },
    ) {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        GlassTextButton("取消", { pendingRemoval = null }, modifier = Modifier.weight(1f))
        GlassTextButton(
            "删除",
            {
              when (removal.type) {
                MonitorFilterType.Path -> {
                  val nextPaths = paths - removal.value
                  paths = nextPaths
                  if (autoSave) saveDraft(nextPaths = nextPaths)
                }
                MonitorFilterType.Operation -> {
                  val nextRules = operationRules - removal.value
                  operationRules = nextRules
                  if (autoSave) saveDraft(nextOperationRules = nextRules)
                }
                MonitorFilterType.Intent -> {
                  val nextRules = operationRules - "*:${removal.value}"
                  operationRules = nextRules
                  if (autoSave) saveDraft(nextOperationRules = nextRules)
                }
              }
              pendingRemoval = null
            },
            modifier = Modifier.weight(1f),
            danger = true,
        )
      }
    }
  }
}

private enum class MonitorFilterType(val label: String, val description: String) {
  Path("路径", "排除目录及其子路径，支持 * 和 ? 通配。"),
  Operation("操作", "按操作名或模式过滤，支持 *、? 和意图后缀，例如 open*、open*:read。"),
  Intent("意图", "按访问目的过滤，不受 open、openat 等具体操作名影响。"),
}

private data class MonitorFilterRemoval(val type: MonitorFilterType, val value: String) {
  val displayValue: String
    get() = if (type == MonitorFilterType.Intent) monitorIntentLabel(value) else value
}

private val MonitorIntents = listOf("read", "write", "create")

private fun sortMonitorFilterValues(values: List<String>): List<String> =
    values.sortedWith(compareBy<String> { it.lowercase() }.thenBy { it })

private fun splitMonitorOperationRules(values: List<String>): Pair<List<String>, List<String>> {
  val intents = mutableListOf<String>()
  val operations = mutableListOf<String>()
  values.forEach { value ->
    val match = Regex("^\\*:(read|write|create)$", RegexOption.IGNORE_CASE).matchEntire(value)
    if (match == null) operations += value else intents += match.groupValues[1].lowercase()
  }
  return operations to intents.distinct()
}

internal fun monitorIntentLabel(intent: String): String =
    when (intent) {
      "read" -> "读取意图"
      "write" -> "写入意图"
      "create" -> "创建意图"
      else -> intent
    }

@Composable
private fun MonitorFilterTabs(
    selected: MonitorFilterType,
    counts: Map<MonitorFilterType, Int>,
    onSelect: (MonitorFilterType) -> Unit,
) {
  val outerShape = RoundedCornerShape(15.dp)
  val dark = isSrxDarkTheme()
  Row(
      modifier =
          Modifier.fillMaxWidth()
              .clip(outerShape)
              .background(glassSurfaceColor(0.56f), outerShape)
              .border(
                  1.dp,
                  MiuixTheme.colorScheme.onSurface.copy(alpha = 0.07f),
                  outerShape,
              )
              .padding(4.dp),
      horizontalArrangement = Arrangement.spacedBy(4.dp),
  ) {
    MonitorFilterType.entries.forEach { type ->
      val active = type == selected
      val itemShape = RoundedCornerShape(11.dp)
      Row(
          modifier =
              Modifier.weight(1f)
                  .height(38.dp)
                  .then(
                      if (active) {
                        Modifier.dropShadow(
                            itemShape,
                            Shadow(
                                radius = 10.dp,
                                color = MiuixTheme.colorScheme.primary,
                                alpha = if (dark) 0.18f else 0.1f,
                            ),
                        )
                      } else {
                        Modifier
                      }
                  )
                  .clip(itemShape)
                  .background(
                      if (active) {
                        MiuixTheme.colorScheme.primary.copy(alpha = if (dark) 0.22f else 0.13f)
                      } else {
                        Color.Transparent
                      },
                      itemShape,
                  )
                  .border(
                      1.dp,
                      if (active) MiuixTheme.colorScheme.primary.copy(alpha = 0.38f)
                      else Color.Transparent,
                      itemShape,
                  )
                  .clickable { onSelect(type) }
                  .padding(horizontal = 7.dp),
          verticalAlignment = Alignment.CenterVertically,
          horizontalArrangement = Arrangement.Center,
      ) {
        Text(
            type.label,
            color =
                if (active) MiuixTheme.colorScheme.primary
                else MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 12.sp,
            fontWeight = FontWeight.Black,
        )
        Spacer(Modifier.size(5.dp))
        Box(
            modifier =
                Modifier.clip(RoundedCornerShape(6.dp))
                    .background(
                        if (active) MiuixTheme.colorScheme.primary.copy(alpha = 0.12f)
                        else glassSurfaceColor(0.7f),
                    )
                    .padding(horizontal = 5.dp, vertical = 2.dp),
            contentAlignment = Alignment.Center,
        ) {
          Text(
              "${counts[type] ?: 0}",
              color =
                  if (active) MiuixTheme.colorScheme.primary
                  else MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 9.sp,
              lineHeight = 11.sp,
              fontWeight = FontWeight.Black,
          )
        }
      }
    }
  }
}

private data class MonitorFilterPathValidation(
    val value: String,
    val valid: Boolean,
    val message: String,
)

private fun validateMonitorFilterPathInput(raw: String): MonitorFilterPathValidation {
  val text = raw.trim()
  if (text.isBlank()) return MonitorFilterPathValidation("", false, "路径不能为空")
  val normalized = SrxConfigNormalizer.sanitizeMonitorFilterPath(text, allowLegacyAbsolute = false)
  if (normalized.isNotBlank()) return MonitorFilterPathValidation(normalized, true, "路径格式正确")
  val message =
      when {
        text.trimStart('/').lowercase().let(::hasStorageRootPrefixForMonitorFilter) ->
            "不能带存储根目录，请输入相对路径"
        text.startsWith("/") -> "不能使用绝对路径，请输入相对路径"
        text.startsWith("!") -> "过滤路径不支持排除前缀"
        text.length > 512 || '\u0000' in text -> "路径格式不正确"
        text.contains("..") -> "路径不能包含 . 或 .."
        else -> "路径包含非法字符"
      }
  return MonitorFilterPathValidation("", false, message)
}

private fun hasStorageRootPrefixForMonitorFilter(path: String): Boolean {
  val lower = path.replace('\\', '/').trimStart('/').lowercase()
  return lower == "sdcard" ||
      lower.startsWith("sdcard/") ||
      lower == "storage/emulated" ||
      lower.startsWith("storage/emulated/") ||
      lower == "storage/self/primary" ||
      lower.startsWith("storage/self/primary/") ||
      lower == "data/media" ||
      lower.startsWith("data/media/")
}

@Composable
private fun MonitorFilterEditor(
    placeholder: String,
    value: String,
    values: List<String>,
    onValue: (String) -> Unit,
    onAdd: () -> Unit,
    onRemove: (String) -> Unit,
    validationText: String = "",
    validationError: Boolean = false,
) {
  Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
      TextField(
          value = value,
          onValueChange = onValue,
          label = placeholder,
          modifier = Modifier.weight(1f),
          insideMargin = DpSize(14.dp, 10.dp),
          colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
          useLabelAsPlaceholder = true,
          singleLine = true,
      )
      RoundIconAction(
          icon = MiuixIcons.Add,
          contentDescription = "添加规则",
          onClick = onAdd,
          size = 36.dp,
          iconSize = 17.dp,
      )
    }
    Box(
        modifier = Modifier.fillMaxWidth().height(16.dp),
        contentAlignment = Alignment.CenterStart,
    ) {
      if (validationText.isNotBlank()) {
        Text(
            validationText,
            color = if (validationError) MiuixTheme.colorScheme.error else srxSuccessColor(),
            fontSize = 11.sp,
            lineHeight = 14.sp,
            modifier = Modifier.padding(start = 2.dp),
        )
      }
    }
    LazyColumn(
        modifier = Modifier.fillMaxWidth().heightIn(min = 42.dp, max = 176.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
      if (values.isEmpty()) {
        item {
          Text(
              "未添加规则",
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 12.sp,
              modifier = Modifier.padding(vertical = 6.dp),
          )
        }
      } else {
        items(values, key = { it }) { item -> MonitorFilterChipRow(item, onRemove) }
      }
    }
  }
}

@Composable
private fun MonitorFilterChipRow(value: String, onRemove: (String) -> Unit) {
  Row(
      modifier =
          Modifier.fillMaxWidth()
              .height(42.dp)
              .clip(RoundedCornerShape(14.dp))
              .background(glassSurfaceColor(0.58f), RoundedCornerShape(14.dp))
              .padding(horizontal = 10.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    Text(
        value,
        modifier = Modifier.weight(1f),
        fontSize = 12.sp,
        lineHeight = 16.sp,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
    IconButton(
        modifier = Modifier.size(28.dp),
        onClick = { onRemove(value) },
    ) {
      Icon(
          MiuixIcons.Delete,
          contentDescription = "删除规则",
          tint = MiuixTheme.colorScheme.error,
          modifier = Modifier.size(15.dp),
      )
    }
  }
}

@Composable
private fun MonitorIntentEditor(selected: List<String>, onToggle: (String) -> Unit) {
  Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
    MonitorIntents.forEach { intent ->
      val active = intent in selected
      val shape = RoundedCornerShape(14.dp)
      Row(
          modifier =
              Modifier.fillMaxWidth()
                  .height(54.dp)
                  .clip(shape)
                  .background(
                      if (active) MiuixTheme.colorScheme.primary.copy(alpha = 0.11f)
                      else glassSurfaceColor(0.58f),
                      shape,
                  )
                  .border(
                      1.dp,
                      if (active) MiuixTheme.colorScheme.primary.copy(alpha = 0.32f)
                      else MiuixTheme.colorScheme.onSurface.copy(alpha = 0.07f),
                      shape,
                  )
                  .clickable { onToggle(intent) }
                  .padding(horizontal = 12.dp),
          verticalAlignment = Alignment.CenterVertically,
          horizontalArrangement = Arrangement.spacedBy(10.dp),
      ) {
        Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
          Text(
              monitorIntentLabel(intent),
              color =
                  if (active) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.onSurface,
              fontWeight = FontWeight.Black,
              fontSize = 13.sp,
          )
          Text(
              when (intent) {
                "read" -> "仅读取现有内容"
                "write" -> "写入或追加内容"
                else -> "新建、覆盖或临时文件"
              },
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 11.sp,
          )
        }
        Icon(
            imageVector = if (active) MiuixIcons.Ok else MiuixIcons.Add,
            contentDescription = if (active) "已添加" else "添加",
            tint = MiuixTheme.colorScheme.primary,
            modifier = Modifier.size(18.dp),
        )
      }
    }
  }
}
