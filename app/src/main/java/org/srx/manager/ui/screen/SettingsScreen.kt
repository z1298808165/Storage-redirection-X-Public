package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassTextButton
import org.srx.manager.RoundIconAction
import org.srx.manager.SectionTitle
import org.srx.manager.SettingGroup
import org.srx.manager.SrxProgressiveListPage
import org.srx.manager.data.AppConfig
import org.srx.manager.data.ConfigDir
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.GlobalConfig
import org.srx.manager.data.UiPreferences
import org.srx.manager.glassSurfaceColor
import org.srx.manager.srxSuccessColor
import org.srx.manager.ui.AppUiState
import org.srx.manager.ui.component.ShatterRow
import org.srx.manager.ui.theme.SrxRowMetrics
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.glass.Add
import top.yukonga.miuix.kmp.icon.glass.Download
import top.yukonga.miuix.kmp.icon.glass.File
import top.yukonga.miuix.kmp.icon.glass.Import
import top.yukonga.miuix.kmp.icon.glass.ListView
import top.yukonga.miuix.kmp.icon.glass.SettingsOutline
import top.yukonga.miuix.kmp.icon.glass.Tune
import top.yukonga.miuix.kmp.icon.glass.Update
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
private fun BackupActionButton(
    text: String,
    icon: ImageVector,
    tint: Color,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
  Row(
      modifier =
          modifier
              .heightIn(min = 46.dp)
              .clip(RoundedCornerShape(16.dp))
              .background(glassSurfaceColor(0.78f), RoundedCornerShape(16.dp))
              .clickable(onClick = onClick)
              .padding(horizontal = 12.dp, vertical = 11.dp),
      verticalAlignment = Alignment.CenterVertically,
      horizontalArrangement = Arrangement.Center,
  ) {
    Icon(icon, contentDescription = text, tint = tint, modifier = Modifier.size(17.dp))
    Spacer(Modifier.width(8.dp))
    Text(
        text,
        color = tint,
        fontSize = 13.sp,
        fontWeight = FontWeight.Black,
        maxLines = 1,
        overflow = TextOverflow.Ellipsis,
    )
  }
}

@Composable
internal fun SettingsScreen(
    state: AppUiState,
    prefs: UiPreferences,
    bottomPadding: androidx.compose.ui.unit.Dp,
    onGlobal: (GlobalConfig) -> Unit,
    onSaveTemplate: (ConfigTemplate) -> Unit,
    onDeleteTemplate: (String) -> Unit,
    onOpenTheme: () -> Unit,
    onBackupExport: () -> Unit,
    onBackupImport: () -> Unit,
    onDiagnosticExport: () -> Unit,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
) {
  val global = state.dashboard.globalConfig
  var deleteTemplate by remember { mutableStateOf<ConfigTemplate?>(null) }
  var shatteringTemplateId by remember { mutableStateOf<String?>(null) }
  var editingTemplate by remember { mutableStateOf<ConfigTemplate?>(null) }
  var showAutoTemplatePicker by remember { mutableStateOf(false) }
  var showAutoTemplateEmptyEnable by remember { mutableStateOf(false) }
  var showAutoTemplateEmptyInfo by remember { mutableStateOf(false) }
  val autoTemplateInUseId = global.autoEnableNewAppsTemplateId
  val autoTemplate =
      remember(state.templates, global.autoEnableNewAppsTemplateId) {
        state.templates.firstOrNull { it.id == global.autoEnableNewAppsTemplateId }
      }
  fun updateAutoEnable(checked: Boolean) {
    if (!checked) {
      onGlobal(global.copy(autoEnableRedirectForNewApps = false))
      return
    }
    when {
      global.autoEnableNewAppsTemplateId.isNotBlank() ->
          onGlobal(global.copy(autoEnableRedirectForNewApps = true))
      state.templates.isEmpty() -> showAutoTemplateEmptyEnable = true
      else -> showAutoTemplatePicker = true
    }
  }
  fun openAutoTemplatePicker() {
    if (state.templates.isEmpty() && global.autoEnableNewAppsTemplateId.isBlank()) {
      showAutoTemplateEmptyInfo = true
    } else {
      showAutoTemplatePicker = true
    }
  }
  fun newTemplateDraft() =
      ConfigTemplate(
          id = "template-${System.currentTimeMillis()}",
          name = "新配置模板",
          config = AppConfig(users = mapOf(state.selectedUser to DisabledDefaultProfile)),
      )
  SrxProgressiveListPage(
      title = "设置",
      bottomPadding = bottomPadding,
      verticalArrangement = Arrangement.spacedBy(SrxRowMetrics.groupSpacing()),
      actions = {
        RoundIconAction(
            icon = MiuixIcons.Glass.File,
            contentDescription = "导出日志包",
            onClick = onDiagnosticExport,
        )
      },
  ) {
    item {
      SectionTitle("模块设置")
      SettingGroup() {
        item {
          CompactSwitchRow(
              title = "文件监视",
              icon = MiuixIcons.Glass.ListView,
              checked = global.fileMonitorEnabled,
              onCheckedChange = { onGlobal(global.copy(fileMonitorEnabled = it)) },
          )
        }
        item {
          CompactSwitchRow(
              title = "Fuse Fixer",
              icon = MiuixIcons.Glass.Tune,
              checked = global.fuseFixEnabled,
              onCheckedChange = { onGlobal(global.copy(fuseFixEnabled = it)) },
          )
        }
        item {
          CompactSwitchRow(
              title = "详细日志",
              icon = MiuixIcons.Glass.File,
              checked = global.verboseLoggingEnabled,
              onCheckedChange = { onGlobal(global.copy(verboseLoggingEnabled = it)) },
          )
        }
        item {
          CompactSwitchRow(
              title = "新应用自动重定向",
              icon = MiuixIcons.Glass.Add,
              checked = global.autoEnableRedirectForNewApps,
              onCheckedChange = ::updateAutoEnable,
              showDivider = !global.autoEnableRedirectForNewApps,
          )
        }
        // 行数保持恒定，显隐在行内判断：分组的 item 增删不可靠（关闭开关后行不消失）。
        item {
          if (global.autoEnableRedirectForNewApps) {
            AutoRedirectTemplateStatusRow(
                template = autoTemplate,
                templateId = global.autoEnableNewAppsTemplateId,
                fallbackNoticeId = state.autoTemplateFallbackNoticeId,
                onClick = ::openAutoTemplatePicker,
            )
          }
        }
        item {
          CompactSwitchRow(
              title = "配置操作即时保存",
              icon = MiuixIcons.Glass.Update,
              checked = global.appConfigAutoSave,
              onCheckedChange = { onGlobal(global.copy(appConfigAutoSave = it)) },
              showDivider = false,
          )
        }
      }
    }
    item {
      SectionTitle("外观")
      SettingGroup() {
        item {
          SettingSelectRow(
              title = "主题与外观",
              leading = { SettingsRowIcon(MiuixIcons.Glass.SettingsOutline) },
              value = themeModeLabel(prefs.themeMode),
              onClick = onOpenTheme,
              showDivider = false,
          )
        }
      }
    }
    item {
      SectionTitle("配置模板")
      SettingGroup(insideMargin = PaddingValues(0.dp)) {
        item {
          ConfigGroupHeader(
              "模板库",
              addLabel = "添加配置模板",
              onAdd = { editingTemplate = newTemplateDraft() },
          )
        }
        if (state.templates.isEmpty()) {
          item { EmptyConfigHint("还没有配置模板，可从应用配置页保存当前配置为模板") }
        } else {
          item {
            LazyColumn(
                modifier = Modifier.fillMaxWidth().heightIn(max = 360.dp),
            ) {
              itemsIndexed(state.templates, key = { _, template -> template.id }) { index, template
                ->
                ShatterRow(
                    shattering = shatteringTemplateId == template.id,
                    onFinished = {
                      onDeleteTemplate(template.id)
                      shatteringTemplateId = null
                    },
                ) {
                  TemplateManageRow(
                      template = template,
                      onEdit = { editingTemplate = template },
                      onDelete = { deleteTemplate = template },
                  )
                }
                if (index != state.templates.lastIndex) ConfigDivider()
              }
            }
          }
        }
      }
    }
    item {
      SectionTitle("权限")
      SettingGroup(insideMargin = PaddingValues(16.dp)) {
        // 非行内容也必须进 item{}：分组只渲染 item 里的内容，
        // 直接写在这里会落到卡片之外（表现为"文本在卡片下方"）。
        item {
          Column {
            Text(
                text = if (state.rootGranted) "root 权限已授予" else "未获得 root 权限",
                color =
                    if (state.rootGranted) MiuixTheme.colorScheme.primary
                    else MiuixTheme.colorScheme.error,
                fontWeight = FontWeight.Bold,
            )
            Spacer(Modifier.height(8.dp))
            Text(
                "路径补全优先使用 root 读取目录；模块状态、配置读写、日志读取和备份还原等管理操作也依赖 root 执行。",
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 13.sp,
                lineHeight = 18.sp,
            )
          }
        }
      }
    }
    item {
      SectionTitle("备份还原")
      SettingGroup(insideMargin = PaddingValues(12.dp)) {
        item {
          Column {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
              BackupActionButton(
                  text = "备份",
                  icon = MiuixIcons.Glass.Download,
                  tint = MiuixTheme.colorScheme.primary,
                  onClick = onBackupExport,
                  modifier = Modifier.weight(1f),
              )
              BackupActionButton(
                  text = "还原",
                  icon = MiuixIcons.Glass.Import,
                  tint = srxSuccessColor(),
                  onClick = onBackupImport,
                  modifier = Modifier.weight(1f),
              )
            }
            Spacer(Modifier.height(10.dp))
            Text(
                "备份会导出一个可传播的单文件，包含全局设置、所有应用配置、配置模板、文件监视过滤、外观偏好和检查更新设置；还原前会校验格式、模块标识和配置字段。",
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 11.sp,
                lineHeight = 15.sp,
            )
            Spacer(Modifier.height(12.dp))
            Text("配置文件路径", fontWeight = FontWeight.Bold, fontSize = 13.sp)
            Spacer(Modifier.height(4.dp))
            Text(
                "$ConfigDir/",
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 11.sp,
                lineHeight = 14.sp,
            )
          }
        }
      }
    }
  }
  AutoTemplatePickerDialog(
      show = showAutoTemplatePicker,
      templates = state.templates,
      currentTemplateId = global.autoEnableNewAppsTemplateId,
      onDismiss = { showAutoTemplatePicker = false },
      onPick = { templateId ->
        showAutoTemplatePicker = false
        onGlobal(
            global.copy(
                autoEnableRedirectForNewApps = true,
                autoEnableNewAppsTemplateId = templateId,
            )
        )
      },
  )
  CenteredDialog(
      title = "没有配置模板",
      summary = "当前还没有配置模板。继续开启后，新安装应用会默认只开启重定向，不会附加允许路径、沙盒路径或映射规则。",
      show = showAutoTemplateEmptyEnable,
      onDismiss = { showAutoTemplateEmptyEnable = false },
  ) {
    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
      GlassTextButton("取消", { showAutoTemplateEmptyEnable = false }, modifier = Modifier.weight(1f))
      GlassTextButton(
          "继续开启",
          {
            showAutoTemplateEmptyEnable = false
            onGlobal(
                global.copy(autoEnableRedirectForNewApps = true, autoEnableNewAppsTemplateId = "")
            )
          },
          modifier = Modifier.weight(1f),
          primary = true,
      )
    }
  }
  CenteredDialog(
      title = "没有可选模板",
      summary = "模板库为空，新安装应用会默认只开启重定向。可以先在配置模板中添加模板，再回来选择。",
      show = showAutoTemplateEmptyInfo,
      onDismiss = { showAutoTemplateEmptyInfo = false },
  ) {
    GlassTextButton(
        "知道了",
        { showAutoTemplateEmptyInfo = false },
        modifier = Modifier.fillMaxWidth(),
        primary = true,
    )
  }
  editingTemplate?.let { template ->
    TemplateEditorDialog(
        template = template,
        userId = state.selectedUser,
        onListDirectories = onListDirectories,
        onDismiss = { editingTemplate = null },
        onSave = {
          onSaveTemplate(it)
          editingTemplate = null
        },
    )
  }
  deleteTemplate?.let { template ->
    val templateInUse = template.id == autoTemplateInUseId
    CenteredDialog(
        title = if (templateInUse) "不能删除模板" else "删除模板",
        summary =
            if (templateInUse) {
              "该模板正用于新应用自动配置，不能删除。"
            } else {
              "确认删除模板“${template.name}”？"
            },
        show = true,
        onDismiss = { deleteTemplate = null },
    ) {
      if (templateInUse) {
        GlassTextButton(
            "知道了",
            { deleteTemplate = null },
            modifier = Modifier.fillMaxWidth(),
            primary = true,
        )
      } else {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
          GlassTextButton("取消", { deleteTemplate = null }, modifier = Modifier.weight(1f))
          GlassTextButton(
              "删除",
              {
                shatteringTemplateId = template.id
                deleteTemplate = null
              },
              modifier = Modifier.weight(1f),
              danger = true,
          )
        }
      }
    }
  }
}
