package org.srx.manager.ui.screen

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
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
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
import androidx.compose.runtime.Composable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.EmptyText
import org.srx.manager.PageHeader
import org.srx.manager.capsuleContainerColor
import org.srx.manager.capsuleSelectedColor
import org.srx.manager.glassPanel
import org.srx.manager.glassSurfaceColor
import org.srx.manager.srxDangerColor
import org.srx.manager.srxPrimaryColor
import org.srx.manager.srxSuccessColor
import org.srx.manager.data.AppFilter
import org.srx.manager.data.InstalledApp
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.component.AppIconImage
import org.srx.manager.ui.component.SrxSearchField
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.PullToRefresh
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.rememberPullToRefreshState
import top.yukonga.miuix.kmp.theme.MiuixTheme
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
    val filtered by remember(state.apps, state.filter, state.search) {
        derivedStateOf {
            state.apps.filter { app ->
                val filterOk = when (state.filter) {
                    AppFilter.User -> !app.isSystem
                    AppFilter.System -> app.isSystem
                    AppFilter.Configured -> app.isConfigured
                }
                val query = state.search.trim().lowercase()
                filterOk && (query.isBlank() || app.label.lowercase().contains(query) || app.packageName.lowercase().contains(query))
            }
        }
    }
    val pullToRefreshState = rememberPullToRefreshState()
    val refreshTexts = listOf("下拉刷新", "释放刷新", "正在刷新", "刷新完成")
    val listShape = RoundedCornerShape(24.dp)
    Column(
        modifier = Modifier
            .fillMaxSize()
            .padding(
                top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 18.dp,
                start = 16.dp,
                end = 16.dp,
            ),
    ) {
        PageHeader("应用管理")
        Spacer(Modifier.height(14.dp))
        SrxSearchField(
            query = state.search,
            onQueryChange = onSearch,
            label = "搜索应用名称或包名...",
        )
        Spacer(Modifier.height(12.dp))
        Row(
            modifier = Modifier.fillMaxWidth(),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.Top,
        ) {
            AppFilterGroup(
                filter = state.filter,
                onFilter = onFilter,
                modifier = Modifier.width(238.dp),
            )
            Spacer(Modifier.weight(1f))
            AppUserSwitcher(
                users = state.users,
                selectedUser = state.selectedUser,
                onUser = onUser,
            )
        }
        Spacer(Modifier.height(12.dp))
        if (state.apps.isEmpty() && !state.appsLoaded) {
            Box(
                modifier = Modifier
                    .fillMaxWidth()
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
            modifier = Modifier
                .fillMaxWidth()
                .weight(1f)
                .glassPanel(listShape, shadowAlpha = 0.05f, surfaceAlpha = 0.62f)
                .clip(listShape),
        ) {
            LazyColumn(
                state = listState,
                modifier = Modifier
                    .fillMaxSize()
                    .overScrollVertical(),
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

@Composable
private fun AppFilterGroup(
    filter: AppFilter,
    onFilter: (AppFilter) -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
            .clip(CircleShape)
            .background(capsuleContainerColor(), CircleShape)
            .padding(5.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        FilterButton("用户", filter == AppFilter.User, Modifier.weight(1f)) { onFilter(AppFilter.User) }
        FilterButton("系统", filter == AppFilter.System, Modifier.weight(1f)) { onFilter(AppFilter.System) }
        FilterButton("已配置", filter == AppFilter.Configured, Modifier.weight(1f)) { onFilter(AppFilter.Configured) }
    }
}

@Composable
private fun FilterButton(label: String, selected: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val color = if (selected) srxPrimaryColor() else MiuixTheme.colorScheme.onSurface
    Box(
        modifier = modifier
            .then(
                if (selected) {
                    Modifier.dropShadow(CircleShape, Shadow(radius = 12.dp, color = if (isSrxDarkTheme()) Color.Black else Color(0xFF73839C), alpha = if (isSrxDarkTheme()) 0.22f else 0.14f))
                } else {
                    Modifier
                },
            )
            .clip(CircleShape)
            .background(if (selected) capsuleSelectedColor() else Color.Transparent, CircleShape)
            .clickable(onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 9.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = label,
            color = color,
            fontWeight = if (selected) FontWeight.Bold else FontWeight.SemiBold,
            fontSize = 12.sp,
            maxLines = 1,
        )
    }
}

@Composable
private fun AppUserSwitcher(
    users: List<String>,
    selectedUser: String,
    onUser: (String) -> Unit,
) {
    if (users.size <= 1) return
    var expanded by remember(users, selectedUser) { mutableStateOf(false) }
    Column(horizontalAlignment = Alignment.End) {
        Text(
            text = "U$selectedUser",
            modifier = Modifier
                .dropShadow(CircleShape, Shadow(radius = 12.dp, color = if (isSrxDarkTheme()) Color.Black else Color(0xFF73839C), alpha = if (isSrxDarkTheme()) 0.22f else 0.14f))
                .clip(CircleShape)
                .background(capsuleSelectedColor(), CircleShape)
                .clickable { expanded = !expanded }
                .padding(horizontal = 15.dp, vertical = 12.dp),
            color = srxPrimaryColor(),
            fontWeight = FontWeight.Black,
            fontSize = 12.sp,
        )
        AnimatedVisibility(visible = expanded) {
            Column(
                modifier = Modifier
                    .padding(top = 8.dp)
                    .clip(RoundedCornerShape(20.dp))
                    .background(glassSurfaceColor(0.88f))
                    .padding(6.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                users.forEach { user ->
                    val selected = user == selectedUser
                    Text(
                        text = "用户 $user",
                        modifier = Modifier
                            .clip(RoundedCornerShape(15.dp))
                            .background(if (selected) capsuleSelectedColor() else Color.Transparent)
                            .clickable {
                                expanded = false
                                onUser(user)
                            }
                            .padding(horizontal = 12.dp, vertical = 10.dp),
                        color = if (selected) srxPrimaryColor() else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                        fontWeight = FontWeight.Bold,
                        fontSize = 12.sp,
                    )
                }
            }
        }
    }
}

@Composable
private fun AppListSkeleton(bottomPadding: Dp) {
    LazyColumn(
        modifier = Modifier.fillMaxSize(),
        contentPadding = PaddingValues(bottom = bottomPadding + 28.dp),
        verticalArrangement = Arrangement.spacedBy(0.dp),
        userScrollEnabled = false,
    ) {
        items(8) { index ->
            AppListSkeletonItem(showDivider = index != 7)
        }
    }
}

@Composable
private fun AppListSkeletonItem(showDivider: Boolean) {
    val blockColor = MiuixTheme.colorScheme.onSurface.copy(alpha = if (isSrxDarkTheme()) 0.08f else 0.07f)
    Column(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 16.dp, vertical = 15.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(13.dp),
        ) {
            SkeletonBlock(
                color = blockColor,
                modifier = Modifier.size(46.dp),
                shape = RoundedCornerShape(12.dp),
            )
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                SkeletonBlock(
                    color = blockColor,
                    modifier = Modifier
                        .fillMaxWidth(0.58f)
                        .height(16.dp),
                    shape = RoundedCornerShape(8.dp),
                )
                SkeletonBlock(
                    color = blockColor,
                    modifier = Modifier
                        .fillMaxWidth(0.78f)
                        .height(12.dp),
                    shape = RoundedCornerShape(6.dp),
                )
            }
            SkeletonBlock(
                color = blockColor,
                modifier = Modifier
                    .width(54.dp)
                    .height(24.dp),
                shape = CircleShape,
            )
        }
        if (showDivider) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(start = 75.dp)
                    .height(1.dp)
                    .background(MiuixTheme.colorScheme.onSurface.copy(alpha = if (isSrxDarkTheme()) 0.03f else 0.04f)),
            )
        }
    }
}

@Composable
private fun SkeletonBlock(
    color: Color,
    modifier: Modifier,
    shape: Shape,
) {
    Box(modifier.clip(shape).background(color))
}

@Composable
private fun AppListItem(
    app: InstalledApp,
    selected: Boolean,
    selectionMode: Boolean,
    showDivider: Boolean,
    onClick: () -> Unit,
    onLongPress: () -> Unit,
) {
    val primaryTextColor = if (app.isMissing) srxDangerColor() else MiuixTheme.colorScheme.onSurface
    val secondaryTextColor = if (app.isMissing) srxDangerColor().copy(alpha = 0.82f) else MiuixTheme.colorScheme.onSurfaceVariantSummary
    Column(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .pointerInput(selectionMode, app.packageName) {
                    detectTapGestures(
                        onLongPress = { onLongPress() },
                        onTap = { onClick() },
                    )
                }
                .background(if (selected) MiuixTheme.colorScheme.primary.copy(alpha = if (isSrxDarkTheme()) 0.12f else 0.08f) else Color.Transparent)
                .padding(horizontal = 16.dp, vertical = 15.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(13.dp),
        ) {
            AppIconImage(appInfo = app.appInfo, label = app.label, modifier = Modifier.size(46.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(app.label, maxLines = 1, overflow = TextOverflow.Ellipsis, color = primaryTextColor, fontWeight = FontWeight.Bold, fontSize = 16.sp, lineHeight = 20.sp)
                Text(app.packageName, maxLines = 1, overflow = TextOverflow.Ellipsis, color = secondaryTextColor, fontSize = 12.sp, lineHeight = 16.sp)
            }
            if (selectionMode) SelectionMark(selected) else StatusPill(app)
        }
        if (showDivider) {
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(start = 75.dp)
                    .height(1.dp)
                    .background(MiuixTheme.colorScheme.onSurface.copy(alpha = if (isSrxDarkTheme()) 0.03f else 0.04f)),
            )
        }
    }
}

@Composable
private fun SelectionMark(selected: Boolean) {
    Box(
        modifier = Modifier
            .size(28.dp)
            .clip(CircleShape)
            .background(if (selected) MiuixTheme.colorScheme.primary else glassSurfaceColor(0.72f), CircleShape)
            .drawBehind {
                drawCircle(
                    color = if (selected) Color.Transparent else Color.Gray.copy(alpha = 0.32f),
                    style = Stroke(width = 1.5.dp.toPx()),
                )
            },
        contentAlignment = Alignment.Center,
    ) {
        if (selected) {
            Icon(Icons.Rounded.Check, contentDescription = "已选择", tint = Color.White, modifier = Modifier.size(16.dp))
        }
    }
}

@Composable
private fun StatusPill(app: InstalledApp) {
    val (text, color) = when {
        app.isMissing -> "应用已卸载" to srxDangerColor()
        app.isEnabled -> "已启用" to srxSuccessColor()
        app.isConfigured -> "未启用" to srxPrimaryColor()
        else -> "未配置" to MiuixTheme.colorScheme.onSurfaceVariantSummary
    }
    Text(
        text = text,
        modifier = Modifier
            .clip(CircleShape)
            .background(color.copy(alpha = 0.12f))
            .padding(horizontal = 10.dp, vertical = 6.dp),
        color = color,
        fontWeight = FontWeight.SemiBold,
        fontSize = 12.sp,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
}
