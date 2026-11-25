package org.srx.manager.ui.component

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.themedSurfaceColor
import org.srx.manager.ui.coui.couiCardFill
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import org.srx.manager.ui.theme.isSrxThemeCoui
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.basic.Search
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
fun SrxSearchField(
    query: String,
    onQueryChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
) {
  val colors = MiuixTheme.colorScheme
  // COUI 下跟着卡片色走：原来取 surface（页面底），浅色时是一块灰、深色时是一块更黑的灰，
  // 和它两侧的白色卡片不是同一个底色。
  val backgroundColor = if (isSrxThemeCoui()) couiCardFill() else themedSurfaceColor()
  BasicTextField(
      value = query,
      onValueChange = onQueryChange,
      singleLine = true,
      textStyle =
          TextStyle(
              fontWeight = FontWeight.Medium,
              fontSize = 14.sp,
              color = colors.onSurface,
          ),
      cursorBrush = SolidColor(colors.primary),
      keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
      modifier =
          modifier
              .fillMaxWidth()
              .heightIn(min = 50.dp)
              // COUI 走纯实色胶囊：液态玻璃那层是按 Miuix 的折射材质设计的，
              // 在 COUI 下给不透明 tint 会渲染成一块死白，给半透明又会透出页面底发灰。
              .then(
                  if (isSrxThemeCoui()) Modifier.background(backgroundColor, CircleShape)
                  else
                      Modifier.liquidGlassControl(
                          shape = CircleShape,
                          tint =
                              backgroundColor.copy(
                                  alpha = if (isSrxLiquidGlassEnabled()) 0.6f else 1f
                              ),
                          refractionHeight = 11.dp,
                          refractionAmount = 12.dp,
                      )
              ),
      decorationBox = { inner ->
        Row(
            modifier = Modifier.fillMaxWidth(),
            verticalAlignment = Alignment.CenterVertically,
        ) {
          Icon(
              imageVector = MiuixIcons.Basic.Search,
              contentDescription = null,
              modifier = Modifier.size(42.dp).padding(start = 16.dp, end = 8.dp),
              tint = colors.onSurfaceVariantSummary,
          )
          Box(modifier = Modifier.weight(1f)) {
            if (query.isBlank()) {
              Text(text = label, color = colors.onSurfaceVariantSummary, fontSize = 14.sp)
            }
            inner()
          }
          AnimatedVisibility(
              visible = query.isNotEmpty(),
              enter = fadeIn() + scaleIn(),
              exit = fadeOut() + scaleOut(),
          ) {
            // 用 Miuix 0.9.5-beta04 新增的 SearchClearButton：中性灰圆底 + 与 SearchBar
            // 统一的按压反馈，比原来直接挂一个可点图标更像"控件"而不是"图标碰巧能点"。
            top.yukonga.miuix.kmp.basic.SearchClearButton(
                onClick = { onQueryChange("") },
                modifier = Modifier.padding(start = 8.dp, end = 12.dp),
                contentDescription = "清除搜索内容",
            )
          }
        }
      },
  )
}
