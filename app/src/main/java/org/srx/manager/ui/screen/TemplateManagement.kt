package org.srx.manager.ui.screen

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import org.srx.manager.GlassCard
import org.srx.manager.GlassTextButton
import org.srx.manager.RoundIconAction
import org.srx.manager.data.AppConfig
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.data.SrxConfigNormalizer
import org.srx.manager.data.UserProfile
import org.srx.manager.subtleFieldLabelColor
import org.srx.manager.ui.theme.isSrxDarkTheme
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextField
import top.yukonga.miuix.kmp.basic.TextFieldDefaults
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.overScrollVertical

@Composable
internal fun TemplateManageRow(
    template: ConfigTemplate,
    onEdit: () -> Unit,
    onDelete: () -> Unit,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .clickable(onClick = onEdit)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(
            MiuixIcons.Tune,
            contentDescription = null,
            tint = MiuixTheme.colorScheme.primary,
            modifier = Modifier.size(19.dp),
        )
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                template.name,
                fontWeight = FontWeight.Bold,
                fontSize = 14.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                "${template.config.users.size} 个用户配置",
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 11.sp,
            )
        }
        RoundIconAction(
            MiuixIcons.Delete,
            "删除模板",
            onDelete,
            danger = true,
            size = 34.dp,
            iconSize = 16.dp,
        )
    }
}

@Composable
internal fun TemplateEditorDialog(
    template: ConfigTemplate,
    userId: String,
    onListDirectories: (String, String, (List<String>) -> Unit) -> Unit,
    onDismiss: () -> Unit,
    onSave: (ConfigTemplate) -> Unit,
) {
    var name by remember(template.id) { mutableStateOf(template.name) }
    var profile by remember(template.id, userId) {
        mutableStateOf(template.config.users[userId] ?: DisabledDefaultProfile)
    }
    var readOnlyEditorEnabled by remember(template.id, userId) {
        mutableStateOf(profile.readOnlyPaths.isNotEmpty())
    }
    LaunchedEffect(profile.readOnlyPaths) {
        if (profile.readOnlyPaths.isNotEmpty()) {
            readOnlyEditorEnabled = true
        }
    }
    fun updateProfile(transform: (UserProfile) -> UserProfile) {
        profile = transform(profile)
    }

    Dialog(
        onDismissRequest = onDismiss,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Box(
            modifier = Modifier
                .fillMaxSize()
                .background(
                    if (isSrxDarkTheme()) {
                        Color.Black.copy(alpha = 0.24f)
                    } else {
                        Color.White.copy(alpha = 0.08f)
                    },
                ),
            contentAlignment = Alignment.Center,
        ) {
            GlassCard(
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(horizontal = 16.dp)
                    .heightIn(max = 720.dp),
                cornerRadius = 30.dp,
                insideMargin = PaddingValues(0.dp),
                alpha = 0.82f,
                shadowAlpha = 0.2f,
            ) {
                Column(Modifier.fillMaxWidth()) {
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(horizontal = 18.dp, vertical = 16.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text(
                            "编辑模板",
                            modifier = Modifier.weight(1f),
                            fontSize = 18.sp,
                            fontWeight = FontWeight.Black,
                        )
                        RoundIconAction(
                            Icons.Rounded.Close,
                            "关闭",
                            onDismiss,
                            size = 36.dp,
                            iconSize = 18.dp,
                        )
                    }
                    LazyColumn(
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 560.dp)
                            .overScrollVertical(),
                        contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
                        verticalArrangement = Arrangement.spacedBy(14.dp),
                    ) {
                        item {
                            TextField(
                                value = name,
                                onValueChange = { name = it.take(48) },
                                label = "模板名称",
                                colors = TextFieldDefaults.textFieldColors(labelColor = subtleFieldLabelColor()),
                                useLabelAsPlaceholder = true,
                                singleLine = true,
                                modifier = Modifier.fillMaxWidth(),
                            )
                        }
                        item {
                            GlassCard(alpha = 0.52f, shadowAlpha = 0.14f) {
                                CompactSwitchRow(
                                    title = "启用重定向",
                                    summary = "应用该模板后是否启用存储重定向X",
                                    checked = profile.enabled,
                                    onCheckedChange = { checked -> updateProfile { it.copy(enabled = checked) } },
                                )
                                CompactSwitchRow(
                                    title = "仅映射模式",
                                    summary = "仅应用显式路径映射；未命中映射时保持原路径",
                                    checked = profile.mappingModeOnly,
                                    onCheckedChange = { checked ->
                                        updateProfile { it.copy(mappingModeOnly = checked) }
                                    },
                                )
                                CompactSwitchRow(
                                    title = "只读模式",
                                    summary = "禁止写入指定真实目录；默认方案会退化通配规则，FUSE daemon 可精确匹配",
                                    checked = readOnlyEditorEnabled || profile.readOnlyPaths.isNotEmpty(),
                                    onCheckedChange = { checked ->
                                        readOnlyEditorEnabled = checked
                                        if (!checked) {
                                            updateProfile { it.copy(readOnlyPaths = emptyList()) }
                                        }
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
                                userId = userId,
                                onListDirectories = onListDirectories,
                                onAdd = { value ->
                                    updateProfile {
                                        it.copy(
                                            allowedRealPaths = (it.allowedRealPaths +
                                                normalizeEditablePathInput(value, userId, true))
                                                .filter(String::isNotBlank)
                                                .distinct()
                                                .sorted(),
                                        )
                                    }
                                },
                                onUpdate = { old, value ->
                                    updateProfile {
                                        val path = normalizeEditablePathInput(value, userId, true)
                                        it.copy(
                                            allowedRealPaths = (it.allowedRealPaths - old + path)
                                                .filter(String::isNotBlank)
                                                .distinct()
                                                .sorted(),
                                        )
                                    }
                                },
                                onRemove = { value ->
                                    updateProfile { it.copy(allowedRealPaths = it.allowedRealPaths - value) }
                                },
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
                                    userId = userId,
                                    onListDirectories = onListDirectories,
                                    onAdd = { value ->
                                        updateProfile {
                                            val path = SrxConfigNormalizer.sanitizeEditablePath(
                                                normalizeEditablePathInput(value, userId, true),
                                                allowRuleSyntax = true,
                                                allowWildcards = true,
                                            )
                                            it.copy(
                                                readOnlyPaths = (it.readOnlyPaths + path)
                                                    .filter(String::isNotBlank)
                                                    .distinct()
                                                    .sorted(),
                                            )
                                        }
                                    },
                                    onUpdate = { old, value ->
                                        updateProfile {
                                            val path = SrxConfigNormalizer.sanitizeEditablePath(
                                                normalizeEditablePathInput(value, userId, true),
                                                allowRuleSyntax = true,
                                                allowWildcards = true,
                                            )
                                            it.copy(
                                                readOnlyPaths = (it.readOnlyPaths - old + path)
                                                    .filter(String::isNotBlank)
                                                    .distinct()
                                                    .sorted(),
                                            )
                                        }
                                    },
                                    onRemove = { value ->
                                        updateProfile { it.copy(readOnlyPaths = it.readOnlyPaths - value) }
                                    },
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
                                    userId = userId,
                                    onListDirectories = onListDirectories,
                                    onAdd = { value ->
                                        updateProfile {
                                            it.copy(
                                                sandboxedPaths = (it.sandboxedPaths +
                                                    normalizeEditablePathInput(value, userId))
                                                    .filter(String::isNotBlank)
                                                    .distinct()
                                                    .sorted(),
                                            )
                                        }
                                    },
                                    onUpdate = { old, value ->
                                        updateProfile {
                                            it.copy(
                                                sandboxedPaths = (it.sandboxedPaths - old +
                                                    normalizeEditablePathInput(value, userId))
                                                    .filter(String::isNotBlank)
                                                    .distinct()
                                                    .sorted(),
                                            )
                                        }
                                    },
                                    onRemove = { value ->
                                        updateProfile { it.copy(sandboxedPaths = it.sandboxedPaths - value) }
                                    },
                                )
                            }
                        }
                        item {
                            MappingEditorCard(
                                mappings = profile.pathMappings,
                                userId = userId,
                                onListDirectories = onListDirectories,
                                onAdd = { from, to ->
                                    val cleanFrom = normalizeEditablePathInput(from, userId)
                                    val cleanTo = normalizeEditablePathInput(to, userId)
                                    if (cleanFrom.isNotBlank() && cleanTo.isNotBlank() && cleanFrom != cleanTo) {
                                        updateProfile {
                                            it.copy(pathMappings = (it.pathMappings + (cleanFrom to cleanTo)).toSortedMap())
                                        }
                                    }
                                },
                                onUpdate = { old, from, to ->
                                    val cleanFrom = normalizeEditablePathInput(from, userId)
                                    val cleanTo = normalizeEditablePathInput(to, userId)
                                    if (cleanFrom.isNotBlank() && cleanTo.isNotBlank() && cleanFrom != cleanTo) {
                                        updateProfile {
                                            val mappings = it.pathMappings.toMutableMap()
                                            mappings.remove(old)
                                            mappings[cleanFrom] = cleanTo
                                            it.copy(pathMappings = mappings.toSortedMap())
                                        }
                                    }
                                },
                                onRemove = { from ->
                                    updateProfile { it.copy(pathMappings = it.pathMappings - from) }
                                },
                            )
                        }
                    }
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(16.dp),
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        GlassTextButton("取消", onDismiss, modifier = Modifier.weight(1f))
                        GlassTextButton(
                            "保存",
                            {
                                val cleanName = name.trim()
                                if (cleanName.isNotBlank()) {
                                    onSave(
                                        template.copy(
                                            name = cleanName,
                                            config = AppConfig(users = template.config.users + (userId to profile)),
                                        ),
                                    )
                                }
                            },
                            modifier = Modifier.weight(1f),
                            primary = true,
                        )
                    }
                }
            }
        }
    }
}
