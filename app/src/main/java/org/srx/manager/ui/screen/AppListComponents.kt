package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
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
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.roundToInt
import org.srx.manager.capsuleSelectedColor
import org.srx.manager.data.AppFilter
import org.srx.manager.data.InstalledApp
import org.srx.manager.glassSurfaceColor
import org.srx.manager.srxDangerColor
import org.srx.manager.srxPrimaryColor
import org.srx.manager.ui.component.AppIconImage
import org.srx.manager.ui.component.liquidGlassControl
import org.srx.manager.ui.component.liquidPressScale
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxThemeCoui
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Ok
import top.yukonga.miuix.kmp.theme.MiuixTheme

// 应用列表的展示组件。
//
// 这里只负责「列表长什么样」，不含数据加载与过滤状态；AppsScreen 只组合这些组件，
// 避免在单文件里同时维护页面编排和十几处视觉细节。

@Composable
internal fun AppListControls(
    filter: AppFilter,
    users: List<String>,
    selectedUser: String,
    onFilter: (AppFilter) -> Unit,
    onUser: (String) -> Unit,
) {
  BoxWithConstraints(Modifier.fillMaxWidth()) {
    val stackUserSwitcher = users.size > 1 && maxWidth < 360.dp
    if (stackUserSwitcher) {
      Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        AppFilterGroup(filter = filter, onFilter = onFilter)
        Box(modifier = Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
          AppUserSwitcher(users = users, selectedUser = selectedUser, onUser = onUser)
        }
      }
    } else {
      // 筛选组按内容取宽（不再 weight 铺满），用户按钮靠右：用户列表异步到达、按钮出现时
      // 三个标签不会跟着重排，避免「点了用户菜单，Tab 位置却变了」。
      Row(
          modifier = Modifier.fillMaxWidth(),
          horizontalArrangement = Arrangement.SpaceBetween,
          verticalAlignment = Alignment.Top,
      ) {
        AppFilterGroup(filter = filter, onFilter = onFilter)
        AppUserSwitcher(users = users, selectedUser = selectedUser, onUser = onUser)
      }
    }
  }
}

@Composable
internal fun AppFilterGroup(
    filter: AppFilter,
    onFilter: (AppFilter) -> Unit,
    modifier: Modifier = Modifier,
) {
  val filters = listOf(AppFilter.User, AppFilter.System, AppFilter.Configured)
  // 与主题模式选择器共用同一个分段控件，选中态同样是"胶囊横移"，两个主题下外观一致。
  SrxSegmentedControl(
      labels = listOf("用户", "系统", "已配置"),
      selectedIndex = filters.indexOf(filter).coerceAtLeast(0),
      onSelect = { onFilter(filters[it]) },
      modifier = modifier,
      itemMinWidth = FilterButtonMinWidth,
      itemHeight = FilterButtonHeight,
      containerPadding = 5.dp,
      itemGap = 2.dp,
  )
}

@Composable
internal fun AppUserSwitcher(
    users: List<String>,
    selectedUser: String,
    onUser: (String) -> Unit,
) {
  if (users.size <= 1) return
  var expanded by remember(users, selectedUser) { mutableStateOf(false) }
  var anchorBounds by remember { mutableStateOf<IntRect?>(null) }
  var anchorHeightPx by remember { androidx.compose.runtime.mutableIntStateOf(0) }
  var anchorWidthPx by remember { androidx.compose.runtime.mutableIntStateOf(0) }
  val density = LocalDensity.current
  val triggerInteraction = remember { MutableInteractionSource() }
  Box(
      Modifier.onGloballyPositioned { coords ->
        anchorHeightPx = coords.size.height
        anchorWidthPx = coords.size.width
        val b = coords.boundsInWindow()
        anchorBounds =
            IntRect(
                b.left.roundToInt(),
                b.top.roundToInt(),
                b.right.roundToInt(),
                b.bottom.roundToInt(),
            )
      }
  ) {
    Text(
        text = "U$selectedUser",
        modifier =
            Modifier.dropShadow(
                    CircleShape,
                    Shadow(
                        radius = 12.dp,
                        color = if (isSrxDarkTheme()) Color.Black else Color(0xFF73839C),
                        alpha = if (isSrxDarkTheme()) 0.22f else 0.14f,
                    ),
                )
                .liquidPressScale(triggerInteraction)
                .liquidGlassControl(
                    shape = CircleShape,
                    tint = capsuleSelectedColor(),
                    refractionHeight = 8.dp,
                    refractionAmount = 10.dp,
                )
                .clickable(interactionSource = triggerInteraction, indication = null) {
                  expanded = !expanded
                }
                .padding(horizontal = 15.dp, vertical = 12.dp),
        color = srxPrimaryColor(),
        fontWeight = FontWeight.Black,
        fontSize = 12.sp,
    )
    // 与「主题风格」一致：点按钮后在它下方弹出锚定菜单，而不是居中对话框。
    SrxAnchoredOptionMenu(
        expanded = expanded,
        onDismiss = { expanded = false },
        options = users.map { it to "用户 $it" },
        selected = selectedUser,
        onSelect = { user ->
          expanded = false
          onUser(user)
        },
        bounds = anchorBounds,
        anchorHeightPx = anchorHeightPx,
        popupOffsetX = (anchorWidthPx - with(density) { 210.dp.toPx() }).toInt(),
    )
  }
}

@Composable
internal fun AppListSkeleton(bottomPadding: Dp) {
  LazyColumn(
      modifier = Modifier.fillMaxSize(),
      contentPadding = PaddingValues(bottom = bottomPadding + 28.dp),
      verticalArrangement = Arrangement.spacedBy(0.dp),
      userScrollEnabled = false,
  ) {
    items(8) { index -> AppListSkeletonItem(showDivider = index != 7) }
  }
}

@Composable
internal fun AppListSkeletonItem(showDivider: Boolean) {
  val blockColor =
      MiuixTheme.colorScheme.onSurface.copy(alpha = if (isSrxDarkTheme()) 0.08f else 0.07f)
  Column(Modifier.fillMaxWidth()) {
    Row(
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 15.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(13.dp),
    ) {
      SkeletonBlock(
          color = blockColor,
          modifier = Modifier.size(42.dp),
          shape = RoundedCornerShape(12.dp),
      )
      Column(
          modifier = Modifier.weight(1f),
          verticalArrangement = Arrangement.spacedBy(8.dp),
      ) {
        SkeletonBlock(
            color = blockColor,
            modifier = Modifier.fillMaxWidth(0.58f).height(16.dp),
            shape = RoundedCornerShape(8.dp),
        )
        SkeletonBlock(
            color = blockColor,
            modifier = Modifier.fillMaxWidth(0.78f).height(12.dp),
            shape = RoundedCornerShape(6.dp),
        )
      }
      SkeletonBlock(
          color = blockColor,
          modifier = Modifier.width(54.dp).height(24.dp),
          shape = CircleShape,
      )
    }
    if (showDivider) {
      // 与设置页一致：从文字起始处起画（16 边距 + 40 图标 + 12 间距），右侧留 16dp。
      ConfigDivider(startInset = 70.dp, endInset = 16.dp)
    }
  }
}

@Composable
internal fun SkeletonBlock(
    color: Color,
    modifier: Modifier,
    shape: Shape,
) {
  Box(modifier.clip(shape).background(color))
}

@Composable
internal fun AppListItem(
    app: InstalledApp,
    selected: Boolean,
    selectionMode: Boolean,
    showDivider: Boolean,
    onClick: () -> Unit,
    onLongPress: () -> Unit,
) {
  val primaryTextColor = if (app.isMissing) srxDangerColor() else MiuixTheme.colorScheme.onSurface
  val secondaryTextColor =
      if (app.isMissing) srxDangerColor().copy(alpha = 0.82f)
      else MiuixTheme.colorScheme.onSurfaceVariantSummary
  Column(Modifier.fillMaxWidth()) {
    Row(
        modifier =
            Modifier.fillMaxWidth()
                // 用 combinedClickable 而不是 detectTapGestures：后者不产生 Role.Button
                // 语义，也没有点击与长按的无障碍动作，读屏用户无法在应用列表这一主操作
                // 面进入配置页。
                .combinedClickable(
                    onClickLabel = if (selectionMode) "切换选中" else "打开配置",
                    onLongClickLabel = "进入多选",
                    onLongClick = { onLongPress() },
                    onClick = { onClick() },
                )
                .background(
                    if (selected)
                        MiuixTheme.colorScheme.primary.copy(
                            alpha = if (isSrxDarkTheme()) 0.12f else 0.08f
                        )
                    else Color.Transparent
                )
                .heightIn(min = 62.dp)
                .padding(horizontal = 16.dp, vertical = 11.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
      AppIconImage(appInfo = app.appInfo, label = app.label, modifier = Modifier.size(42.dp))
      Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Text(
            app.label,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            color = primaryTextColor,
            fontWeight = FontWeight.Bold,
            fontSize = 15.sp,
            lineHeight = 19.sp,
        )
        Text(
            app.packageName,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            // 包名保留但压弱：Eta 风格的单行列表里作为次要信息存在。
            color = secondaryTextColor,
            fontSize = 11.sp,
            lineHeight = 15.sp,
        )
      }
      if (selectionMode) SelectionMark(selected) else StatusPill(app)
    }
    if (showDivider) {
      Box(
          Modifier.fillMaxWidth()
              .padding(start = 75.dp)
              .height(1.dp)
              .background(
                  MiuixTheme.colorScheme.onSurface.copy(
                      alpha = if (isSrxDarkTheme()) 0.03f else 0.04f
                  )
              ),
      )
    }
  }
}

@Composable
internal fun SelectionMark(selected: Boolean) {
  Box(
      modifier =
          Modifier.size(28.dp)
              .clip(CircleShape)
              .background(
                  if (selected) MiuixTheme.colorScheme.primary else glassSurfaceColor(0.72f),
                  CircleShape,
              )
              .drawBehind {
                drawCircle(
                    color = if (selected) Color.Transparent else Color.Gray.copy(alpha = 0.32f),
                    style = Stroke(width = 1.5.dp.toPx()),
                )
              },
      contentAlignment = Alignment.Center,
  ) {
    if (selected) {
      Icon(
          MiuixIcons.Glass.Ok,
          contentDescription = "已选择",
          tint = Color.White,
          modifier = Modifier.size(16.dp),
      )
    }
  }
}

@Composable
internal fun StatusPill(app: InstalledApp) {
  // 徽章统一用强调色系：绿/橙这类语义色在深色页面上过于跳脱，
  // 「已启用 / 已配置」只靠填充强度区分，异常态才用错误色。
  val accent = MiuixTheme.colorScheme.primary
  val (text, color, filled) =
      when {
        app.isMissing -> Triple("应用已卸载", MiuixTheme.colorScheme.error, true)
        app.isEnabled -> Triple("已启用", accent, true)
        app.isConfigured -> Triple("已配置", accent, false)
        else -> Triple("未配置", MiuixTheme.colorScheme.onSurfaceVariantSummary, true)
      }
  Text(
      text = text,
      modifier =
          Modifier.clip(CircleShape)
              .background(
                  if (filled)
                      color.copy(alpha = if (isSrxThemeCoui() && !isSrxDarkTheme()) 0.2f else 0.12f)
                  else Color.Transparent,
                  CircleShape,
              )
              .then(
                  if (filled) Modifier
                  else Modifier.border(1.dp, color.copy(alpha = 0.45f), CircleShape)
              )
              .padding(horizontal = 10.dp, vertical = 6.dp),
      color = color,
      fontWeight = FontWeight.SemiBold,
      fontSize = 12.sp,
      maxLines = 1,
      overflow = TextOverflow.Ellipsis,
  )
}

/** 三个筛选标签的最小宽度：按内容取宽，同时保证三个胶囊等宽、不被拉伸到整行。 */
private val FilterButtonMinWidth = 64.dp

/** 分段高度与主题模式选择器保持一致（12sp 文字 + 上下留白）。 */
private val FilterButtonHeight = 34.dp

/** 用户切换菜单：宽度与点击热区按单手操作留余量，但保持窄菜单不遮挡列表。 */
