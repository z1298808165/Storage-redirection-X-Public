package org.srx.manager.ui.screen

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import org.srx.manager.EmptyText
import org.srx.manager.PageHeader
import org.srx.manager.RoundIconAction
import org.srx.manager.data.AppFilter
import org.srx.manager.data.InstalledApp
import org.srx.manager.glassPanel
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.component.SrxSearchField
import top.yukonga.miuix.kmp.basic.PullToRefresh
import top.yukonga.miuix.kmp.basic.rememberPullToRefreshState
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun AppsScreen(
    state: AppUiState,
    listState: LazyListState,
    bottomPadding: Dp,
    selectedPackages: Set<String>,
    onRefresh: () -> Unit,
    onSearch: (String) -> Unit,
    onFilter: (AppFilter) -> Unit,
    onUser: (String) -> Unit,
    onOpenApp: (InstalledApp) -> Unit,
    onLongPressApp: (InstalledApp) -> Unit,
) {
  val filtered by
      remember(state.apps, state.filter, state.search) {
        derivedStateOf {
          val query = state.search.trim().lowercase()
          state.apps.filter { app ->
            val filterOk =
                when (state.filter) {
                  AppFilter.User -> !app.isSystem
                  AppFilter.System -> app.isSystem
                  AppFilter.Configured -> app.isConfigured
                }
            filterOk &&
                (query.isBlank() ||
                    app.searchLabel.contains(query) ||
                    app.searchPackageName.contains(query))
          }
        }
      }
  val pullToRefreshState = rememberPullToRefreshState()
  val refreshTexts = listOf("下拉刷新", "释放刷新", "正在刷新", "刷新完成")
  val listShape = RoundedCornerShape(24.dp)
  val scrollScope = rememberCoroutineScope()
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
        title = "应用管理",
        actions = {
          // 长列表下回拉到顶部再下拉刷新成本太高，标题栏提供一个常驻入口。
          // 刷新中禁用，避免连点反复拉起 root 查询。
          RoundIconAction(
              MiuixIcons.Refresh,
              "刷新应用列表",
              {
                // 刷新语义包含「看最新结果」，先回到顶部，避免刷完仍停在旧位置。
                scrollScope.launch { listState.animateScrollToItem(0) }
                onRefresh()
              },
              enabled = !state.appsRefreshing,
          )
        },
    )
    Spacer(Modifier.height(14.dp))
    SrxSearchField(
        query = state.search,
        onQueryChange = onSearch,
        label = "搜索应用名称或包名...",
    )
    Spacer(Modifier.height(12.dp))
    AppListControls(
        filter = state.filter,
        users = state.users,
        selectedUser = state.selectedUser,
        onFilter = onFilter,
        onUser = onUser,
    )
    Spacer(Modifier.height(12.dp))
    if (state.apps.isEmpty() && !state.appsLoaded) {
      Box(
          modifier =
              Modifier.fillMaxWidth()
                  .weight(1f)
                  .glassPanel(listShape, shadowAlpha = 0.05f, surfaceAlpha = 0.62f)
                  .clip(listShape),
      ) {
        AppListSkeleton(bottomPadding = bottomPadding)
      }
      return@Column
    }
    PullToRefresh(
        isRefreshing = state.appsRefreshing,
        pullToRefreshState = pullToRefreshState,
        onRefresh = onRefresh,
        refreshTexts = refreshTexts,
        modifier =
            Modifier.fillMaxWidth()
                .weight(1f)
                .glassPanel(listShape, shadowAlpha = 0.05f, surfaceAlpha = 0.62f)
                .clip(listShape),
    ) {
      LazyColumn(
          state = listState,
          modifier = Modifier.fillMaxSize().overScrollVertical(),
          contentPadding = PaddingValues(bottom = bottomPadding + 28.dp),
          verticalArrangement = Arrangement.spacedBy(0.dp),
          overscrollEffect = null,
      ) {
        if (filtered.isEmpty()) {
          item { EmptyText("没有找到应用") }
        } else {
          itemsIndexed(
              filtered,
              key = { _, app -> app.packageName },
              contentType = { _, app ->
                when {
                  app.isMissing -> "missing"
                  app.isEnabled -> "enabled"
                  app.isConfigured -> "configured"
                  else -> "unconfigured"
                }
              },
          ) { index, app ->
            AppListItem(
                app = app,
                selected = app.packageName in selectedPackages,
                selectionMode = selectedPackages.isNotEmpty(),
                showDivider = index != filtered.lastIndex,
                onClick = { onOpenApp(app) },
                onLongPress = { onLongPressApp(app) },
            )
          }
        }
      }
    }
  }
}
