package org.srx.manager.ui.screen

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
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
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.TransformOrigin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntRect
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.LayoutDirection
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Popup
import androidx.compose.ui.window.PopupPositionProvider
import androidx.compose.ui.window.PopupProperties
import kotlinx.coroutines.delay
import org.srx.manager.capsuleContainerColor
import org.srx.manager.capsuleSelectedColor
import org.srx.manager.data.AppFilter
import org.srx.manager.data.InstalledApp
import org.srx.manager.glassSurfaceColor
import org.srx.manager.srxDangerColor
import org.srx.manager.srxPrimaryColor
import org.srx.manager.srxSuccessColor
import org.srx.manager.ui.component.AppIconImage
import org.srx.manager.ui.component.liquidGlassControl
import org.srx.manager.ui.component.liquidPressScale
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Ok
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
        AppFilterGroup(filter = filter, onFilter = onFilter, modifier = Modifier.fillMaxWidth())
        Box(modifier = Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
          AppUserSwitcher(users = users, selectedUser = selectedUser, onUser = onUser)
        }
      }
    } else {
      Row(
          modifier = Modifier.fillMaxWidth(),
          horizontalArrangement = Arrangement.spacedBy(10.dp),
          verticalAlignment = Alignment.Top,
      ) {
        AppFilterGroup(filter = filter, onFilter = onFilter, modifier = Modifier.weight(1f))
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
  Row(
      modifier =
          modifier
              .liquidGlassControl(
                  shape = CircleShape,
                  tint = capsuleContainerColor(),
                  refractionHeight = 8.dp,
                  refractionAmount = 10.dp,
              )
              .padding(5.dp),
      horizontalArrangement = Arrangement.spacedBy(2.dp),
  ) {
    FilterButton("用户", filter == AppFilter.User, Modifier.weight(1f)) { onFilter(AppFilter.User) }
    FilterButton("系统", filter == AppFilter.System, Modifier.weight(1f)) {
      onFilter(AppFilter.System)
    }
    FilterButton("已配置", filter == AppFilter.Configured, Modifier.weight(1f)) {
      onFilter(AppFilter.Configured)
    }
  }
}

@Composable
internal fun FilterButton(
    label: String,
    selected: Boolean,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
  val color = if (selected) srxPrimaryColor() else MiuixTheme.colorScheme.onSurface
  Box(
      modifier =
          modifier
              .then(
                  if (selected) {
                    Modifier.dropShadow(
                        CircleShape,
                        Shadow(
                            radius = 12.dp,
                            color = if (isSrxDarkTheme()) Color.Black else Color(0xFF73839C),
                            alpha = if (isSrxDarkTheme()) 0.22f else 0.14f,
                        ),
                    )
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
internal fun AppUserSwitcher(
    users: List<String>,
    selectedUser: String,
    onUser: (String) -> Unit,
) {
  if (users.size <= 1) return
  var expanded by remember(users, selectedUser) { mutableStateOf(false) }
  // 关闭时保留一小段存活时间，让退场动画播完再移除 Popup。
  var menuAlive by remember { mutableStateOf(false) }
  LaunchedEffect(expanded) {
    if (expanded) {
      menuAlive = true
    } else if (menuAlive) {
      delay(MenuExitAnimMs.toLong())
      menuAlive = false
    }
  }
  val triggerInteraction = remember { MutableInteractionSource() }
  val menuGapPx = with(LocalDensity.current) { 8.dp.roundToPx() }
  val menuTransformOrigin = TransformOrigin(1f, 0f)
  val menuPositionProvider =
      remember(menuGapPx) {
        object : PopupPositionProvider {
          override fun calculatePosition(
              anchorBounds: IntRect,
              windowSize: IntSize,
              layoutDirection: LayoutDirection,
              popupContentSize: IntSize,
          ): IntOffset {
            // 菜单右边缘与触发按钮右边缘对齐，避免右对齐的按钮下挂一个偏左的菜单。
            val x = anchorBounds.right - popupContentSize.width
            val maxY = (windowSize.height - popupContentSize.height).coerceAtLeast(0)
            val y = (anchorBounds.bottom + menuGapPx).coerceIn(0, maxY)
            return IntOffset(x.coerceAtLeast(0), y)
          }
        }
      }
  Box {
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
    if (menuAlive) {
      // 用户菜单必须以浮层覆盖在列表之上：早先的内联展开会把下方应用列表整体推下去，
      // 切换用户时布局还会再跳一次，看起来像列表被挤走。
      Popup(
          popupPositionProvider = menuPositionProvider,
          onDismissRequest = { expanded = false },
          properties = PopupProperties(focusable = true),
      ) {
        AnimatedVisibility(
            visible = expanded,
            enter =
                fadeIn(tween(MenuEnterAnimMs)) +
                    scaleIn(
                        animationSpec = tween(MenuEnterAnimMs),
                        initialScale = 0.92f,
                        transformOrigin = menuTransformOrigin,
                    ),
            exit =
                fadeOut(tween(MenuExitAnimMs)) +
                    scaleOut(
                        animationSpec = tween(MenuExitAnimMs),
                        targetScale = 0.94f,
                        transformOrigin = menuTransformOrigin,
                    ),
        ) {
          Column(
              modifier =
                  Modifier.width(MenuMinWidth)
                      .liquidGlassControl(
                          shape = RoundedCornerShape(22.dp),
                          tint = glassSurfaceColor(0.94f),
                          refractionHeight = 14.dp,
                          refractionAmount = 16.dp,
                      )
                      .padding(MenuPadding),
              verticalArrangement = Arrangement.spacedBy(MenuItemGap),
          ) {
            users.forEach { user ->
              val selected = user == selectedUser
              Text(
                  text = "用户 $user",
                  modifier =
                      Modifier.fillMaxWidth()
                          .clip(RoundedCornerShape(16.dp))
                          .background(if (selected) capsuleSelectedColor() else Color.Transparent)
                          .clickable {
                            expanded = false
                            onUser(user)
                          }
                          .padding(horizontal = 16.dp, vertical = 13.dp),
                  color =
                      if (selected) srxPrimaryColor()
                      else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                  fontWeight = FontWeight.Bold,
                  fontSize = 13.sp,
                  textAlign = TextAlign.End,
              )
            }
          }
        }
      }
    }
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
          modifier = Modifier.size(46.dp),
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
                .padding(horizontal = 16.dp, vertical = 15.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(13.dp),
    ) {
      AppIconImage(appInfo = app.appInfo, label = app.label, modifier = Modifier.size(46.dp))
      Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(
            app.label,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            color = primaryTextColor,
            fontWeight = FontWeight.Bold,
            fontSize = 16.sp,
            lineHeight = 20.sp,
        )
        Text(
            app.packageName,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            color = secondaryTextColor,
            fontSize = 12.sp,
            lineHeight = 16.sp,
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
          MiuixIcons.Ok,
          contentDescription = "已选择",
          tint = Color.White,
          modifier = Modifier.size(16.dp),
      )
    }
  }
}

@Composable
internal fun StatusPill(app: InstalledApp) {
  val (text, color) =
      when {
        app.isMissing -> "应用已卸载" to srxDangerColor()
        app.isEnabled -> "已启用" to srxSuccessColor()
        app.isConfigured -> "已配置" to srxPrimaryColor()
        else -> "未配置" to MiuixTheme.colorScheme.onSurfaceVariantSummary
      }
  Text(
      text = text,
      modifier =
          Modifier.clip(CircleShape)
              .background(color.copy(alpha = 0.12f))
              .padding(horizontal = 10.dp, vertical = 6.dp),
      color = color,
      fontWeight = FontWeight.SemiBold,
      fontSize = 12.sp,
      maxLines = 1,
      overflow = TextOverflow.Ellipsis,
  )
}

/** 用户切换菜单：宽度与点击热区按单手操作留余量，但保持窄菜单不遮挡列表。 */
private val MenuMinWidth = 132.dp
private val MenuPadding = 6.dp
private val MenuItemGap = 4.dp
private const val MenuEnterAnimMs = 160
private const val MenuExitAnimMs = 120
