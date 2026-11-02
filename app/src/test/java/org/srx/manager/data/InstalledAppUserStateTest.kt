package org.srx.manager.data

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 列表行的「已配置 / 已启用 / 缺失」必须按条目所属用户判定。
 *
 * 配置是按 `users.<id>` 存的：同一份文件里既有主用户的档案，也可能没有分身用户的档案。 早先用 `config != null` 判定，会让分身用户看到主用户的「已配置 /
 * 已启用」，也就是 用户 999 列表里冒出用户 0 的配置状态。
 */
class InstalledAppUserStateTest {
  private val configForUserZero = AppConfig(users = mapOf("0" to UserProfile(enabled = true)))

  private fun app(userId: String, installed: Boolean = true, config: AppConfig? = null) =
      InstalledApp(
          packageName = "com.tencent.mobileqq",
          label = "QQ",
          isSystem = false,
          appInfo = null,
          config = config,
          isInstalled = installed,
          userId = userId,
      )

  @Test
  fun otherUsersConfigDoesNotMarkAppConfigured() {
    val forUser999 = app(userId = "999", config = configForUserZero)

    assertFalse(forUser999.isConfigured)
    assertFalse(forUser999.isEnabled)
    assertFalse(forUser999.isMissing)
  }

  @Test
  fun sameUsersConfigMarksAppConfiguredAndEnabled() {
    val forUser0 = app(userId = "0", config = configForUserZero)

    assertTrue(forUser0.isConfigured)
    assertTrue(forUser0.isEnabled)
    assertFalse(forUser0.isMissing)
  }

  @Test
  fun disabledProfileStaysConfiguredButNotEnabled() {
    val disabled = AppConfig(users = mapOf("999" to UserProfile(enabled = false)))
    val forUser999 = app(userId = "999", config = disabled)

    assertTrue(forUser999.isConfigured)
    assertFalse(forUser999.isEnabled)
  }

  @Test
  fun configuredButNotInstalledReportsMissing() {
    val missing = app(userId = "999", installed = false, config = disabledFor("999"))

    assertTrue(missing.isConfigured)
    assertTrue(missing.isMissing)
    assertFalse(missing.isEnabled)
  }

  private fun disabledFor(userId: String) =
      AppConfig(users = mapOf(userId to UserProfile(enabled = false)))
}
