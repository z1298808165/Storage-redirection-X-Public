package org.srx.manager.ui.screen

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.sizeIn
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
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
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.EmptyText
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.MinTouchTargetSize
import org.srx.manager.PageHeader
import org.srx.manager.RoundIconAction
import org.srx.manager.data.FileMonitorFilters
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.LogEntry
import org.srx.manager.glassSurfaceColor
import org.srx.manager.root.isSafePackageName
import org.srx.manager.srxSuccessColor
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.component.AppIconImage
import org.srx.manager.ui.component.SrxSearchField
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.PullToRefresh
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.rememberPullToRefreshState
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.File
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
      remember(logs, query, appsByPackage) {
        val q = query.trim().lowercase()
        if (q.isBlank()) logs
        else
            logs.filter {
              val resolvedLabel = appsByPackage[it.packageName]?.label.orEmpty()
              listOf(
                      it.label,
                      resolvedLabel,
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
  // 同一批日志里可能存在内容完全相同的条目，按出现次数补一个序号使 key 唯一。
  // 该序号只取决于条目自身内容，插入新日志不会改变已有条目的 key。
  val logEntryKeys =
      remember(filtered) {
        val seen = HashMap<String, Int>()
        val keys = HashMap<LogEntry, String>(filtered.size)
        for (entry in filtered) {
          val base = entry.contentKey()
          val ordinal = seen.getOrDefault(base, 0)
          seen[base] = ordinal + 1
          keys.putIfAbsent(entry, if (ordinal == 0) base else "$base#$ordinal")
        }
        keys
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
              // key 不能包含列表下标：日志是最新在前，新增一条会让其后所有条目的下标
              // 位移，等价于全部 key 变化，导致整列表重建、item 内的展开状态丢失、
              // 复用失效。这里用内容指纹加「同内容第几次出现」的序号，既保证唯一，
              // 又不受插入位置影响。
              key = { _, entry -> logEntryKeys[entry] ?: entry.contentKey() },
              contentType = { _, entry -> if (entry.ok) "success" else "error" },
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
  val context = LocalContext.current
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
        LogOperationBadge(
            operation = entry.operation,
            filterOperation = entry.filterOperation,
            ok = entry.ok,
            onCopy = { value ->
              val clipboard =
                  context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
              clipboard.setPrimaryClip(ClipData.newPlainText("操作规则", value))
              Toast.makeText(context, "已复制操作规则：$value", Toast.LENGTH_SHORT).show()
            },
        )
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
      SelectionContainer {
        Text(
            primaryPath.ifBlank { "未解析到路径" },
            maxLines = if (expanded) Int.MAX_VALUE else 2,
            overflow = TextOverflow.Ellipsis,
            fontSize = 12.sp,
            lineHeight = 17.sp,
        )
      }
      if (expanded && actualPath.isNotBlank() && actualPath != primaryPath) {
        SelectionContainer {
          Text(
              "实际路径：$actualPath",
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 12.sp,
              lineHeight = 16.sp,
              maxLines = 3,
              overflow = TextOverflow.Ellipsis,
          )
        }
      }
      if (expanded && requestPath.isNotBlank() && requestPath != primaryPath) {
        SelectionContainer {
          Text(
              "请求路径：$requestPath",
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 12.sp,
              lineHeight = 16.sp,
              maxLines = 3,
              overflow = TextOverflow.Ellipsis,
          )
        }
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

/** 日志条目的内容指纹，用于 LazyColumn 的稳定 key，不含列表位置信息。 */
private fun LogEntry.contentKey(): String =
    "$timestamp|$processPackage|$callerPackage|$packageName|$operation|$path|$ok"

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
      "mount_prep" -> "挂载准备"
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
          // 展开按钮视觉上只有 24dp 且在日志卡片里密集排布，把命中区域撑到无障碍基线
          // 尺寸以降低误触，内层图标视觉不变。
          Modifier.sizeIn(minWidth = MinTouchTargetSize, minHeight = MinTouchTargetSize),
      contentAlignment = Alignment.Center,
  ) {
    PathExpandButtonSurface(expanded = expanded, enabled = enabled, onClick = onClick)
  }
}

@Composable
private fun PathExpandButtonSurface(expanded: Boolean, enabled: Boolean, onClick: () -> Unit) {
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
private fun LogOperationBadge(
    operation: String,
    filterOperation: String,
    ok: Boolean,
    onCopy: (String) -> Unit,
) {
  val color = if (ok) srxSuccessColor() else MiuixTheme.colorScheme.error
  val copyValue = filterOperation.ifBlank { operation }.ifBlank { "unknown" }
  Text(
      text = operation.ifBlank { "unknown" },
      modifier =
          Modifier.clip(RoundedCornerShape(7.dp))
              .background(color.copy(alpha = 0.14f))
              .clickable { onCopy(copyValue) }
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
