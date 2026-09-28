package org.srx.manager.ui.screen

import androidx.compose.animation.AnimatedVisibility
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
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
  val triggerInteraction = remember { MutableInteractionSource() }
  Column(horizontalAlignment = Alignment.End) {
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
    AnimatedVisibility(visible = expanded) {
      Column(
          modifier =
              Modifier.padding(top = 8.dp)
                  .liquidGlassControl(
                      shape = RoundedCornerShape(20.dp),
                      tint = glassSurfaceColor(0.88f),
                      refractionHeight = 12.dp,
                      refractionAmount = 14.dp,
                  )
                  .padding(6.dp),
          verticalArrangement = Arrangement.spacedBy(4.dp),
      ) {
        users.forEach { user ->
          val selected = user == selectedUser
          Text(
              text = "用户 $user",
              modifier =
                  Modifier.clip(RoundedCornerShape(15.dp))
                      .background(if (selected) capsuleSelectedColor() else Color.Transparent)
                      .clickable {
                        expanded = false
                        onUser(user)
                      }
                      .padding(horizontal = 12.dp, vertical = 10.dp),
              color =
                  if (selected) srxPrimaryColor()
                  else MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontWeight = FontWeight.Bold,
              fontSize = 12.sp,
          )
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
          Icons.Rounded.Check,
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
