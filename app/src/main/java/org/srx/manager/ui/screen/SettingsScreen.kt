package org.srx.manager.ui.screen

import android.os.Build

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
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
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.PageHeader
import org.srx.manager.RoundIconAction
import org.srx.manager.SectionTitle
import org.srx.manager.data.AppConfig
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.GlobalConfig
import org.srx.manager.data.UiColorSpec
import org.srx.manager.data.UiColorStyle
import org.srx.manager.data.UiPreferences
import org.srx.manager.data.UiThemeMode
import org.srx.manager.glassSurfaceColor
import org.srx.manager.srxSuccessColor
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.AppUiState
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Download
import top.yukonga.miuix.kmp.icon.extended.File
import top.yukonga.miuix.kmp.icon.extended.Import
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import kotlin.math.roundToInt

@Composable
private fun BackupActionButton(
    text: String,
    icon: ImageVector,
    tint: Color,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier = modifier
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
        Text(text, color = tint, fontSize = 13.sp, fontWeight = FontWeight.Black, maxLines = 1)
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
    onFloating: (Boolean) -> Unit,
    onLiquid: (Boolean) -> Unit,
    onBlurEffect: (Boolean) -> Unit,
    onDynamicColor: (Boolean) -> Unit,
    onAccentColor: (Int) -> Unit,
    onColorStyle: (UiColorStyle) -> Unit,
    onColorSpec: (UiColorSpec) -> Unit,
    onThemeMode: (UiThemeMode) -> Unit,
    onPredictiveBack: (Boolean) -> Unit,
    onPageScale: (Float) -> Unit,
    onBackupExport: () -> Unit,
    onBackupImport: () -> Unit,
    onDiagnosticExport: () -> Unit,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
) {
    val global = state.dashboard.globalConfig
    var deleteTemplate by remember { mutableStateOf<ConfigTemplate?>(null) }
    var editingTemplate by remember { mutableStateOf<ConfigTemplate?>(null) }
    var showAutoTemplatePicker by remember { mutableStateOf(false) }
    var showAutoTemplateEmptyEnable by remember { mutableStateOf(false) }
    var showAutoTemplateEmptyInfo by remember { mutableStateOf(false) }
    var showAccentColorPicker by remember { mutableStateOf(false) }
    var showColorStylePicker by remember { mutableStateOf(false) }
    var showColorSpecPicker by remember { mutableStateOf(false) }
    var showPageScaleEditor by remember { mutableStateOf(false) }
    val autoTemplateInUseId = global.autoEnableNewAppsTemplateId
    val autoTemplate = remember(state.templates, global.autoEnableNewAppsTemplateId) {
        state.templates.firstOrNull { it.id == global.autoEnableNewAppsTemplateId }
    }
    fun updateAutoEnable(checked: Boolean) {
        if (!checked) {
            onGlobal(global.copy(autoEnableRedirectForNewApps = false))
            return
        }
        when {
            global.autoEnableNewAppsTemplateId.isNotBlank() -> onGlobal(global.copy(autoEnableRedirectForNewApps = true))
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
    fun newTemplateDraft() = ConfigTemplate(
        id = "template-${System.currentTimeMillis()}",
        name = "新配置模板",
        config = AppConfig(users = mapOf(state.selectedUser to DisabledDefaultProfile)),
    )
    LazyColumn(
        modifier = Modifier
            .fillMaxSize()
            .overScrollVertical(),
        contentPadding = PaddingValues(
            top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 18.dp,
            bottom = bottomPadding + 28.dp,
            start = 16.dp,
            end = 16.dp,
        ),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        item {
            PageHeader(
                title = "设置",
                actions = {
                    RoundIconAction(
                        icon = MiuixIcons.File,
                        contentDescription = "导出日志包",
                        onClick = onDiagnosticExport,
                        size = 36.dp,
                        iconSize = 17.dp,
                    )
                },
            )
        }
        item {
            SectionTitle("模块设置")
            GlassCard(alpha = 0.58f) {
                CompactSwitchRow(
                    title = "文件监视",
                    summary = "记录已启用应用和系统代写进程的文件创建操作",
                    checked = global.fileMonitorEnabled,
                    onCheckedChange = { onGlobal(global.copy(fileMonitorEnabled = it)) },
                )
                CompactSwitchRow(
                    title = "Fuse Fixer",
                    summary = "SRX 内置 Fuse Fixer 兼容保护，处理特殊 Unicode 字符",
                    checked = global.fuseFixEnabled,
                    onCheckedChange = { onGlobal(global.copy(fuseFixEnabled = it)) },
                )
                CompactSwitchRow(
                    title = "详细日志",
                    summary = "开启后立即记录 Rust、Java、Stats 和诊断采集日志",
                    checked = global.verboseLoggingEnabled,
                    onCheckedChange = { onGlobal(global.copy(verboseLoggingEnabled = it)) },
                )
                CompactSwitchRow(
                    title = "新应用自动重定向",
                    summary = "收到系统新安装事件后，自动写入默认配置",
                    checked = global.autoEnableRedirectForNewApps,
                    onCheckedChange = ::updateAutoEnable,
                    showDivider = !global.autoEnableRedirectForNewApps,
                )
                if (global.autoEnableRedirectForNewApps) {
                    AutoRedirectTemplateStatusRow(
                        template = autoTemplate,
                        templateId = global.autoEnableNewAppsTemplateId,
                        fallbackNoticeId = state.autoTemplateFallbackNoticeId,
                        onClick = ::openAutoTemplatePicker,
                    )
                }
                CompactSwitchRow(
                    title = "配置操作即时保存",
                    summary = "开启后应用配置页的每次操作都会立即写入模块配置",
                    checked = global.appConfigAutoSave,
                    onCheckedChange = { onGlobal(global.copy(appConfigAutoSave = it)) },
                    showDivider = false,
                )
            }
        }
        item {
            SectionTitle("实验区")
            GlassCard(alpha = 0.58f) {
                CompactSwitchRow(
                    title = "Fuse daemon",
                    summary = "仅在普通应用的通配规则前缀启用 scoped FUSE，精确处理 !、*、?；普通路径继续使用 mount namespace。可提升复杂规则准确性，但通配前缀内的高频读写会多一层用户态转发。",
                    checked = global.fuseDaemonRedirectEnabled,
                    onCheckedChange = { onGlobal(global.copy(fuseDaemonRedirectEnabled = it)) },
                    showDivider = false,
                )
            }
        }
        item {
            SectionTitle("主题")
            GlassCard(
                insideMargin = PaddingValues(top = 10.dp, bottom = 8.dp),
                alpha = 0.58f,
            ) {
                ThemeModeSelector(
                    mode = prefs.themeMode,
                    onMode = onThemeMode,
                    modifier = Modifier
                        .fillMaxWidth()
                        .padding(horizontal = 8.dp),
                )
                Spacer(Modifier.height(12.dp))
                CompactSwitchRow(
                    title = "动态取色",
                    summary = "开启后跟随系统壁纸色，关闭后使用固定主题色",
                    checked = prefs.dynamicColor,
                    onCheckedChange = onDynamicColor,
                    showDivider = prefs.dynamicColor,
                )
                if (prefs.dynamicColor) {
                    SettingSelectRow(
                        title = "强调色",
                        summary = "默认使用系统壁纸色，也可指定应用主题色",
                        value = accentColorLabel(prefs.accentColor),
                        onClick = { showAccentColorPicker = true },
                        showDivider = prefs.accentColor != 0,
                        leading = { AccentColorPenIcon(prefs.accentColor) },
                    )
                }
                if (prefs.dynamicColor && prefs.accentColor != 0) {
                    SettingSelectRow(
                        title = "色彩风格",
                        summary = "控制强调色生成主题色板时的倾向",
                        value = colorStyleLabel(prefs.colorStyle),
                        onClick = { showColorStylePicker = true },
                    )
                    SettingSelectRow(
                        title = "色彩标准",
                        summary = "选择 MIUIX 主题色生成标准",
                        value = colorSpecLabel(prefs.colorSpec),
                        onClick = { showColorSpecPicker = true },
                        showDivider = false,
                    )
                }
            }
        }
        item {
            SectionTitle("视觉效果")
            GlassCard(alpha = 0.58f) {
                val showPredictiveBack = Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE
                SettingSelectRow(
                    title = "界面缩放",
                    summary = "调整页面密度，范围 80% - 110%",
                    value = pageScalePercentLabel(prefs.pageScale),
                    onClick = { showPageScaleEditor = true },
                )
                CompactSwitchRow(
                    title = "悬浮底栏",
                    summary = "启用 MIUIX 主题风格的悬浮底部导航",
                    checked = prefs.floatingBottomBar,
                    onCheckedChange = onFloating,
                )
                CompactSwitchRow(
                    title = "液态玻璃",
                    summary = "控制卡片、按钮、弹窗、通知和底栏的玻璃材质",
                    checked = prefs.liquidGlass,
                    onCheckedChange = onLiquid,
                )
                CompactSwitchRow(
                    title = "模糊",
                    summary = "启用顶部、底部栏和液态玻璃背景模糊，模糊强度使用固定值",
                    checked = prefs.blurEffect,
                    onCheckedChange = onBlurEffect,
                    showDivider = showPredictiveBack,
                )
                if (showPredictiveBack) {
                    CompactSwitchRow(
                        title = "预测性返回手势",
                        summary = "启用系统预测性返回手势支持，二级页面返回时显示上级页面预览",
                        checked = prefs.predictiveBack,
                        onCheckedChange = onPredictiveBack,
                        showDivider = false,
                    )
                }
            }
        }
        item {
            SectionTitle("配置模板")
            GlassCard(insideMargin = PaddingValues(0.dp), alpha = 0.58f) {
                ConfigGroupHeader("模板库", addLabel = "添加配置模板", onAdd = { editingTemplate = newTemplateDraft() })
                if (state.templates.isEmpty()) {
                    EmptyConfigHint("还没有配置模板，可从应用配置页保存当前配置为模板")
                } else {
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 360.dp),
                    ) {
                        itemsIndexed(state.templates, key = { _, template -> template.id }) { index, template ->
                            TemplateManageRow(
                                template = template,
                                onEdit = { editingTemplate = template },
                                onDelete = { deleteTemplate = template },
                            )
                            if (index != state.templates.lastIndex) ConfigDivider()
                        }
                    }
                }
            }
        }
        item {
            SectionTitle("权限")
            GlassCard(insideMargin = PaddingValues(16.dp), alpha = 0.58f) {
                Text(
                    text = if (state.rootGranted) "root 权限已授予" else "未获得 root 权限",
                    color = if (state.rootGranted) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.error,
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
        item {
            SectionTitle("备份还原")
            GlassCard(insideMargin = PaddingValues(12.dp), alpha = 0.58f) {
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    BackupActionButton(
                        text = "备份",
                        icon = MiuixIcons.Download,
                        tint = MiuixTheme.colorScheme.primary,
                        onClick = onBackupExport,
                        modifier = Modifier.weight(1f),
                    )
                    BackupActionButton(
                        text = "还原",
                        icon = MiuixIcons.Import,
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
                    "/data/adb/modules/storage.redirect.x/config/",
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    fontSize = 11.sp,
                    lineHeight = 14.sp,
                )
            }
        }
    }
    SettingOptionDialog(
        show = showAccentColorPicker,
        title = "强调色",
        options = AccentColorOptions,
        selected = prefs.accentColor,
        onDismiss = { showAccentColorPicker = false },
        leading = { value, selected -> AccentColorPenIcon(value, selected) },
        onSelect = {
            onAccentColor(it)
            showAccentColorPicker = false
        },
    )
    SettingOptionDialog(
        show = showColorStylePicker,
        title = "色彩风格",
        options = ColorStyleOptions,
        selected = prefs.colorStyle,
        onDismiss = { showColorStylePicker = false },
        onSelect = {
            onColorStyle(it)
            showColorStylePicker = false
        },
    )
    SettingOptionDialog(
        show = showColorSpecPicker,
        title = "色彩标准",
        options = ColorSpecOptions,
        selected = prefs.colorSpec,
        onDismiss = { showColorSpecPicker = false },
        onSelect = {
            onColorSpec(it)
            showColorSpecPicker = false
        },
    )
    PageScaleDialog(
        show = showPageScaleEditor,
        scale = prefs.pageScale,
        onDismiss = { showPageScaleEditor = false },
        onSave = {
            onPageScale(it)
            showPageScaleEditor = false
        },
    )
    AutoTemplatePickerDialog(
        show = showAutoTemplatePicker,
        templates = state.templates,
        currentTemplateId = global.autoEnableNewAppsTemplateId,
        onDismiss = { showAutoTemplatePicker = false },
        onPick = { templateId ->
            showAutoTemplatePicker = false
            onGlobal(global.copy(autoEnableRedirectForNewApps = true, autoEnableNewAppsTemplateId = templateId))
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
            GlassTextButton("继续开启", {
                showAutoTemplateEmptyEnable = false
                onGlobal(global.copy(autoEnableRedirectForNewApps = true, autoEnableNewAppsTemplateId = ""))
            }, modifier = Modifier.weight(1f), primary = true)
        }
    }
    CenteredDialog(
        title = "没有可选模板",
        summary = "模板库为空，新安装应用会默认只开启重定向。可以先在配置模板中添加模板，再回来选择。",
        show = showAutoTemplateEmptyInfo,
        onDismiss = { showAutoTemplateEmptyInfo = false },
    ) {
        GlassTextButton("知道了", { showAutoTemplateEmptyInfo = false }, modifier = Modifier.fillMaxWidth(), primary = true)
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
            summary = if (templateInUse) {
                "该模板正用于新应用自动配置，不能删除。"
            } else {
                "确认删除模板“${template.name}”？"
            },
            show = true,
            onDismiss = { deleteTemplate = null },
        ) {
            if (templateInUse) {
                GlassTextButton("知道了", { deleteTemplate = null }, modifier = Modifier.fillMaxWidth(), primary = true)
            } else {
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    GlassTextButton("取消", { deleteTemplate = null }, modifier = Modifier.weight(1f))
                    GlassTextButton("删除", {
                        onDeleteTemplate(template.id)
                        deleteTemplate = null
                    }, modifier = Modifier.weight(1f), danger = true)
                }
            }
        }
    }
}

private const val PageScaleMinPercent = 80
private const val PageScaleMaxPercent = 110

private fun pageScalePercent(scale: Float): Int =
    (scale.coerceIn(PageScaleMinPercent / 100f, PageScaleMaxPercent / 100f) * 100).roundToInt()

private fun pageScalePercentLabel(scale: Float): String = "${pageScalePercent(scale)}%"

@Composable
private fun PageScaleDialog(
    show: Boolean,
    scale: Float,
    onDismiss: () -> Unit,
    onSave: (Float) -> Unit,
) {
    var input by remember(show, scale) { mutableStateOf(pageScalePercent(scale).toString()) }
    val parsed = input.toIntOrNull()
    val clamped = parsed?.coerceIn(PageScaleMinPercent, PageScaleMaxPercent)
    val isInvalid = input.isNotBlank() && parsed == null
    CenteredDialog(
        title = "界面缩放",
        summary = "降低比例可以缓解 DPI 或字体放大后的文本截断；范围 80% - 110%。",
        show = show,
        onDismiss = onDismiss,
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            TextField(
                value = input,
                onValueChange = { value ->
                    input = value.filter(Char::isDigit).take(3)
                },
                label = "缩放百分比",
                colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
                useLabelAsPlaceholder = true,
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Text(
                text = when {
                    isInvalid || parsed == null -> "请输入 80 - 110"
                    clamped != parsed -> "将保存为 ${clamped}%"
                    else -> "当前为 ${clamped}%"
                },
                color = if (isInvalid || parsed == null) {
                    MiuixTheme.colorScheme.error
                } else {
                    MiuixTheme.colorScheme.onSurfaceVariantSummary
                },
                fontSize = 12.sp,
                lineHeight = 16.sp,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
                GlassTextButton(
                    "保存",
                    {
                        input.toIntOrNull()
                            ?.coerceIn(PageScaleMinPercent, PageScaleMaxPercent)
                            ?.let { onSave(it / 100f) }
                    },
                    modifier = Modifier.weight(1f),
                    primary = true,
                )
            }
        }
    }
}
