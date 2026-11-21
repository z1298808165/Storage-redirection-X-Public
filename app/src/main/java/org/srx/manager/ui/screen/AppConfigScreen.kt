package org.srx.manager.ui.screen

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassTextButton
import org.srx.manager.RoundIconAction
import org.srx.manager.SettingGroup
import org.srx.manager.SrxProgressiveListPage
import org.srx.manager.data.AppConfig
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UserProfile
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.AppUiState
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Delete
import top.yukonga.miuix.kmp.icon.glass.Ok
import top.yukonga.miuix.kmp.icon.glass.Tune

@Composable
private fun TemplateNameDialog(
    show: Boolean,
    title: String,
    confirmText: String,
    initialName: String = "",
    onDismiss: () -> Unit,
    onConfirm: (String) -> Unit,
) {
  var name by remember(show, initialName) { mutableStateOf(initialName) }
  CenteredDialog(
      title = title,
      show = show,
      onDismiss = onDismiss,
  ) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
      TextField(
          value = name,
          onValueChange = { name = it.take(48) },
          label = "模板名称",
          colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
          useLabelAsPlaceholder = true,
          singleLine = true,
          modifier = Modifier.fillMaxWidth(),
      )
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
        GlassTextButton(
            confirmText,
            { onConfirm(name.trim()) },
            modifier = Modifier.weight(1f),
            primary = true,
        )
      }
    }
  }
}

@Composable
internal fun AppConfigScreen(
    state: AppUiState,
    app: InstalledApp,
    config: AppConfig,
    prefs: UiPreferences,
    onBack: () -> Unit,
    onSave: () -> Unit,
    onDelete: () -> Unit,
    onSaveTemplate: (String) -> Unit,
    onApplyTemplate: (String) -> Unit,
    onProfileChange: ((UserProfile) -> UserProfile) -> Unit,
    onAddAllowed: (String) -> Unit,
    onAddSandbox: (String) -> Unit,
    onUpdateAllowed: (String, String) -> Unit,
    onUpdateSandbox: (String, String) -> Unit,
    onRemoveAllowed: (String) -> Unit,
    onRemoveSandbox: (String) -> Unit,
    onSetReadOnlyEnabled: (Boolean) -> Unit,
    onAddReadOnly: (String) -> Unit,
    onUpdateReadOnly: (String, String) -> Unit,
    onRemoveReadOnly: (String) -> Unit,
    onAddMapping: (String, String) -> Unit,
    onUpdateMapping: (String, String, String) -> Unit,
    onRemoveMapping: (String) -> Unit,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
) {
  val profile = config.users[state.selectedUser] ?: DisabledDefaultProfile
  var confirmDelete by remember { mutableStateOf(false) }
  var showTemplateActions by remember { mutableStateOf(false) }
  var showSaveTemplate by remember { mutableStateOf(false) }
  var showApplyTemplate by remember { mutableStateOf(false) }
  var readOnlyEditorEnabled by
      rememberSaveable(app.packageName, state.selectedUser) {
        mutableStateOf(profile.readOnlyPaths.isNotEmpty())
      }
  LaunchedEffect(profile.readOnlyPaths) {
    if (profile.readOnlyPaths.isNotEmpty()) {
      readOnlyEditorEnabled = true
    }
  }
  var pendingTemplate by remember { mutableStateOf<ConfigTemplate?>(null) }
  val canDelete = app.isConfigured || config.users.values.any { it != DisabledDefaultProfile }
  // 与检查更新、主题设置等二级页一致：使用统一的外部顶栏，操作按钮作为 actions，
  // 不再单独做一张悬浮 header 卡片，保持 Miuix 主题风格统一。
  SrxProgressiveListPage(
      title = app.label,
      subtitle = "用户 ${state.selectedUser}",
      onBack = onBack,
      topSpacing = 8.dp,
      actions = {
        RoundIconAction(
            icon = MiuixIcons.Glass.Tune,
            contentDescription = "配置模板",
            onClick = { showTemplateActions = true },
        )
        RoundIconAction(
            icon = MiuixIcons.Glass.Delete,
            contentDescription = "删除配置",
            onClick = { confirmDelete = true },
            danger = true,
            enabled = canDelete,
        )
        if (!state.dashboard.globalConfig.appConfigAutoSave) {
          RoundIconAction(
              icon = MiuixIcons.Glass.Ok,
              contentDescription = "保存配置",
              onClick = onSave,
          )
        }
      },
  ) {
    item {
      SettingGroup() {
        item {
          CompactSwitchRow(
              title = "启用重定向",
              summary = "控制用户 ${state.selectedUser} 下该应用是否启用存储重定向X",
              checked = profile.enabled,
              onCheckedChange = { checked -> onProfileChange { it.copy(enabled = checked) } },
          )
        }
        item {
          CompactSwitchRow(
              title = "仅映射模式",
              summary = "仅应用显式路径映射；未命中映射时保持原路径",
              checked = profile.mappingModeOnly,
              onCheckedChange = { checked ->
                onProfileChange { it.copy(mappingModeOnly = checked) }
              },
          )
        }
        item {
          CompactSwitchRow(
              title = "只读模式",
              summary = "禁止写入指定真实目录；namespace 会退化通配规则，FUSE 数据面可精确匹配",
              checked = readOnlyEditorEnabled || profile.readOnlyPaths.isNotEmpty(),
              onCheckedChange = { checked ->
                readOnlyEditorEnabled = checked
                onSetReadOnlyEnabled(checked)
              },
              showDivider = false,
          )
        }
      }
    }
    item {
      PathEditorCard(
          title = "允许路径",
          emptyHint = "允许路径可直接访问；仅支持共享存储根下的相对路径；! 可排除子路径，* 和 ? 在默认方案下会退化匹配",
          values = profile.allowedRealPaths,
          addLabel = "添加允许路径",
          placeholder = "路径",
          userId = state.selectedUser,
          onListDirectories = onListDirectories,
          onAdd = onAddAllowed,
          onUpdate = onUpdateAllowed,
          onRemove = onRemoveAllowed,
          allowRuleSyntax = true,
          boundaryHint = "边界：填写共享存储根下的相对路径，例如 Download/MyApp。",
      )
    }
    if (readOnlyEditorEnabled || profile.readOnlyPaths.isNotEmpty()) {
      item {
        PathEditorCard(
            title = "只读路径",
            emptyHint = "只读路径保持可读但禁止写入；仅支持共享存储根下的相对路径；可用 ! 排除子路径，默认方案会退化通配",
            values = profile.readOnlyPaths,
            addLabel = "添加只读路径",
            placeholder = "路径或通配符",
            userId = state.selectedUser,
            onListDirectories = onListDirectories,
            onAdd = onAddReadOnly,
            onUpdate = onUpdateReadOnly,
            onRemove = onRemoveReadOnly,
            allowRuleSyntax = true,
            allowWildcards = true,
            boundaryHint = "边界：填写共享存储根下的相对路径，例如 Pictures/Original；可用 *、? 及 ! 排除子路径。",
        )
      }
    }
    if (profile.mappingModeOnly) {
      item {
        PathEditorCard(
            title = "沙盒路径",
            emptyHint = "仅映射模式下，未命中映射且匹配沙盒路径时将进入应用沙盒；支持 *、? 通配及 ! 排除子路径",
            values = profile.sandboxedPaths,
            addLabel = "添加沙盒路径",
            placeholder = "路径",
            userId = state.selectedUser,
            onListDirectories = onListDirectories,
            onAdd = onAddSandbox,
            onUpdate = onUpdateSandbox,
            onRemove = onRemoveSandbox,
            allowRuleSyntax = true,
            allowWildcards = true,
            boundaryHint = "边界：填写共享存储根下的相对路径；支持 *、? 通配及 ! 排除子路径。",
        )
      }
    }
    item {
      MappingEditorCard(
          mappings = profile.pathMappings,
          userId = state.selectedUser,
          onListDirectories = onListDirectories,
          onAdd = onAddMapping,
          onUpdate = onUpdateMapping,
          onRemove = onRemoveMapping,
      )
    }
  }

  CenteredDialog(
      title = "删除配置",
      summary = "确认删除 ${app.label} 的配置？此操作会移除对应 JSON 文件。",
      show = confirmDelete,
      onDismiss = { confirmDelete = false },
  ) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
      GlassTextButton("取消", { confirmDelete = false }, modifier = Modifier.weight(1f))
      GlassTextButton(
          "删除",
          {
            confirmDelete = false
            onDelete()
          },
          modifier = Modifier.weight(1f),
          danger = true,
      )
    }
  }

  CenteredDialog(
      title = "配置模板",
      summary = "将当前应用配置保存为模板，或用已有模板覆盖当前应用配置。",
      show = showTemplateActions,
      onDismiss = { showTemplateActions = false },
  ) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
      GlassTextButton(
          "应用已有模板",
          {
            showTemplateActions = false
            showApplyTemplate = true
          },
          modifier = Modifier.fillMaxWidth(),
      )
      GlassTextButton(
          "保存为模板",
          {
            showTemplateActions = false
            showSaveTemplate = true
          },
          modifier = Modifier.fillMaxWidth(),
          primary = true,
      )
    }
  }
  TemplateNameDialog(
      show = showSaveTemplate,
      title = "保存为模板",
      confirmText = "保存",
      onDismiss = { showSaveTemplate = false },
      onConfirm = {
        showSaveTemplate = false
        onSaveTemplate(it)
      },
  )
  TemplatePickerDialog(
      show = showApplyTemplate,
      templates = state.templates,
      title = "应用配置模板",
      emptyText = "还没有配置模板",
      onDismiss = { showApplyTemplate = false },
      onPick = {
        pendingTemplate = it
        showApplyTemplate = false
      },
  )
  pendingTemplate?.let { template ->
    CenteredDialog(
        title = "应用模板",
        summary = "将模板“${template.name}”应用到 ${app.label}，当前配置会被覆盖。",
        show = true,
        onDismiss = { pendingTemplate = null },
    ) {
      Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        GlassTextButton("取消", { pendingTemplate = null }, modifier = Modifier.weight(1f))
        GlassTextButton(
            "应用",
            {
              onApplyTemplate(template.id)
              pendingTemplate = null
            },
            modifier = Modifier.weight(1f),
            primary = true,
        )
      }
    }
  }
}
