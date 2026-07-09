package org.srx.manager.data

import android.content.Context
import androidx.datastore.preferences.core.booleanPreferencesKey
import androidx.datastore.preferences.core.edit
import androidx.datastore.preferences.core.floatPreferencesKey
import androidx.datastore.preferences.core.intPreferencesKey
import androidx.datastore.preferences.core.stringPreferencesKey
import androidx.datastore.preferences.preferencesDataStore
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map

private val Context.dataStore by preferencesDataStore(name = "srx_manager")
private const val SharedPrefsName = "srx_manager"
private const val PredictiveBackPrefsKey = "predictive_back"

enum class UiThemeMode {
  Light,
  Dark,
  System,
}

enum class UiColorStyle {
  TonalSpot,
  Neutral,
  Vibrant,
  Expressive,
  Rainbow,
  FruitSalad,
  Monochrome,
  Fidelity,
  Content,
}

enum class UiColorSpec {
  Spec2021,
  Spec2025,
}

data class UiPreferences(
    val floatingBottomBar: Boolean = true,
    val liquidGlass: Boolean = true,
    val blurEffect: Boolean = true,
    val dynamicColor: Boolean = false,
    val accentColor: Int = 0,
    val colorStyle: UiColorStyle = UiColorStyle.TonalSpot,
    val colorSpec: UiColorSpec = UiColorSpec.Spec2025,
    val themeMode: UiThemeMode = UiThemeMode.System,
    val predictiveBack: Boolean = false,
    val pageScale: Float = 1.0f,
    val autoCheckUpdates: Boolean = true,
    val updateChannel: UpdateChannel = UpdateChannel.Stable,
)

class PreferencesRepository(private val context: Context) {
  private companion object {
    const val PageScaleMin = 0.8f
    const val PageScaleMax = 1.1f
  }

  private val floatingBottomBarKey = booleanPreferencesKey("floating_bottom_bar")
  private val liquidGlassKey = booleanPreferencesKey("liquid_glass")
  private val blurEffectKey = booleanPreferencesKey("blur_effect")
  private val legacyLiquidGlassBlurKey = floatPreferencesKey("liquid_glass_blur")
  private val dynamicColorKey = booleanPreferencesKey("dynamic_color")
  private val accentColorKey = intPreferencesKey("accent_color")
  private val colorStyleKey = stringPreferencesKey("color_style")
  private val colorSpecKey = stringPreferencesKey("color_spec")
  private val themeModeKey = stringPreferencesKey("theme_mode")
  private val predictiveBackKey = booleanPreferencesKey("predictive_back")
  private val pageScaleKey = floatPreferencesKey("page_scale")
  private val autoCheckUpdatesKey = booleanPreferencesKey("auto_check_updates")
  private val updateChannelKey = stringPreferencesKey("update_channel")

  val uiPreferences: Flow<UiPreferences> =
      context.dataStore.data.map { prefs ->
        UiPreferences(
            floatingBottomBar = prefs[floatingBottomBarKey] ?: true,
            liquidGlass = prefs[liquidGlassKey] ?: true,
            blurEffect =
                prefs[blurEffectKey] ?: ((prefs[legacyLiquidGlassBlurKey] ?: 0.7f) > 0.01f),
            dynamicColor = prefs[dynamicColorKey] ?: false,
            accentColor = prefs[accentColorKey] ?: 0,
            colorStyle =
                prefs[colorStyleKey]?.let { runCatching { UiColorStyle.valueOf(it) }.getOrNull() }
                    ?: UiColorStyle.TonalSpot,
            colorSpec =
                prefs[colorSpecKey]?.let { runCatching { UiColorSpec.valueOf(it) }.getOrNull() }
                    ?: UiColorSpec.Spec2025,
            themeMode =
                prefs[themeModeKey]?.let { runCatching { UiThemeMode.valueOf(it) }.getOrNull() }
                    ?: UiThemeMode.System,
            predictiveBack = predictiveBackCompatPref(),
            pageScale = normalizePageScale(prefs[pageScaleKey]),
            autoCheckUpdates = prefs[autoCheckUpdatesKey] ?: true,
            updateChannel =
                prefs[updateChannelKey]?.let {
                  runCatching { UpdateChannel.valueOf(it) }.getOrNull()
                } ?: UpdateChannel.Stable,
        )
      }

  suspend fun setFloatingBottomBar(enabled: Boolean) {
    context.dataStore.edit { it[floatingBottomBarKey] = enabled }
  }

  suspend fun setLiquidGlass(enabled: Boolean) {
    context.dataStore.edit { it[liquidGlassKey] = enabled }
  }

  suspend fun setBlurEffect(enabled: Boolean) {
    context.dataStore.edit { it[blurEffectKey] = enabled }
  }

  suspend fun setDynamicColor(enabled: Boolean) {
    context.dataStore.edit { it[dynamicColorKey] = enabled }
  }

  suspend fun setAccentColor(color: Int) {
    context.dataStore.edit { it[accentColorKey] = color }
  }

  suspend fun setColorStyle(style: UiColorStyle) {
    context.dataStore.edit { it[colorStyleKey] = style.name }
  }

  suspend fun setColorSpec(spec: UiColorSpec) {
    context.dataStore.edit { it[colorSpecKey] = spec.name }
  }

  suspend fun setThemeMode(mode: UiThemeMode) {
    context.dataStore.edit { it[themeModeKey] = mode.name }
  }

  suspend fun setPredictiveBack(enabled: Boolean) {
    setPredictiveBackCompatPref(enabled)
    context.dataStore.edit { it[predictiveBackKey] = enabled }
  }

  suspend fun setPageScale(scale: Float) {
    context.dataStore.edit { it[pageScaleKey] = normalizePageScale(scale) }
  }

  suspend fun setAutoCheckUpdates(enabled: Boolean) {
    context.dataStore.edit { it[autoCheckUpdatesKey] = enabled }
  }

  suspend fun setUpdateChannel(channel: UpdateChannel) {
    context.dataStore.edit { it[updateChannelKey] = channel.name }
  }

  suspend fun readBackupUiPreferences(): BackupUiPreferences {
    val prefs = uiPreferences.first()
    return BackupUiPreferences(
        predictiveBack = prefs.predictiveBack,
        floatingBottomBar = prefs.floatingBottomBar,
        liquidGlass = prefs.liquidGlass,
        blurEffect = prefs.blurEffect,
        dynamicColor = prefs.dynamicColor,
        accentColor = prefs.accentColor,
        colorStyle = prefs.colorStyle,
        colorSpec = prefs.colorSpec,
        themeMode = prefs.themeMode,
        pageScale = prefs.pageScale,
        autoCheckUpdates = prefs.autoCheckUpdates,
        updateChannel = prefs.updateChannel,
    )
  }

  suspend fun restoreBackupUiPreferences(preferences: BackupUiPreferences) {
    preferences.predictiveBack?.let { setPredictiveBack(it) }
    context.dataStore.edit {
      preferences.floatingBottomBar?.let { value -> it[floatingBottomBarKey] = value }
      preferences.liquidGlass?.let { value -> it[liquidGlassKey] = value }
      preferences.blurEffect?.let { value -> it[blurEffectKey] = value }
      preferences.dynamicColor?.let { value -> it[dynamicColorKey] = value }
      preferences.accentColor?.let { value -> it[accentColorKey] = value }
      preferences.colorStyle?.let { value -> it[colorStyleKey] = value.name }
      preferences.colorSpec?.let { value -> it[colorSpecKey] = value.name }
      preferences.themeMode?.let { value -> it[themeModeKey] = value.name }
      preferences.pageScale?.let { value -> it[pageScaleKey] = normalizePageScale(value) }
      preferences.autoCheckUpdates?.let { value -> it[autoCheckUpdatesKey] = value }
      preferences.updateChannel?.let { value -> it[updateChannelKey] = value.name }
    }
  }

  suspend fun readPredictiveBack(): Boolean = predictiveBackCompatPref()

  fun setPredictiveBackCompatPref(enabled: Boolean) {
    context
        .getSharedPreferences(SharedPrefsName, Context.MODE_PRIVATE)
        .edit()
        .putBoolean(PredictiveBackPrefsKey, enabled)
        .commit()
  }

  private fun predictiveBackCompatPref(): Boolean =
      context
          .getSharedPreferences(SharedPrefsName, Context.MODE_PRIVATE)
          .getBoolean(PredictiveBackPrefsKey, false)

  private fun normalizePageScale(scale: Float?): Float {
    val value = scale ?: 1.0f
    return if (value.isFinite()) value.coerceIn(PageScaleMin, PageScaleMax) else 1.0f
  }
}
