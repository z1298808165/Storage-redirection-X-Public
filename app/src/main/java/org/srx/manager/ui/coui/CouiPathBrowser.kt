package org.srx.manager.ui.coui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.suqi8.coui.kmp.basic.Card
import io.github.suqi8.coui.kmp.basic.Icon
import io.github.suqi8.coui.kmp.basic.Text
import io.github.suqi8.coui.kmp.icon.COUIIcons
import io.github.suqi8.coui.kmp.icon.extended.Folder
import org.srx.manager.ui.screen.applyPathPrefix
import org.srx.manager.ui.screen.rememberStorageBrowserState
import org.srx.manager.ui.theme.SrxShapes
import org.srx.manager.ui.theme.srxPrimaryTextColor
import org.srx.manager.ui.theme.srxSecondaryTextColor

/** COUI 主题的目录补全浏览器：复用与 Miuix 版相同的目录读取与补全逻辑，只把渲染换成 COUI 组件。 输入为公共存储相对路径时给出下一级目录建议，绝对路径则提示直接填写。 */
@Composable
internal fun CouiPathSuggestionBrowser(
    value: String,
    userId: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
    onPick: (String) -> Unit,
) {
  val absolutePath = value.trim().startsWith('/')
  val publicStorageAlias =
      value.trim().matches(Regex("^/(storage/emulated|data/media|sdcard)(/|$).*"))
  val loading = absolutePath && !publicStorageAlias
  val storageState = rememberStorageBrowserState(userId, value, onListDirectories)
  val browserState =
      if (loading) CouiBrowserState(emptyList(), false)
      else CouiBrowserState(storageState.suggestions, storageState.loading)
  Card(
      modifier = Modifier.fillMaxWidth().height(188.dp).couiPanelEdge(20.dp),
  ) {
    Box(modifier = Modifier.fillMaxSize().clip(RoundedCornerShape(SrxShapes.card))) {
      when {
        browserState.loading -> CouiBrowserMessage("正在读取目录...")
        browserState.suggestions.isEmpty() && loading ->
            CouiBrowserMessage("绝对路径请直接填写，目录提示仅适用于公共存储")
        browserState.suggestions.isEmpty() -> CouiBrowserMessage("没有可补全的下一级路径")
        else ->
            LazyColumn(modifier = Modifier.fillMaxSize()) {
              itemsIndexed(browserState.suggestions) { index, suggestion ->
                Column {
                  Row(
                      modifier =
                          Modifier.fillMaxWidth()
                              .clickable { onPick(applyPathPrefix(value, suggestion.relativePath)) }
                              .padding(horizontal = 12.dp, vertical = 10.dp),
                      verticalAlignment = Alignment.CenterVertically,
                      horizontalArrangement = Arrangement.spacedBy(8.dp),
                  ) {
                    Icon(
                        COUIIcons.Folder,
                        contentDescription = null,
                        modifier = Modifier.size(18.dp),
                        tint = srxSecondaryTextColor(),
                    )
                    Column(
                        Modifier.weight(1f),
                        verticalArrangement = Arrangement.spacedBy(1.dp),
                    ) {
                      Text(
                          text = suggestion.displayPath,
                          color = srxPrimaryTextColor(),
                          fontSize = 13.sp,
                          lineHeight = 17.sp,
                          maxLines = 1,
                          overflow = TextOverflow.Ellipsis,
                      )
                      if (
                          !suggestion.isParent && suggestion.relativePath != suggestion.displayPath
                      ) {
                        Text(
                            text = suggestion.relativePath,
                            color = srxSecondaryTextColor(),
                            fontSize = 11.sp,
                            lineHeight = 14.sp,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                      }
                    }
                  }
                }
              }
            }
      }
    }
  }
}

private data class CouiBrowserState(
    val suggestions: List<org.srx.manager.ui.screen.PathSuggestion>,
    val loading: Boolean,
)

@Composable
private fun CouiBrowserMessage(text: String) {
  Box(modifier = Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
    Text(
        text = text,
        color = srxSecondaryTextColor(),
        fontSize = 13.sp,
        lineHeight = 18.sp,
    )
  }
}
