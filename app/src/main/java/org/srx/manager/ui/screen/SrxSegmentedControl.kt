package org.srx.manager.ui.screen

import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.capsuleContainerColor
import org.srx.manager.capsuleSelectedColor
import org.srx.manager.srxPrimaryColor
import org.srx.manager.ui.theme.LocalSrxColorScheme
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Text

private val SegmentContainerShape = RoundedCornerShape(18.dp)
private val SegmentItemShape = RoundedCornerShape(14.dp)
private const val SEGMENT_ANIM_MS = 260

/**
 * 分段控件的统一实现（Miuix / COUI 共用）：等宽分项 + 一块会横移的选中胶囊。
 *
 * 之前两个主题各写一份，选中态是"直接换背景色"，切换时生硬且两边样式会漂移；改成同一个 实现后外观与动画天然一致：胶囊用 [capsuleSelectedColor]，文字选中色走统一真值源
 * [LocalSrxColorScheme]，因此在 COUI 分支里也不会读到 Miuix 的默认色板。
 *
 * [itemMinWidth] 为 null 时按可用宽度等分（主题模式选择器），非 null 时每项固定该宽度 （应用列表的"用户/系统/已配置"，不铺满整行）。
 */
@Composable
internal fun SrxSegmentedControl(
    labels: List<String>,
    selectedIndex: Int,
    onSelect: (Int) -> Unit,
    modifier: Modifier = Modifier,
    itemMinWidth: Dp? = null,
    itemHeight: Dp = 34.dp,
    containerPadding: Dp = 4.dp,
    itemGap: Dp = 4.dp,
) {
  if (labels.isEmpty()) return
  val scheme = LocalSrxColorScheme.current
  val index = selectedIndex.coerceIn(0, labels.lastIndex)
  BoxWithConstraints(
      modifier =
          modifier
              .clip(SegmentContainerShape)
              .background(capsuleContainerColor(), SegmentContainerShape)
              .padding(containerPadding),
  ) {
    val totalGap = itemGap * (labels.size - 1)
    val itemWidth = itemMinWidth ?: ((maxWidth - totalGap) / labels.size).coerceAtLeast(0.dp)
    val targetX = (itemWidth + itemGap) * index
    val x by
        animateDpAsState(
            targetValue = targetX,
            animationSpec = tween(SEGMENT_ANIM_MS, easing = FastOutSlowInEasing),
            label = "segmentX",
        )
    Box {
      // 选中胶囊画在文字下面，只做视觉指示，不接收点击。
      Box(
          Modifier.offset(x = x)
              .width(itemWidth)
              .height(itemHeight)
              .dropShadow(
                  SegmentItemShape,
                  Shadow(
                      radius = 10.dp,
                      color = scheme.primary,
                      alpha = if (isSrxDarkTheme()) 0.12f else 0.09f,
                  ),
              )
              .clip(SegmentItemShape)
              .background(capsuleSelectedColor(), SegmentItemShape)
      )
      Row(horizontalArrangement = Arrangement.spacedBy(itemGap)) {
        labels.forEachIndexed { itemIndex, label ->
          val selected = itemIndex == index
          // 关掉按压水波纹：选中的表达已经由滑块位置 + 文字颜色承担，
          // 再叠一层灰色高亮会先闪一下灰块、再等滑块动画过去。
          val pressSource = remember { MutableInteractionSource() }
          Box(
              modifier =
                  Modifier.width(itemWidth).height(itemHeight).clip(SegmentItemShape).clickable(
                      interactionSource = pressSource,
                      indication = null,
                  ) {
                    onSelect(itemIndex)
                  },
              contentAlignment = Alignment.Center,
          ) {
            Text(
                text = label,
                color = if (selected) srxPrimaryColor() else scheme.onSurface,
                fontWeight = if (selected) FontWeight.Black else FontWeight.SemiBold,
                fontSize = 12.sp,
                maxLines = 1,
            )
          }
        }
      }
    }
  }
}
