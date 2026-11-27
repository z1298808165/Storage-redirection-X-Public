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
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import org.srx.manager.RoundIconAction
import org.srx.manager.SrxProgressivePage
import org.srx.manager.data.FileMonitorFilters
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.LogEntry
import org.srx.manager.glassSurfaceColor
import org.srx.manager.root.isSafePackageName
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.component.AppIconImage
import org.srx.manager.ui.component.SrxSearchField
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.PullToRefresh
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.rememberPullToRefreshState
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.ChevronForward
import top.yukonga.miuix.kmp.icon.glass.Close
import top.yukonga.miuix.kmp.icon.glass.Delete
import top.yukonga.miuix.kmp.icon.glass.ExpandMore
import top.yukonga.miuix.kmp.icon.glass.GridView
import top.yukonga.miuix.kmp.icon.glass.ListView
import top.yukonga.miuix.kmp.icon.glass.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
internal fun LogsScreen(
    state: AppUiState,
    logs: List<LogEntry>,
    apps: List<InstalledApp>,
    listState: LazyListState,
    bottomPadding: Dp,
    groupedByApp: Boolean,
    onToggleGrouped: () -> Unit,
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
                      it.eventRole,
                      it.correlationId,
                      it.observerPackage,
                      it.observationCount.toString(),
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
  // 按应用聚合视图：以实际落盘进程优先，其次调用方，保证同一应用的记录落在一组。
  // 聚合视图的主搜索只按应用名/包名过滤分组；路径搜索放在详情弹窗里。
  val appLogGroups =
      remember(filtered, appsByPackage, query) {
        val q = query.trim().lowercase()
        filtered
            .groupBy { it.processPackage.ifBlank { it.callerPackage.ifBlank { it.packageName } } }
            .map { (pkg, entries) -> AppLogGroup(pkg, entries) }
            .filter { group ->
              q.isBlank() ||
                  group.packageName.lowercase().contains(q) ||
                  (appsByPackage[group.packageName]?.label.orEmpty().lowercase().contains(q))
            }
            .sortedByDescending { group -> group.entries.firstOrNull()?.timestamp.orEmpty() }
      }
  var detailGroup by remember { mutableStateOf<AppLogGroup?>(null) }
  var detailQuery by remember { mutableStateOf("") }
  // 只在页面首次出现时回到顶部。停留期间的自动刷新绝不能滚回去——用户可能正
  // 在看下面的记录，滚回顶部等于把阅读位置抢走。下拉刷新本身就发生在顶部，无需处理。
  LaunchedEffect(Unit) { listState.scrollToItem(0) }
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
  SrxProgressivePage(
      title = "文件监视",
      actions = {
        RoundIconAction(
            MiuixIcons.Glass.Tune,
            "文件监视过滤",
            { showFilters = true },
            size = 36.dp,
            iconSize = 17.dp,
        )
        RoundIconAction(
            MiuixIcons.Glass.Delete,
            "清空文件监视记录",
            { confirmClear = true },
            danger = true,
        )
      },
  ) { pageModifier, innerPadding, scrollModifiers ->
    Column(modifier = pageModifier.padding(horizontal = 16.dp)) {
      Spacer(Modifier.height(innerPadding.calculateTopPadding() + 14.dp))
      SrxSearchField(
          query,
          { query = it },
          if (groupedByApp) "搜索应用名或包名" else "搜索应用名、包名或路径",
          trailing = {
            RoundIconAction(
                icon = if (groupedByApp) MiuixIcons.Glass.GridView else MiuixIcons.Glass.ListView,
                contentDescription = if (groupedByApp) "切换到明细视图" else "切换到按应用视图",
                onClick = onToggleGrouped,
                size = 34.dp,
                iconSize = 16.dp,
            )
          },
      )
      Spacer(Modifier.height(10.dp))
      Text(
          text =
              if (query.isBlank()) "共 ${logs.size} 条" else "匹配 ${filtered.size} / ${logs.size} 条",
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
            modifier = Modifier.fillMaxSize().then(scrollModifiers),
            contentPadding = PaddingValues(bottom = bottomPadding + 28.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
            overscrollEffect = null,
        ) {
          if (filtered.isEmpty()) {
            item { EmptyText(if (query.isBlank()) "暂无文件操作记录" else "没有匹配的日志") }
          } else if (groupedByApp) {
            items(appLogGroups, key = { it.packageName }) { group ->
              GroupedAppCard(
                  group = group,
                  appsByPackage = appsByPackage,
                  onOpen = {
                    detailQuery = ""
                    detailGroup = group
                  },
              )
            }
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
    AppLogsDialog(
        group = detailGroup,
        appsByPackage = appsByPackage,
        query = detailQuery,
        onQueryChange = { detailQuery = it },
        showFullTime = showFullTime,
        onToggleTime = { showFullTime = !showFullTime },
        onOpenApp = onOpenApp,
        onDismiss = { detailGroup = null },
    )
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
  val isFilesystemObservation = entry.eventRole == "observation"
  val isMediaProviderFallback = entry.identifyMethod == "media_provider_fallback"
  val hasAttributedCaller = isAttributedFilesystemObservation(entry)
  val displayName =
      when {
        entry.isModuleWebUiExport -> entry.label.ifBlank { "存储重定向X" }
        hasAttributedCaller -> app?.label ?: entry.packageName
        isFilesystemObservation -> "文件系统观察"
        else ->
            app?.label
                ?: entry.label.takeIf { it.isNotBlank() && it != entry.packageName }
                ?: entry.packageName.ifBlank { "未知应用" }
      }
  val openTarget =
      if (
          entry.isModuleWebUiExport ||
              (isFilesystemObservation && !hasAttributedCaller) ||
              isMediaProviderFallback
      ) {
        null
      } else {
        app ?: entry.toInstalledAppOrNull(displayName)
      }
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
      // 横向 16dp 与其他卡片一致；纵向 8dp。首行只有 34dp 图标 + 单行标题，
      // 卡片自身的上下留白 + 图标比文字高出的部分会叠成一条空白带，
      // 所以这里比常规卡片再紧一档。
      insideMargin = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
      shadowAlpha = 0f,
  ) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
      Row(
          modifier = Modifier.fillMaxWidth(),
          verticalAlignment = Alignment.CenterVertically,
          // 右侧三件套（操作类型 / 时间 / 展开）统一 10dp 间距：之前用 8dp，
          // 但展开按钮外层是 48dp 触控框、视觉圆只有 24dp，左右各 12dp 的空白
          // 全算进了间距，于是"时间↔展开"看起来比"类型↔时间"宽一倍多。
          horizontalArrangement = Arrangement.spacedBy(10.dp),
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
        // 这里刻意比配置页的行（SrxRowMetrics.iconSize，Miuix 30dp）大一号：
        // 日志卡片首行是"图标 + 单行应用名"，图标稍大才能撑住这一行的视觉重量。
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

internal fun LogEntry.toInstalledAppOrNull(displayName: String): InstalledApp? {
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

internal fun formatLogEntryTime(entry: LogEntry, showFullTime: Boolean): String {
  if (!showFullTime) return entry.timeText.ifBlank { "--:--" }
  val timestamp = entry.timestamp.replace('T', ' ')
  return when {
    timestamp.length >= 16 -> timestamp.substring(0, 16)
    timestamp.isNotBlank() -> timestamp
    else -> entry.timeText.ifBlank { "--:--" }
  }
}

internal fun logEntryPrimaryPath(entry: LogEntry): String {
  val backend = entry.backendPath
  if (
      backend.isNotBlank() &&
          entry.source in setOf("sandbox_path", "redirect_root", "fuse_redirect")
  ) {
    return backend
  }
  return entry.path.ifBlank { entry.landingPath }
}

internal fun logEntryRequestPath(entry: LogEntry): String =
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

internal fun isAttributedFilesystemObservation(entry: LogEntry): Boolean =
    entry.eventRole == "observation" &&
        (entry.source == "path_mapping" && entry.identifyMethod == "daemon_inotify" ||
            entry.identifyMethod in
                setOf(
                    "provider_open",
                    "saf_provider",
                    "public_path_package",
                    "path_owner",
                    "owner_uid",
                )) &&
        entry.packageName.isSinglePackageName() &&
        !entry.packageName.isIntermediateLogPackage()

internal fun logEntrySummary(entry: LogEntry): String {
  if (entry.isModuleWebUiExport) return "存储重定向X · ${entry.action.ifBlank { "模块导出" }}"
  val parts = mutableListOf<String>()
  if (entry.operationIntent.isNotBlank()) parts += monitorIntentLabel(entry.operationIntent)
  // passthrough 是大多数，不显示以免淹没真正走了规则的记录。
  when (entry.decision) {
    "redirect" -> parts += "已重定向"
    "deny" -> parts += "已拦截"
    "rule_miss" -> parts += "规则未命中"
    else -> Unit
  }
  val process = entry.processPackage.takeIf { it.isNotBlank() && it != "-" }
  val caller = entry.callerPackage.takeIf { it.isNotBlank() && it != "-" }
  val watch = entry.watchPackage.takeIf { it.isNotBlank() && it != "-" }
  val method = logIdentifyMethodText(entry.identifyMethod)
  val hasAttributedCaller = isAttributedFilesystemObservation(entry)
  when {
    hasAttributedCaller -> {
      val subject =
          if (entry.identifyMethod in setOf("provider_open", "provider_mkdir", "saf_provider"))
              "调用方"
          else "源应用"
      val attribution =
          if (entry.identifyMethod in setOf("provider_open", "provider_mkdir", "saf_provider"))
              "路径提示关联"
          else method
      parts += "$subject ${entry.packageName}（$attribution）"
      if (watch != null && watch != process && !watch.isIntermediateLogPackage()) {
        parts += "监视上下文 $watch"
      }
    }
    entry.eventRole == "observation" -> {
      val observer = entry.observerPackage.takeIf { it.isNotBlank() && it != "-" }
      if (process != null) {
        parts += "文件系统观察 · 落盘进程 $process" + method.parenthesized()
      } else {
        parts += "文件系统观察" + method.parenthesized()
      }
      if (observer != null && observer != process) parts += "观察器 $observer"
      if (watch != null && watch != process && !watch.isIntermediateLogPackage()) {
        parts += "监视上下文 $watch"
      }
      if (entry.correlationId.isNotBlank()) parts += "关联 ${entry.correlationId}"
      if (entry.identifyMethod == "media_provider_fallback") {
        parts += "调用方未从文件系统事件中获得"
      }
    }
    caller != null &&
        caller != process &&
        caller.isSinglePackageName() &&
        !caller.isIntermediateLogPackage() -> parts += "调用方 $caller" + method.parenthesized()
    caller != null && caller != process -> parts += "候选应用 $caller" + method.parenthesized()
    entry.identifyMethod == "media_provider_fallback" -> {
      if (process != null) parts += "落盘进程 $process" + method.parenthesized()
      if (watch != null && watch != process && !watch.isIntermediateLogPackage()) {
        parts += "监视上下文 $watch"
      }
      parts += "调用方未从文件系统事件中获得"
    }
    entry.identifyMethod == "watch_package" &&
        watch != null &&
        watch != process &&
        !watch.isIntermediateLogPackage() -> parts += "监视应用 $watch" + method.parenthesized()
    process != null -> parts += "进程 $process" + method.parenthesized()
    method.isNotBlank() -> parts += method
  }
  val decision = logDecisionText(entry.decision)
  if (decision.isNotBlank()) parts += decision
  val reliability = logReliabilityText(entry.identifyReliability)
  if (reliability.isNotBlank()) parts += "可靠性 $reliability"
  if (entry.observationCount > 0) {
    val observer = entry.observerPackage.takeIf { it.isNotBlank() }
    parts +=
        if (observer == null) {
          "已关联文件系统观察 ${entry.observationCount} 条"
        } else {
          "已关联文件系统观察 ${entry.observationCount} 条（$observer）"
        }
  }
  return parts.joinToString(" · ")
}

internal fun logDecisionText(decision: String): String =
    when (decision) {
      "redirect" -> "已按规则重定向"
      "allow" -> "命中放行规则"
      "deny" -> "被规则拦截（只读）"
      "rule_miss" -> "路径在规则范围内但未重定向"
      "passthrough" -> "未配置规则"
      else -> ""
    }

internal fun String.parenthesized(): String = if (isBlank()) "" else "（$this）"

internal fun String.isSinglePackageName(): Boolean =
    all { it.isLetterOrDigit() || it == '_' || it == '.' || it == '-' } && contains('.')

/** 日志条目的内容指纹，用于 LazyColumn 的稳定 key，不含列表位置信息。 */
internal fun LogEntry.contentKey(): String =
    "$timestamp|$processPackage|$callerPackage|$packageName|$eventRole|$correlationId|$observationCount|$operation|$path|$ok"

private fun String.isIntermediateLogPackage(): Boolean =
    this == "com.google.android.providers.media.module" ||
        this == "com.android.providers.media.module" ||
        this == "com.android.providers.media" ||
        this == "com.android.providers.downloads" ||
        this == "com.android.providers.downloads.ui" ||
        this == "com.android.externalstorage" ||
        this == "com.android.mtp" ||
        contains(".documentsui") ||
        contains("photopicker") ||
        contains("mediapicker")

internal fun logIdentifyMethodText(method: String): String =
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
      "public_path_package" -> "隐藏路径中的应用包名"
      "module_export" -> "模块导出记录"
      "provider_mkdir" -> "Provider 目录创建请求"
      "provider_open" -> "Provider 打开请求"
      "mount_prep" -> "挂载准备"
      "media_provider_fallback" -> "MediaProvider 回退"
      "thread_name" -> "线程名"
      "java_stack" -> "Java 栈推断"
      "shared_uid" -> "共享 UID 回退"
      "unknown" -> "来源未知"
      else -> method
    }

internal fun logReliabilityText(reliability: String): String =
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
          // 触控区不再用 48dp 外框：那会让这枚按钮比视觉圆宽一倍，右侧三件套的间距
          // 全被撑歪，而且圆点离卡片右边缘还有 12dp 空白，和下方路径文本对不齐。
          // 这里宽度收到与视觉圆一致的 24dp，高度留 32dp 兼顾纵向命中。
          Modifier.size(width = 24.dp, height = 32.dp)
              .clickable(
                  enabled = enabled,
                  interactionSource = null,
                  indication = null,
                  onClick = onClick,
              ),
      contentAlignment = Alignment.Center,
  ) {
    PathExpandButtonSurface(expanded = expanded, enabled = enabled)
  }
}

@Composable
private fun PathExpandButtonSurface(expanded: Boolean, enabled: Boolean) {
  Box(
      modifier =
          Modifier.size(24.dp)
              .clip(CircleShape)
              .background(
                  if (enabled && isSrxLiquidGlassEnabled()) glassSurfaceColor(0.5f)
                  else Color.Transparent,
                  CircleShape,
              ),
      contentAlignment = Alignment.Center,
  ) {
    Icon(
        // 展开/收起是纵向动作，必须用上下箭头：之前用 ChevronForward 再旋转 180°，
        // 视觉上就是"右箭头 / 左箭头"，与展开语义不符。
        imageVector = MiuixIcons.Glass.ExpandMore,
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
  // 成功态跟强调色走：日志卡片上每行都有一枚绿徽章，和整体色调不搭。
  val color = if (ok) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.error
  val copyValue = filterOperation.ifBlank { operation }.ifBlank { "unknown" }
  Text(
      text = operation.ifBlank { "unknown" },
      modifier =
          Modifier.clip(RoundedCornerShape(7.dp))
              .background(color.copy(alpha = if (isSrxDarkTheme()) 0.14f else 0.2f))
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

/** 聚合视图里的一个应用分组。 */
internal data class AppLogGroup(val packageName: String, val entries: List<LogEntry>) {
  val latestTimestamp: String
    get() = entries.firstOrNull()?.timestamp.orEmpty()
}

/** 聚合视图里的应用卡片：图标 + 应用名 + 包名 + 记录数与最近时间，点击打开详情弹窗。 */
@Composable
private fun GroupedAppCard(
    group: AppLogGroup,
    appsByPackage: Map<String, InstalledApp>,
    onOpen: () -> Unit,
) {
  val app = appsByPackage[group.packageName]
  val label = app?.label ?: group.packageName.ifBlank { "未知应用" }
  val latest = group.latestTimestamp
  GlassCard(insideMargin = PaddingValues(horizontal = 16.dp, vertical = 12.dp)) {
    Row(
        modifier = Modifier.fillMaxWidth().clickable { onOpen() },
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      AppIconImage(appInfo = app?.appInfo, label = label, modifier = Modifier.size(40.dp))
      Column(modifier = Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Text(
            text = label,
            color = MiuixTheme.colorScheme.onSurface,
            fontSize = 15.sp,
            fontWeight = FontWeight.Medium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            text = group.packageName,
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 11.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            text =
                "${group.entries.size} 条记录" +
                    (if (latest.isNotBlank()) " · 最近 ${latest.takeLast(8)}" else ""),
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            fontSize = 11.sp,
        )
      }
      Icon(
          imageVector = MiuixIcons.Glass.ChevronForward,
          contentDescription = null,
          modifier = Modifier.size(16.dp),
          tint = MiuixTheme.colorScheme.onSurfaceVariantSummary,
      )
    }
  }
}

/** 某个应用的记录详情：固定高度，避免记录条数不同导致弹窗高度跳变。 */
@Composable
private fun AppLogsDialog(
    group: AppLogGroup?,
    appsByPackage: Map<String, InstalledApp>,
    query: String,
    onQueryChange: (String) -> Unit,
    showFullTime: Boolean,
    onToggleTime: () -> Unit,
    onOpenApp: (InstalledApp) -> Unit,
    onDismiss: () -> Unit,
) {
  if (group == null) return
  val app = appsByPackage[group.packageName]
  val label = app?.label ?: group.packageName.ifBlank { "未知应用" }
  val q = query.trim().lowercase()
  val entries =
      remember(group, q) {
        if (q.isBlank()) group.entries
        else
            group.entries.filter { entry ->
              listOf(entry.path, entry.landingPath, entry.fromPath, entry.backendPath).any {
                it.lowercase().contains(q)
              }
            }
      }
  CenteredDialog(show = true, onDismiss = onDismiss) {
    Column(
        modifier = Modifier.fillMaxWidth().height(560.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
      Row(
          modifier = Modifier.fillMaxWidth(),
          verticalAlignment = Alignment.CenterVertically,
          horizontalArrangement = Arrangement.spacedBy(10.dp),
      ) {
        AppIconImage(appInfo = app?.appInfo, label = label, modifier = Modifier.size(32.dp))
        Column(modifier = Modifier.weight(1f)) {
          Text(
              text = label,
              color = MiuixTheme.colorScheme.onSurface,
              fontSize = 15.sp,
              fontWeight = FontWeight.Medium,
              maxLines = 1,
          )
          Text(
              text = "${group.entries.size} 条记录",
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 11.sp,
          )
        }
        RoundIconAction(
            icon = MiuixIcons.Glass.Close,
            contentDescription = "关闭",
            onClick = onDismiss,
            size = 32.dp,
            iconSize = 15.dp,
        )
      }
      SrxSearchField(query, onQueryChange, "搜索路径")
      LazyColumn(
          modifier = Modifier.fillMaxSize(),
          verticalArrangement = Arrangement.spacedBy(10.dp),
          contentPadding = PaddingValues(bottom = 8.dp),
      ) {
        if (entries.isEmpty()) {
          item { EmptyText("没有匹配的记录") }
        } else {
          items(entries, key = { entry -> entry.contentKey() + entry.timestamp }) { entry ->
            LogCard(
                entry = entry,
                app = app,
                showFullTime = showFullTime,
                onToggleTime = onToggleTime,
                onOpenApp = onOpenApp,
            )
          }
        }
      }
    }
  }
}
