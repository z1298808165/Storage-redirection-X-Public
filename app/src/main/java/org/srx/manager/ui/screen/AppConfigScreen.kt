package org.srx.manager.ui.screen

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Save
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.EmptyText
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.LocalSrxBackdrop
import org.srx.manager.RoundIconAction
import org.srx.manager.SectionTitle
import org.srx.manager.data.AppConfig
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.InstalledApp
import org.srx.manager.data.LogEntry
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UserProfile
import org.srx.manager.floatingGlassPanel
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.liquid.CombinedBackdrop
import org.srx.manager.ui.theme.isSrxBlurEffectEnabled
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.blur.rememberLayerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

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
private fun LogsPreview(logs: List<LogEntry>) {
  var showFullTime by rememberSaveable { mutableStateOf(false) }
  SectionTitle("最近文件监视")
  GlassCard(
      insideMargin = PaddingValues(0.dp),
      cornerRadius = 24.dp,
      alpha = 0.66f,
  ) {
    if (logs.isEmpty()) {
      EmptyText("暂无文件操作记录")
    } else {
      logs.take(4).forEach {
        LogRow(
            entry = it,
            showFullTime = showFullTime,
            onToggleTime = { showFullTime = !showFullTime },
        )
      }
    }
  }
}

@Composable
private fun AppConfigHeader(
    title: String,
    subtitle: String,
    canDelete: Boolean,
    showSave: Boolean,
    onBack: () -> Unit,
    onDelete: () -> Unit,
    onTemplate: () -> Unit,
    onSave: () -> Unit,
) {
  Box(
      modifier =
          Modifier.fillMaxWidth()
              .padding(
                  top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp,
                  start = 12.dp,
                  end = 12.dp,
                  bottom = 4.dp,
              ),
  ) {
    val shape = RoundedCornerShape(24.dp)
    val useBackdrop =
        isSrxLiquidGlassEnabled() && isSrxBlurEffectEnabled() && LocalSrxBackdrop.current != null
    Card(
        modifier = Modifier.fillMaxWidth().floatingGlassPanel(shape),
        cornerRadius = 24.dp,
        insideMargin = PaddingValues(horizontal = 10.dp, vertical = 10.dp),
        colors =
            CardDefaults.defaultColors(
                color =
                    if (useBackdrop) Color.Transparent
                    else MiuixTheme.colorScheme.surfaceContainerHigh,
            ),
    ) {
      Row(
          modifier = Modifier.fillMaxWidth(),
          verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
          horizontalArrangement = Arrangement.spacedBy(10.dp),
      ) {
        RoundIconAction(MiuixIcons.Back, "返回", onBack, size = 38.dp, iconSize = 19.dp)
        Column(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
          Text(
              text = title,
              fontSize = 18.sp,
              lineHeight = 22.sp,
              fontWeight = FontWeight.Black,
              maxLines = 1,
              overflow = TextOverflow.Ellipsis,
          )
          Text(
              text = subtitle,
              color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
              fontSize = 11.sp,
              lineHeight = 13.sp,
              fontWeight = FontWeight.SemiBold,
              maxLines = 1,
          )
        }
        RoundIconAction(
            icon = MiuixIcons.Tune,
            contentDescription = "配置模板",
            onClick = onTemplate,
            size = 38.dp,
            iconSize = 19.dp,
        )
        RoundIconAction(
            icon = MiuixIcons.Delete,
            contentDescription = "删除配置",
            onClick = onDelete,
            danger = true,
            enabled = canDelete,
            size = 38.dp,
            iconSize = 19.dp,
        )
        if (showSave) {
          RoundIconAction(
              icon = Icons.Rounded.Save,
              contentDescription = "保存配置",
              onClick = onSave,
              size = 38.dp,
              iconSize = 19.dp,
          )
        }
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
  val pageContentBackdrop = rememberLayerBackdrop()
  val baseBackdrop = LocalSrxBackdrop.current
  val headerBackdrop =
      remember(baseBackdrop, pageContentBackdrop) {
        baseBackdrop?.let { CombinedBackdrop(it, pageContentBackdrop) }
      }

  Scaffold(
      topBar = {
        CompositionLocalProvider(LocalSrxBackdrop provides headerBackdrop) {
          AppConfigHeader(
              title = app.label,
              subtitle = "用户 ${state.selectedUser}",
              canDelete = canDelete,
              showSave = !state.dashboard.globalConfig.appConfigAutoSave,
              onBack = onBack,
              onDelete = { confirmDelete = true },
              onTemplate = { showTemplateActions = true },
              onSave = onSave,
          )
        }
      },
  ) { padding ->
    Box(
        modifier =
            Modifier.fillMaxSize()
                .then(
                    if (baseBackdrop != null) Modifier.layerBackdrop(pageContentBackdrop)
                    else Modifier
                ),
    ) {
      LazyColumn(
          modifier = Modifier.fillMaxSize().overScrollVertical(),
          contentPadding =
              PaddingValues(
                  top = padding.calculateTopPadding() + 8.dp,
                  bottom =
                      WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() +
                          28.dp,
                  start = 16.dp,
                  end = 16.dp,
              ),
          verticalArrangement = Arrangement.spacedBy(14.dp),
      ) {
        item {
          GlassCard(alpha = 0.52f, shadowAlpha = 0.14f) {
            CompactSwitchRow(
                title = "启用重定向",
                summary = "控制用户 ${state.selectedUser} 下该应用是否启用存储重定向X",
                checked = profile.enabled,
                onCheckedChange = { checked -> onProfileChange { it.copy(enabled = checked) } },
            )
            CompactSwitchRow(
                title = "仅映射模式",
                summary = "仅应用显式路径映射；未命中映射时保持原路径",
                checked = profile.mappingModeOnly,
                onCheckedChange = { checked ->
                  onProfileChange { it.copy(mappingModeOnly = checked) }
                },
            )
            CompactSwitchRow(
                title = "只读模式",
                summary = "禁止写入指定真实目录；默认方案会退化通配规则，FUSE daemon 可精确匹配",
                checked = readOnlyEditorEnabled || profile.readOnlyPaths.isNotEmpty(),
                onCheckedChange = { checked ->
                  readOnlyEditorEnabled = checked
                  onSetReadOnlyEnabled(checked)
                },
                showDivider = false,
            )
          }
        }
        item {
          PathEditorCard(
              title = "允许路径",
              emptyHint = "允许路径可直接访问；! 可排除子路径，* 和 ? 在默认方案下会退化匹配",
              values = profile.allowedRealPaths,
              addLabel = "添加允许路径",
              placeholder = "路径",
              userId = state.selectedUser,
              onListDirectories = onListDirectories,
              onAdd = onAddAllowed,
              onUpdate = onUpdateAllowed,
              onRemove = onRemoveAllowed,
              allowRuleSyntax = true,
          )
        }
        if (readOnlyEditorEnabled || profile.readOnlyPaths.isNotEmpty()) {
          item {
            PathEditorCard(
                title = "只读路径",
                emptyHint = "只读路径保持可读但禁止写入；可用 ! 排除子路径，默认方案会退化通配",
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
            )
          }
        }
        if (profile.mappingModeOnly) {
          item {
            PathEditorCard(
                title = "沙盒路径",
                emptyHint = "仅映射模式下，未命中映射且匹配沙盒路径时将进入应用沙盒",
                values = profile.sandboxedPaths,
                addLabel = "添加沙盒路径",
                placeholder = "路径",
                userId = state.selectedUser,
                onListDirectories = onListDirectories,
                onAdd = onAddSandbox,
                onUpdate = onUpdateSandbox,
                onRemove = onRemoveSandbox,
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
