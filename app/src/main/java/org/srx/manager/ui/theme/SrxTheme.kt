package org.srx.manager.ui.theme

import android.app.Activity
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.core.view.WindowInsetsControllerCompat
import org.srx.manager.data.UiColorSpec
import org.srx.manager.data.UiColorStyle
import org.srx.manager.data.UiThemeMode
import top.yukonga.miuix.kmp.theme.ColorSchemeMode
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.ThemeColorSpec
import top.yukonga.miuix.kmp.theme.ThemeController
import top.yukonga.miuix.kmp.theme.ThemePaletteStyle

private val LocalSrxDarkTheme = staticCompositionLocalOf { false }
private val LocalSrxLiquidGlass = staticCompositionLocalOf { true }
private val LocalSrxBlurEffect = staticCompositionLocalOf { true }

@Composable
fun SrxTheme(
    dynamicColor: Boolean = false,
    accentColor: Int = 0,
    colorStyle: UiColorStyle = UiColorStyle.TonalSpot,
    colorSpec: UiColorSpec = UiColorSpec.Spec2025,
    themeMode: UiThemeMode = UiThemeMode.System,
    liquidGlass: Boolean = true,
    blurEffect: Boolean = true,
    content: @Composable () -> Unit,
) {
    val context = LocalContext.current
    val systemDark = isSystemInDarkTheme()
    val dark = when (themeMode) {
        UiThemeMode.Light -> false
        UiThemeMode.Dark -> true
        UiThemeMode.System -> systemDark
    }
    val colorSchemeMode = when (themeMode) {
        UiThemeMode.Light -> if (dynamicColor) ColorSchemeMode.MonetLight else ColorSchemeMode.Light
        UiThemeMode.Dark -> if (dynamicColor) ColorSchemeMode.MonetDark else ColorSchemeMode.Dark
        UiThemeMode.System -> if (dynamicColor) ColorSchemeMode.MonetSystem else ColorSchemeMode.System
    }
    val keyColor = when {
        dynamicColor && accentColor == 0 -> null
        dynamicColor && accentColor != 0 -> Color(accentColor)
        else -> Color(0xFF8EA8F8)
    }
    val paletteStyle = when (colorStyle) {
        UiColorStyle.TonalSpot -> ThemePaletteStyle.TonalSpot
        UiColorStyle.Neutral -> ThemePaletteStyle.Neutral
        UiColorStyle.Vibrant -> ThemePaletteStyle.Vibrant
        UiColorStyle.Expressive -> ThemePaletteStyle.Expressive
        UiColorStyle.Rainbow -> ThemePaletteStyle.Rainbow
        UiColorStyle.FruitSalad -> ThemePaletteStyle.FruitSalad
        UiColorStyle.Monochrome -> ThemePaletteStyle.Monochrome
        UiColorStyle.Fidelity -> ThemePaletteStyle.Fidelity
        UiColorStyle.Content -> ThemePaletteStyle.Content
    }
    val themeColorSpec = when (colorSpec) {
        UiColorSpec.Spec2021 -> ThemeColorSpec.Spec2021
        UiColorSpec.Spec2025 -> ThemeColorSpec.Spec2025
    }
    val controller = remember(colorSchemeMode, keyColor, dark, paletteStyle, themeColorSpec) {
        ThemeController(
            colorSchemeMode = colorSchemeMode,
            keyColor = keyColor,
            isDark = dark,
            paletteStyle = paletteStyle,
            colorSpec = themeColorSpec,
        )
    }
    MiuixTheme(controller = controller) {
        LaunchedEffect(dark) {
            val window = (context as? Activity)?.window ?: return@LaunchedEffect
            WindowInsetsControllerCompat(window, window.decorView).apply {
                isAppearanceLightStatusBars = !dark
                isAppearanceLightNavigationBars = !dark
            }
        }
        CompositionLocalProvider(
            LocalSrxDarkTheme provides dark,
            LocalSrxLiquidGlass provides liquidGlass,
            LocalSrxBlurEffect provides blurEffect,
        ) {
            content()
        }
    }
}

@Composable
@ReadOnlyComposable
fun isSrxDarkTheme(): Boolean = LocalSrxDarkTheme.current

@Composable
@ReadOnlyComposable
fun isSrxLiquidGlassEnabled(): Boolean = LocalSrxLiquidGlass.current

@Composable
@ReadOnlyComposable
fun isSrxBlurEffectEnabled(): Boolean = LocalSrxBlurEffect.current
