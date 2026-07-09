package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.RoundIconAction
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Add
import top.yukonga.miuix.kmp.theme.MiuixTheme

private fun isAndroidDataOrObbMappingTarget(path: String): Boolean {
  val parts = path.trim().replace('\\', '/').trim('/').split('/').filter(String::isNotBlank)
  return parts.size >= 2 &&
      parts[0].equals("Android", ignoreCase = true) &&
      (parts[1].equals("data", ignoreCase = true) || parts[1].equals("obb", ignoreCase = true))
}

private fun mappingTargetError(path: String): String? =
    if (isAndroidDataOrObbMappingTarget(path)) {
      "映射目标不能位于 Android/data 或 Android/obb"
    } else {
      null
    }

@Composable
private fun PathInputDialog(
    show: Boolean,
    title: String,
    userId: String,
    placeholder: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
    initialValue: String = "",
    allowRuleSyntax: Boolean = false,
    allowWildcards: Boolean = allowRuleSyntax,
    confirmText: String = "添加",
    onDismiss: () -> Unit,
    onConfirm: (String) -> Unit,
) {
  var value by
      remember(show, initialValue, userId, allowRuleSyntax, allowWildcards) {
        mutableStateOf(
            pathTextFieldValue(normalizeEditablePathInput(initialValue, userId, allowRuleSyntax))
        )
      }
  CenteredDialog(
      title = title,
      summary = null,
      show = show,
      onDismiss = onDismiss,
  ) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
      TextField(
          value = value,
          onValueChange = {
            value = normalizeEditablePathTextFieldValue(it, userId, allowRuleSyntax)
          },
          label = placeholder,
          colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
          useLabelAsPlaceholder = true,
          singleLine = true,
          modifier = Modifier.fillMaxWidth(),
      )
      PathSuggestionBrowser(value.text, userId, onListDirectories) {
        value = pathTextFieldValue(it)
      }
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton(
            confirmText,
            { onConfirm(value.text) },
            modifier = Modifier.weight(1f),
            primary = true,
        )
      }
    }
  }
}

@Composable
private fun MappingInputDialog(
    show: Boolean,
    userId: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
    initialRequest: String = "",
    initialTarget: String = "",
    confirmText: String = "添加",
    title: String = "添加路径映射",
    onDismiss: () -> Unit,
    onConfirm: (String, String) -> Unit,
) {
  var from by
      remember(show, initialRequest, userId) {
        mutableStateOf(pathTextFieldValue(normalizeEditablePathInput(initialRequest, userId)))
      }
  var to by
      remember(show, initialTarget, userId) {
        mutableStateOf(pathTextFieldValue(normalizeEditablePathInput(initialTarget, userId)))
      }
  var activeField by remember(show) { mutableStateOf<MappingField?>(null) }
  var targetError by
      remember(show, initialTarget, userId) { mutableStateOf(mappingTargetError(to.text)) }
  CenteredDialog(
      title = title,
      summary = null,
      show = show,
      onDismiss = onDismiss,
  ) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
      TextField(
          value = from,
          onValueChange = { from = normalizeEditablePathTextFieldValue(it, userId) },
          label = "请求路径",
          colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
          useLabelAsPlaceholder = true,
          singleLine = true,
          modifier =
              Modifier.fillMaxWidth().onFocusChanged {
                if (it.isFocused) activeField = MappingField.From
              },
      )
      if (activeField == MappingField.From) {
        PathSuggestionBrowser(from.text, userId, onListDirectories) {
          from = pathTextFieldValue(it)
        }
      }
      TextField(
          value = to,
          onValueChange = {
            to = normalizeEditablePathTextFieldValue(it, userId)
            targetError = mappingTargetError(to.text)
          },
          label = "目标路径",
          colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
          useLabelAsPlaceholder = true,
          singleLine = true,
          modifier =
              Modifier.fillMaxWidth().onFocusChanged {
                if (it.isFocused) activeField = MappingField.To
              },
      )
      if (activeField == MappingField.To) {
        PathSuggestionBrowser(to.text, userId, onListDirectories) {
          to = pathTextFieldValue(it)
          targetError = mappingTargetError(to.text)
        }
      }
      targetError?.let { error ->
        Text(
            text = error,
            color = MiuixTheme.colorScheme.error,
            fontSize = 12.sp,
            lineHeight = 16.sp,
        )
      }
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton(
            confirmText,
            {
              val error = mappingTargetError(to.text)
              targetError = error
              if (error == null) onConfirm(from.text, to.text)
            },
            modifier = Modifier.weight(1f),
            primary = true,
        )
      }
    }
  }
}

@Composable
internal fun PathEditorCard(
    title: String,
    emptyHint: String,
    values: List<String>,
    addLabel: String,
    placeholder: String,
    userId: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
    onAdd: (String) -> Unit,
    onUpdate: (String, String) -> Unit,
    onRemove: (String) -> Unit,
    allowRuleSyntax: Boolean = false,
    allowWildcards: Boolean = allowRuleSyntax,
) {
  var showDialog by remember { mutableStateOf(false) }
  var editingValue by remember { mutableStateOf<String?>(null) }
  GlassCard(insideMargin = PaddingValues(0.dp), alpha = 0.52f, shadowAlpha = 0.14f) {
    ConfigGroupHeader(title = title, addLabel = addLabel, onAdd = { showDialog = true })
    if (values.isEmpty()) {
      EmptyConfigHint(emptyHint)
    } else {
      values.forEachIndexed { index, value ->
        RuleRow(
            value = value,
            onEdit = { editingValue = value },
            onRemove = onRemove,
        )
        if (index != values.lastIndex) ConfigDivider()
      }
    }
  }
  val editing = editingValue
  PathInputDialog(
      show = showDialog || editing != null,
      title = if (editing == null) addLabel else "编辑$title",
      userId = userId,
      placeholder = placeholder,
      onListDirectories = onListDirectories,
      initialValue = editing.orEmpty(),
      allowRuleSyntax = allowRuleSyntax,
      allowWildcards = allowWildcards,
      confirmText = if (editing == null) "添加" else "保存",
      onDismiss = {
        showDialog = false
        editingValue = null
      },
      onConfirm = {
        val oldValue = editing
        showDialog = false
        editingValue = null
        if (oldValue == null) onAdd(it) else onUpdate(oldValue, it)
      },
  )
}

@Composable
internal fun MappingEditorCard(
    mappings: Map<String, String>,
    userId: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
    onAdd: (String, String) -> Unit,
    onUpdate: (String, String, String) -> Unit,
    onRemove: (String) -> Unit,
) {
  var showDialog by remember { mutableStateOf(false) }
  var editingMapping by remember { mutableStateOf<Pair<String, String>?>(null) }
  GlassCard(insideMargin = PaddingValues(0.dp), alpha = 0.52f, shadowAlpha = 0.14f) {
    ConfigGroupHeader(title = "路径映射", addLabel = "添加路径映射", onAdd = { showDialog = true })
    if (mappings.isEmpty()) {
      EmptyConfigHint("将请求路径映射到目标路径")
    } else {
      mappings.entries.forEachIndexed { index, (request, target) ->
        MappingRow(
            request = request,
            target = target,
            onEdit = { editingMapping = request to target },
            onRemove = onRemove,
        )
        if (index != mappings.size - 1) ConfigDivider()
      }
    }
  }
  val editing = editingMapping
  MappingInputDialog(
      show = showDialog || editing != null,
      userId = userId,
      onListDirectories = onListDirectories,
      initialRequest = editing?.first.orEmpty(),
      initialTarget = editing?.second.orEmpty(),
      confirmText = if (editing == null) "添加" else "保存",
      title = if (editing == null) "添加路径映射" else "修改路径映射",
      onDismiss = {
        showDialog = false
        editingMapping = null
      },
      onConfirm = { from, to ->
        val oldRequest = editing?.first
        showDialog = false
        editingMapping = null
        if (oldRequest == null) onAdd(from, to) else onUpdate(oldRequest, from, to)
      },
  )
}

@Composable
internal fun ConfigGroupHeader(
    title: String,
    addLabel: String,
    onAdd: () -> Unit,
) {
  Row(
      modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 14.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(10.dp),
  ) {
    Text(
        text = title,
        modifier = Modifier.weight(1f),
        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        fontSize = 12.sp,
        lineHeight = 16.sp,
        fontWeight = FontWeight.Black,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
    RoundIconAction(
        icon = MiuixIcons.Add,
        contentDescription = addLabel,
        onClick = onAdd,
        size = 36.dp,
        iconSize = 17.dp,
    )
  }
  ConfigDivider()
}

@Composable
internal fun ConfigDivider() {
  Box(
      Modifier.fillMaxWidth()
          .height(1.dp)
          .background(
              MiuixTheme.colorScheme.onSurface.copy(
                  alpha = if (isSrxDarkTheme()) 0.035f else 0.045f
              )
          ),
  )
}

@Composable
internal fun EmptyConfigHint(text: String) {
  Text(
      text = text,
      modifier = Modifier.fillMaxWidth().padding(16.dp),
      color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
      fontSize = 12.sp,
      lineHeight = 18.sp,
      textAlign = TextAlign.Center,
  )
}

@Composable
private fun RuleRow(
    value: String,
    onEdit: () -> Unit,
    onRemove: (String) -> Unit,
) {
  Row(
      modifier =
          Modifier.fillMaxWidth()
              .clickable(onClick = onEdit)
              .padding(horizontal = 16.dp, vertical = 12.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(10.dp),
  ) {
    Text(
        text = value,
        modifier = Modifier.weight(1f),
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        color =
            if (value.startsWith("!")) {
              MiuixTheme.colorScheme.error
            } else {
              MiuixTheme.colorScheme.onSurface
            },
        fontSize = 12.sp,
        lineHeight = 16.sp,
    )
    RoundIconAction(
        Icons.Rounded.Close,
        "删除",
        { onRemove(value) },
        danger = true,
        size = 34.dp,
        iconSize = 17.dp,
    )
  }
}

@Composable
private fun MappingRow(
    request: String,
    target: String,
    onEdit: () -> Unit,
    onRemove: (String) -> Unit,
) {
  Row(
      modifier =
          Modifier.fillMaxWidth()
              .clickable(onClick = onEdit)
              .padding(horizontal = 16.dp, vertical = 12.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.spacedBy(10.dp),
  ) {
    Text(
        request,
        modifier = Modifier.weight(0.38f),
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        fontSize = 12.sp,
        lineHeight = 16.sp,
    )
    Text(
        "→",
        color = MiuixTheme.colorScheme.primary,
        fontSize = 15.sp,
        lineHeight = 18.sp,
    )
    Text(
        target,
        modifier = Modifier.weight(0.62f),
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
        color = MiuixTheme.colorScheme.onSurface,
        fontSize = 12.sp,
        lineHeight = 16.sp,
    )
    RoundIconAction(
        Icons.Rounded.Close,
        "删除",
        { onRemove(request) },
        danger = true,
        size = 34.dp,
        iconSize = 17.dp,
    )
  }
}
