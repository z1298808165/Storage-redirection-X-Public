package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.EmptyText
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.PageHeader
import org.srx.manager.RoundIconAction
import org.srx.manager.data.FileMonitorFilters
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.LogEntry
import org.srx.manager.data.SrxConfigNormalizer
import org.srx.manager.glassSurfaceColor
import org.srx.manager.root.isSafePackageName
import org.srx.manager.srxSuccessColor
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.component.AppIconImage
import org.srx.manager.ui.component.SrxSearchField
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.PullToRefresh
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.basic.rememberPullToRefreshState
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Add
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.File
import top.yukonga.miuix.kmp.icon.extended.Ok
import top.yukonga.miuix.kmp.icon.extended.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun LogsScreen(
    state: AppUiState,
    logs: List<LogEntry>,
    apps: List<InstalledApp>,
    listState: LazyListState,
    bottomPadding: Dp,
    onRefresh: () -> Unit,
    onClear: () -> Unit,
    onOpenApp: (InstalledApp) -> Unit,
    onSaveFilters: (FileMonitorFilters, Boolean) -> Unit,
) {
  var query by remember { mutableStateOf("") }
  var confirmClear by remember { mutableStateOf(false) }
  var showFilters by remember { mutableStateOf(false) }
  var showFullTime by rememberSaveable { mutableStateOf(false) }
  val appsByPackage = remember(apps) { apps.associateBy { it.packageName } }
  val filtered =
      remember(logs, query) {
        val q = query.trim().lowercase()
        if (q.isBlank()) logs
        else
            logs.filter {
              listOf(
                      it.label,
                      it.packageName,
                      it.processPackage,
                      it.callerPackage,
                      it.watchPackage,
                      it.operation,
                      it.operationIntent,
                      it.action,
                      it.errorText,
                      it.path,
                      it.landingPath,
                      it.fromPath,
                      it.backendPath,
                  )
                  .any { value -> value.lowercase().contains(q) }
            }
      }
  val pullToRefreshState = rememberPullToRefreshState()
  val refreshTexts = listOf("下拉刷新", "释放刷新", "正在刷新", "刷新完成")
  Column(
      modifier =
          Modifier.fillMaxSize()
              .padding(
                  top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 18.dp,
                  start = 16.dp,
                  end = 16.dp,
              ),
  ) {
    PageHeader(
        title = "文件监视",
        actions = {
          RoundIconAction(
              MiuixIcons.Tune,
              "文件监视过滤",
              { showFilters = true },
              size = 36.dp,
              iconSize = 17.dp,
          )
          RoundIconAction(MiuixIcons.Delete, "清空文件监视记录", { confirmClear = true }, danger = true)
        },
    )
    Spacer(Modifier.height(14.dp))
    SrxSearchField(query, { query = it }, "搜索应用名、包名或路径")
    Spacer(Modifier.height(10.dp))
    Text(
        text = if (query.isBlank()) "共 ${logs.size} 条" else "匹配 ${filtered.size} / ${logs.size} 条",
        modifier = Modifier.fillMaxWidth(),
        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        fontSize = 11.sp,
        textAlign = TextAlign.End,
    )
    Spacer(Modifier.height(12.dp))
    PullToRefresh(
        isRefreshing = state.logsRefreshing,
        pullToRefreshState = pullToRefreshState,
        onRefresh = onRefresh,
        refreshTexts = refreshTexts,
        modifier = Modifier.fillMaxWidth().weight(1f),
    ) {
      LazyColumn(
          state = listState,
          modifier = Modifier.fillMaxSize().overScrollVertical(),
          contentPadding = PaddingValues(bottom = bottomPadding + 28.dp),
          verticalArrangement = Arrangement.spacedBy(12.dp),
          overscrollEffect = null,
      ) {
        if (filtered.isEmpty()) {
          item { EmptyText(if (query.isBlank()) "暂无文件操作记录" else "没有匹配的日志") }
        } else {
          itemsIndexed(
              filtered,
              key = { index, entry ->
                "${entry.timestamp}|${entry.processPackage}|${entry.callerPackage}|${entry.packageName}|${entry.path}|$index"
              },
          ) { _, entry ->
            val app =
                if (entry.isModuleWebUiExport) null
                else
                    appsByPackage[entry.packageName]
                        ?: appsByPackage[entry.callerPackage]
                        ?: appsByPackage[entry.watchPackage]
            LogCard(
                entry = entry,
                app = app,
                showFullTime = showFullTime,
                onToggleTime = { showFullTime = !showFullTime },
                onOpenApp = onOpenApp,
            )
          }
        }
      }
    }
  }
  CenteredDialog(
      title = "清空文件监视记录",
      summary = "确认清空当前文件监视记录？此操作会清空模块日志文件。",
      show = confirmClear,
      onDismiss = { confirmClear = false },
  ) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
      GlassTextButton("取消", { confirmClear = false }, modifier = Modifier.weight(1f))
      GlassTextButton(
          "清空",
          {
            confirmClear = false
            onClear()
          },
          modifier = Modifier.weight(1f),
          danger = true,
      )
    }
  }
  FileMonitorFilterDialog(
      show = showFilters,
      filters = state.fileMonitorFilters,
      autoSave = state.dashboard.globalConfig.appConfigAutoSave,
      onDismiss = { showFilters = false },
      onSave = { filters, silent ->
        if (!silent) showFilters = false
        onSaveFilters(filters, silent)
      },
  )
}

@Composable
private fun LogCard(
    entry: LogEntry,
    app: InstalledApp?,
    showFullTime: Boolean,
    onToggleTime: () -> Unit,
    onOpenApp: (InstalledApp) -> Unit,
) {
  var expanded by
      remember(entry.timestamp, entry.packageName, entry.path, entry.landingPath) {
        mutableStateOf(false)
      }
  val displayName =
      if (entry.isModuleWebUiExport) {
        entry.label.ifBlank { "存储重定向X" }
      } else {
        app?.label
            ?: entry.label.takeIf { it.isNotBlank() && it != entry.packageName }
            ?: entry.packageName.ifBlank { "未知应用" }
      }
  val openTarget =
      if (entry.isModuleWebUiExport) null else app ?: entry.toInstalledAppOrNull(displayName)
  val summary = logEntrySummary(entry)
  val primaryPath = logEntryPrimaryPath(entry)
  val requestPath = logEntryRequestPath(entry)
  val actualPath = entry.backendPath
  val canExpand =
      summary.isNotBlank() ||
          primaryPath.length > 48 ||
          requestPath.isNotBlank() ||
          actualPath.isNotBlank() ||
          (!entry.ok && entry.errorText.isNotBlank())
  GlassCard(
      modifier = Modifier,
      insideMargin = PaddingValues(horizontal = 15.dp, vertical = 15.dp),
      cornerRadius = 22.dp,
      alpha = 0.58f,
      shadowAlpha = 0f,
  ) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
      Row(
          modifier = Modifier.fillMaxWidth(),
          verticalAlignment = Alignment.CenterVertically,
          horizontalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        LogAppIdentityAction(
            app = openTarget,
            displayName = displayName,
            modifier = Modifier.weight(1f),
            onOpenApp = onOpenApp,
        )
        LogOperationBadge(entry.operation, entry.ok)
        LogTimeText(
            text = formatLogEntryTime(entry, showFullTime),
            showFullTime = showFullTime,
            onClick = onToggleTime,
        )
        PathExpandButton(
            expanded = expanded,
            enabled = canExpand,
            onClick = { expanded = !expanded },
        )
      }
      if (summary.isNotBlank()) {
        Text(
            summary,
            maxLines = if (expanded) Int.MAX_VALUE else 1,
            overflow = TextOverflow.Ellipsis,
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 12.sp,
            lineHeight = 16.sp,
        )
      }
      Text(
          primaryPath.ifBlank { "未解析到路径" },
          maxLines = if (expanded) Int.MAX_VALUE else 2,
          overflow = TextOverflow.Ellipsis,
          fontSize = 12.sp,
          lineHeight = 17.sp,
      )
      if (expanded && actualPath.isNotBlank() && actualPath != primaryPath) {
        Text(
            "实际路径：$actualPath",
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 12.sp,
            lineHeight = 16.sp,
            maxLines = 3,
            overflow = TextOverflow.Ellipsis,
        )
      }
      if (expanded && requestPath.isNotBlank() && requestPath != primaryPath) {
        Text(
            "请求路径：$requestPath",
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 12.sp,
            lineHeight = 16.sp,
            maxLines = 3,
            overflow = TextOverflow.Ellipsis,
        )
      }
      if (expanded && !entry.ok && entry.errorText.isNotBlank()) {
        Text(
            entry.errorText,
            color = MiuixTheme.colorScheme.error,
            fontSize = 12.sp,
            lineHeight = 16.sp,
            fontWeight = FontWeight.Bold,
        )
      }
    }
  }
}

@Composable
private fun LogAppIdentityAction(
    app: InstalledApp?,
    displayName: String,
    modifier: Modifier = Modifier,
    onOpenApp: (InstalledApp) -> Unit,
) {
  Row(
      modifier =
          modifier.clip(RoundedCornerShape(12.dp)).clickable(enabled = app != null) {
            app?.let(onOpenApp)
          },
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(8.dp),
  ) {
    AppIconImage(
        appInfo = app?.appInfo,
        label = displayName,
        modifier = Modifier.size(34.dp),
    )
    Text(
        text = displayName,
        modifier = Modifier.weight(1f),
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        fontWeight = FontWeight.Black,
        fontSize = 15.sp,
    )
  }
}

private fun LogEntry.toInstalledAppOrNull(displayName: String): InstalledApp? {
  val targetPackage = packageName.takeIf(::isSafePackageName) ?: return null
  return InstalledApp(
      packageName = targetPackage,
      label = displayName.ifBlank { targetPackage },
      isSystem = false,
      appInfo = null,
      config = null,
      isInstalled = false,
  )
}

private fun formatLogEntryTime(entry: LogEntry, showFullTime: Boolean): String {
  if (!showFullTime) return entry.timeText.ifBlank { "--:--" }
  val timestamp = entry.timestamp.replace('T', ' ')
  return when {
    timestamp.length >= 16 -> timestamp.substring(0, 16)
    timestamp.isNotBlank() -> timestamp
    else -> entry.timeText.ifBlank { "--:--" }
  }
}

private fun logEntryPrimaryPath(entry: LogEntry): String {
  val backend = entry.backendPath
  if (
      backend.isNotBlank() &&
          entry.source in setOf("sandbox_path", "redirect_root", "fuse_redirect")
  ) {
    return backend
  }
  return entry.path.ifBlank { entry.landingPath }
}

private fun logEntryRequestPath(entry: LogEntry): String =
    entry.fromPath.ifBlank {
      if (entry.backendPath.isNotBlank()) entry.landingPath.ifBlank { entry.path } else ""
    }

@Composable
private fun FileMonitorFilterDialog(
    show: Boolean,
    filters: FileMonitorFilters,
    autoSave: Boolean,
    onDismiss: () -> Unit,
    onSave: (FileMonitorFilters, Boolean) -> Unit,
) {
  val initialRules = remember(show) { splitMonitorOperationRules(filters.excludedOperations) }
  var paths by remember(show) { mutableStateOf(filters.excludedPaths) }
  var operations by remember(show) { mutableStateOf(initialRules.first) }
  var intents by remember(show) { mutableStateOf(initialRules.second) }
  var selectedType by remember(show) { mutableStateOf(MonitorFilterType.Path) }
  var pathInput by remember(show) { mutableStateOf("") }
  var pathValidation by remember(show) { mutableStateOf<MonitorFilterPathValidation?>(null) }
  var operationInput by remember(show) { mutableStateOf("") }
  var pendingRemoval by remember(show) { mutableStateOf<MonitorFilterRemoval?>(null) }
  fun saveDraft(
      nextPaths: List<String> = paths,
      nextOperations: List<String> = operations,
      nextIntents: List<String> = intents,
      silent: Boolean = true,
  ) {
    onSave(
        FileMonitorFilters(
            excludedPaths = nextPaths,
            excludedOperations = mergeMonitorOperationRules(nextOperations, nextIntents),
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
                    val nextPaths = paths + value
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
                  val nextOperations = operations + value
                  operations = nextOperations
                  operationInput = ""
                  if (autoSave) saveDraft(nextOperations = nextOperations)
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
                  val nextIntents = intents + intent
                  intents = nextIntents
                  if (autoSave) saveDraft(nextIntents = nextIntents)
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
                  val nextOperations = operations - removal.value
                  operations = nextOperations
                  if (autoSave) saveDraft(nextOperations = nextOperations)
                }
                MonitorFilterType.Intent -> {
                  val nextIntents = intents - removal.value
                  intents = nextIntents
                  if (autoSave) saveDraft(nextIntents = nextIntents)
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

private fun splitMonitorOperationRules(values: List<String>): Pair<List<String>, List<String>> {
  val intents = mutableListOf<String>()
  val operations = mutableListOf<String>()
  values.forEach { value ->
    val match = Regex("^\\*:(read|write|create)$", RegexOption.IGNORE_CASE).matchEntire(value)
    if (match == null) operations += value else intents += match.groupValues[1].lowercase()
  }
  return operations to intents.distinct()
}

private fun mergeMonitorOperationRules(
    operations: List<String>,
    intents: List<String>,
): List<String> = (operations + intents.map { "*:$it" }).distinct()

private fun monitorIntentLabel(intent: String): String =
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

@Composable
private fun LogTimeText(
    text: String,
    showFullTime: Boolean,
    onClick: () -> Unit,
) {
  Text(
      text = text,
      modifier =
          Modifier.clip(RoundedCornerShape(8.dp))
              .clickable(onClick = onClick)
              .padding(horizontal = 2.dp, vertical = 2.dp),
      color =
          if (showFullTime) {
            MiuixTheme.colorScheme.onSurface
          } else {
            MiuixTheme.colorScheme.onSurfaceVariantSummary
          },
      fontSize = 12.sp,
      lineHeight = 14.sp,
      maxLines = 1,
  )
}

private fun logEntrySummary(entry: LogEntry): String {
  if (entry.isModuleWebUiExport) return "存储重定向X · ${entry.action.ifBlank { "模块导出" }}"
  val parts = mutableListOf<String>()
  if (entry.operationIntent.isNotBlank()) parts += monitorIntentLabel(entry.operationIntent)
  val process = entry.processPackage.takeIf { it.isNotBlank() && it != "-" }
  val caller = entry.callerPackage.takeIf { it.isNotBlank() && it != "-" }
  val watch = entry.watchPackage.takeIf { it.isNotBlank() && it != "-" }
  val method = logIdentifyMethodText(entry.identifyMethod)
  when {
    caller != null &&
        caller != process &&
        caller.isSinglePackageName() &&
        !caller.isIntermediateLogPackage() -> parts += "调用方 $caller" + method.parenthesized()
    caller != null && caller != process -> parts += "候选应用 $caller" + method.parenthesized()
    entry.identifyMethod == "watch_package" &&
        watch != null &&
        watch != process &&
        !watch.isIntermediateLogPackage() -> parts += "监视应用 $watch" + method.parenthesized()
    process != null -> parts += "进程 $process" + method.parenthesized()
    method.isNotBlank() -> parts += method
  }
  val reliability = logReliabilityText(entry.identifyReliability)
  if (reliability.isNotBlank()) parts += "可靠性 $reliability"
  return parts.joinToString(" · ")
}

private fun String.parenthesized(): String = if (isBlank()) "" else "（$this）"

private fun String.isSinglePackageName(): Boolean =
    all { it.isLetterOrDigit() || it == '_' || it == '.' || it == '-' } && contains('.')

private fun String.isIntermediateLogPackage(): Boolean =
    this == "com.google.android.providers.media.module" ||
        this == "com.android.providers.media.module" ||
        this == "com.android.providers.media" ||
        this == "com.android.providers.downloads" ||
        this == "com.android.providers.downloads.ui" ||
        this == "com.android.externalstorage" ||
        this == "com.android.mtp" ||
        contains(".documentsui") ||
        contains(".photopicker")

private fun logIdentifyMethodText(method: String): String =
    when (method) {
      "caller" -> "直接调用方"
      "recent_caller" -> "近期调用方"
      "recent_private_caller" -> "近期私有路径调用方"
      "recent_private_owner" -> "近期私有路径归属"
      "path_owner" -> "路径归属"
      "path_config" -> "路径配置"
      "daemon_inotify" -> "外部 inotify"
      "path_hint" -> "路径推断"
      "stack" -> "堆栈推断"
      "owner_uid" -> "文件属主"
      "download_owner" -> "下载记录"
      "query_access" -> "媒体查询记录"
      "module_export" -> "模块导出记录"
      "provider_open" -> "Provider 打开请求"
      "media_provider_fallback" -> "MediaProvider 回退"
      "thread_name" -> "线程名"
      "java_stack" -> "Java 栈推断"
      "shared_uid" -> "共享 UID 回退"
      "unknown" -> "来源未知"
      else -> method
    }

private fun logReliabilityText(reliability: String): String =
    when (reliability) {
      "high" -> "高"
      "medium" -> "中"
      "fallback" -> "回退"
      "none" -> "未知"
      else -> reliability
    }

@Composable
private fun PathExpandButton(expanded: Boolean, enabled: Boolean, onClick: () -> Unit) {
  Box(
      modifier =
          Modifier.size(24.dp)
              .clip(CircleShape)
              .background(
                  if (enabled && isSrxLiquidGlassEnabled()) glassSurfaceColor(0.5f)
                  else Color.Transparent,
                  CircleShape,
              )
              .clickable(
                  enabled = enabled,
                  interactionSource = null,
                  indication = null,
                  onClick = onClick,
              ),
      contentAlignment = Alignment.Center,
  ) {
    Icon(
        imageVector = Icons.Rounded.KeyboardArrowDown,
        contentDescription = if (expanded) "收起详情" else "展开详情",
        tint =
            if (enabled) MiuixTheme.colorScheme.onSurface
            else MiuixTheme.colorScheme.onSurfaceVariantSummary.copy(alpha = 0.45f),
        modifier = Modifier.size(18.dp).graphicsLayer { rotationZ = if (expanded) 180f else 0f },
    )
  }
}

@Composable
private fun LogOperationBadge(operation: String, ok: Boolean) {
  val color = if (ok) srxSuccessColor() else MiuixTheme.colorScheme.error
  Text(
      text = operation.ifBlank { "unknown" },
      modifier =
          Modifier.clip(RoundedCornerShape(7.dp))
              .background(color.copy(alpha = 0.14f))
              .padding(horizontal = 7.dp, vertical = 3.dp),
      color = color,
      fontSize = 10.sp,
      fontWeight = FontWeight.Black,
      lineHeight = 10.sp,
      maxLines = 1,
      overflow = TextOverflow.Ellipsis,
  )
}

@Composable
internal fun LogRow(
    entry: LogEntry,
    showFullTime: Boolean,
    onToggleTime: () -> Unit,
) {
  Row(
      modifier = Modifier.fillMaxWidth().padding(14.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(10.dp),
  ) {
    Box(
        Modifier.size(32.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(
                if (entry.ok) MiuixTheme.colorScheme.primary.copy(alpha = 0.14f)
                else MiuixTheme.colorScheme.error.copy(alpha = 0.14f)
            ),
        contentAlignment = Alignment.Center,
    ) {
      Icon(
          MiuixIcons.File,
          contentDescription = null,
          tint = if (entry.ok) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.error,
          modifier = Modifier.size(18.dp),
      )
    }
    Column(Modifier.weight(1f)) {
      Text(
          if (entry.isModuleWebUiExport) entry.label.ifBlank { "存储重定向X" }
          else entry.packageName.ifBlank { "未知应用" },
          maxLines = 1,
          overflow = TextOverflow.Ellipsis,
          fontWeight = FontWeight.SemiBold,
      )
      Text(
          entry.action,
          maxLines = 1,
          overflow = TextOverflow.Ellipsis,
          color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
          fontSize = 12.sp,
      )
      Text(
          entry.path.ifBlank { "未解析到路径" },
          maxLines = 1,
          overflow = TextOverflow.Ellipsis,
          fontSize = 12.sp,
      )
    }
    LogTimeText(
        text = formatLogEntryTime(entry, showFullTime),
        showFullTime = showFullTime,
        onClick = onToggleTime,
    )
  }
}
