package org.srx.manager.ui.screen

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.KeyboardArrowDown
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.dropShadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.shadow.Shadow
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import org.srx.manager.CenteredDialog
import org.srx.manager.data.ConfigTemplate
import org.srx.manager.glassSurfaceColor
import org.srx.manager.ui.theme.isSrxDarkTheme
import org.srx.manager.ui.theme.isSrxLiquidGlassEnabled
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.File
import top.yukonga.miuix.kmp.icon.extended.Ok
import top.yukonga.miuix.kmp.icon.extended.Tune
import top.yukonga.miuix.kmp.theme.MiuixTheme

@Composable
internal fun TemplatePickerDialog(
    show: Boolean,
    templates: List<ConfigTemplate>,
    title: String,
    emptyText: String,
    onDismiss: () -> Unit,
    onPick: (ConfigTemplate) -> Unit,
) {
    CenteredDialog(
        title = title,
        show = show,
        onDismiss = onDismiss,
    ) {
        if (templates.isEmpty()) {
            Text(
                emptyText,
                modifier = Modifier.fillMaxWidth(),
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                fontSize = 13.sp,
                lineHeight = 18.sp,
                textAlign = TextAlign.Center,
            )
        } else {
            LazyColumn(
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 420.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                items(templates, key = { it.id }) { template ->
                    TemplatePickRow(template, onClick = { onPick(template) })
                }
            }
        }
    }
}

@Composable
internal fun AutoTemplatePickerDialog(
    show: Boolean,
    templates: List<ConfigTemplate>,
    currentTemplateId: String,
    onDismiss: () -> Unit,
    onPick: (String) -> Unit,
) {
    val currentTemplateExists = remember(templates, currentTemplateId) {
        currentTemplateId.isNotBlank() && templates.any { it.id == currentTemplateId }
    }
    val listMaxHeight = (LocalConfiguration.current.screenHeightDp.dp * 0.48f).coerceIn(220.dp, 340.dp)
    CenteredDialog(
        title = "新应用默认配置",
        summary = "选择一个模板后，新安装应用会自动使用该模板生成配置。",
        show = show,
        onDismiss = onDismiss,
    ) {
        LazyColumn(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(max = listMaxHeight),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            item {
                AutoTemplateDefaultRow(
                    selected = currentTemplateId.isBlank() || !currentTemplateExists,
                    onClick = { onPick("") },
                )
            }
            items(templates, key = { it.id }) { template ->
                TemplatePickRow(
                    template = template,
                    selected = currentTemplateExists && template.id == currentTemplateId,
                    onClick = { onPick(template.id) },
                )
            }
        }
    }
}

@Composable
internal fun AutoRedirectTemplateStatusRow(
    template: ConfigTemplate?,
    templateId: String,
    fallbackNoticeId: String,
    onClick: () -> Unit,
) {
    val missingTemplate = (templateId.isNotBlank() && template == null) || fallbackNoticeId.isNotBlank()
    val accentColor = if (missingTemplate) MiuixTheme.colorScheme.error else MiuixTheme.colorScheme.primary
    val summary = when {
        template != null -> "使用模板：${template.name}"
        missingTemplate -> "模板已失效，已回退为仅开启重定向"
        else -> "仅开启重定向，无其它规则配置"
    }
    Column(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .clickable(onClick = onClick)
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Box(
                modifier = Modifier
                    .size(34.dp)
                    .clip(CircleShape)
                    .background(accentColor.copy(alpha = if (isSrxLiquidGlassEnabled()) 0.16f else 0.1f), CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Icon(MiuixIcons.Tune, contentDescription = null, tint = accentColor, modifier = Modifier.size(17.dp))
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(
                    "自动配置模板",
                    fontSize = 14.sp,
                    lineHeight = 18.sp,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    summary,
                    color = if (missingTemplate) accentColor else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    fontSize = 12.sp,
                    lineHeight = 16.sp,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            AutoTemplateChevron(
                tint = if (missingTemplate) accentColor else MiuixTheme.colorScheme.onSurface,
            )
        }
        ConfigDivider()
    }
}

@Composable
internal fun SettingSelectRow(
    title: String,
    summary: String,
    value: String,
    onClick: () -> Unit,
    showDivider: Boolean = true,
    leading: @Composable (() -> Unit)? = null,
) {
    Column(Modifier.fillMaxWidth()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(min = 64.dp)
                .clickable(onClick = onClick)
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            leading?.invoke()
            Column(
                modifier = Modifier.weight(1f),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                Text(
                    text = title,
                    fontSize = 16.sp,
                    lineHeight = 20.sp,
                    fontWeight = FontWeight.Bold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = summary,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    fontSize = 13.sp,
                    lineHeight = 18.sp,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text(
                text = value,
                color = MiuixTheme.colorScheme.primary,
                fontSize = 13.sp,
                lineHeight = 17.sp,
                fontWeight = FontWeight.Bold,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                textAlign = TextAlign.End,
                modifier = Modifier.widthIn(max = 108.dp),
            )
            AutoTemplateChevron(tint = MiuixTheme.colorScheme.onSurface)
        }
        if (showDivider) {
            ConfigDivider()
        }
    }
}

@Composable
internal fun <T> SettingOptionDialog(
    show: Boolean,
    title: String,
    options: List<Pair<T, String>>,
    selected: T,
    onDismiss: () -> Unit,
    leading: @Composable ((T, Boolean) -> Unit)? = null,
    onSelect: (T) -> Unit,
) {
    val listMaxHeight = (LocalConfiguration.current.screenHeightDp.dp * 0.48f).coerceIn(220.dp, 360.dp)
    CenteredDialog(
        title = title,
        show = show,
        onDismiss = onDismiss,
    ) {
        LazyColumn(
            modifier = Modifier
                .fillMaxWidth()
                .heightIn(max = listMaxHeight),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(options) { (value, label) ->
                SettingOptionRow(
                    label = label,
                    selected = value == selected,
                    leading = leading?.let { leadingContent ->
                        { leadingContent(value, value == selected) }
                    },
                    onClick = { onSelect(value) },
                )
            }
        }
    }
}

@Composable
private fun TemplatePickRow(
    template: ConfigTemplate,
    onClick: () -> Unit,
    selected: Boolean = false,
) {
    SelectableDialogRow(
        selected = selected,
        title = template.name,
        subtitle = "${template.config.users.size} 个用户配置",
        onClick = onClick,
        leading = {
            SelectableDialogIcon(
                icon = MiuixIcons.File,
                selected = selected,
                shape = RoundedCornerShape(15.dp),
            )
        },
    )
}

@Composable
private fun AutoTemplateDefaultRow(selected: Boolean, onClick: () -> Unit) {
    SelectableDialogRow(
        selected = selected,
        title = "仅开启重定向",
        subtitle = "不附加允许路径、沙盒路径或映射规则",
        onClick = onClick,
        leading = {
            SelectableDialogIcon(
                icon = MiuixIcons.Ok,
                selected = selected,
                shape = CircleShape,
            )
        },
    )
}

@Composable
private fun SettingOptionRow(
    label: String,
    selected: Boolean,
    leading: @Composable (() -> Unit)? = null,
    onClick: () -> Unit,
) {
    SelectableDialogRow(
        selected = selected,
        title = label,
        onClick = onClick,
        leading = leading,
    )
}

@Composable
private fun SelectableDialogRow(
    selected: Boolean,
    title: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    leading: @Composable (() -> Unit)? = null,
) {
    val primary = MiuixTheme.colorScheme.primary
    val dark = isSrxDarkTheme()
    val liquid = isSrxLiquidGlassEnabled()
    val shape = RoundedCornerShape(16.dp)
    val interactionSource = remember { MutableInteractionSource() }
    val pressed by interactionSource.collectIsPressedAsState()
    val scale by animateFloatAsState(
        targetValue = if (pressed) 0.972f else 1f,
        animationSpec = spring(dampingRatio = 0.72f, stiffness = 620f),
        label = "dialog-option-press",
    )
    val container = if (selected) {
        primary.copy(alpha = if (dark) 0.16f else 0.09f)
    } else {
        glassSurfaceColor(if (liquid) 0.74f else 0.9f)
    }
    val borderColor = if (selected) {
        primary.copy(alpha = if (dark) 0.42f else 0.3f)
    } else {
        MiuixTheme.colorScheme.onSurface.copy(alpha = if (dark) 0.085f else 0.06f)
    }
    Row(
        modifier = modifier
            .fillMaxWidth()
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }
            .then(
                if (selected) {
                    Modifier.dropShadow(
                        shape,
                        Shadow(
                            radius = 14.dp,
                            color = primary,
                            alpha = if (dark) 0.12f else 0.08f,
                        ),
                    )
                } else {
                    Modifier
                },
            )
            .clip(shape)
            .background(container, shape)
            .border(1.dp, borderColor, shape)
            .clickable(
                interactionSource = interactionSource,
                indication = null,
                onClick = onClick,
            )
            .padding(horizontal = 12.dp, vertical = if (subtitle == null) 11.dp else 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(11.dp),
    ) {
        leading?.invoke()
        Column(
            modifier = Modifier.weight(1f),
            verticalArrangement = Arrangement.spacedBy(if (subtitle == null) 0.dp else 2.dp),
        ) {
            Text(
                title,
                color = if (selected) primary else MiuixTheme.colorScheme.onSurface,
                fontWeight = if (selected) FontWeight.Black else FontWeight.Bold,
                fontSize = 14.sp,
                lineHeight = 18.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (subtitle != null) {
                Text(
                    subtitle,
                    color = if (selected) {
                        primary.copy(alpha = if (dark) 0.82f else 0.78f)
                    } else {
                        MiuixTheme.colorScheme.onSurfaceVariantSummary
                    },
                    fontSize = 11.sp,
                    lineHeight = 15.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        SelectionCheckMark(selected)
    }
}

@Composable
private fun SelectableDialogIcon(
    icon: ImageVector,
    selected: Boolean,
    shape: Shape,
    tint: Color = MiuixTheme.colorScheme.primary,
) {
    Box(
        modifier = Modifier
            .size(34.dp)
            .then(
                if (selected) {
                    Modifier.dropShadow(
                        shape,
                        Shadow(radius = 10.dp, color = tint, alpha = if (isSrxDarkTheme()) 0.14f else 0.1f),
                    )
                } else {
                    Modifier
                },
            )
            .clip(shape)
            .background(tint.copy(alpha = if (selected) 0.2f else if (isSrxLiquidGlassEnabled()) 0.14f else 0.1f), shape),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = tint, modifier = Modifier.size(18.dp))
    }
}

@Composable
private fun SelectionCheckMark(selected: Boolean) {
    Box(
        modifier = Modifier.size(24.dp),
        contentAlignment = Alignment.Center,
    ) {
        if (selected) {
            Box(
                modifier = Modifier
                    .size(23.dp)
                    .dropShadow(
                        CircleShape,
                        Shadow(
                            radius = 9.dp,
                            color = MiuixTheme.colorScheme.primary,
                            alpha = if (isSrxDarkTheme()) 0.2f else 0.16f,
                        ),
                    )
                    .clip(CircleShape)
                    .background(MiuixTheme.colorScheme.primary, CircleShape),
                contentAlignment = Alignment.Center,
            ) {
                Icon(MiuixIcons.Ok, contentDescription = null, tint = Color.White, modifier = Modifier.size(14.dp))
            }
        }
    }
}

@Composable
private fun AutoTemplateChevron(tint: Color) {
    Box(
        modifier = Modifier
            .size(24.dp)
            .clip(CircleShape)
            .background(if (isSrxLiquidGlassEnabled()) glassSurfaceColor(0.5f) else Color.Transparent, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = Icons.Rounded.KeyboardArrowDown,
            contentDescription = null,
            tint = tint,
            modifier = Modifier.size(18.dp),
        )
    }
}
