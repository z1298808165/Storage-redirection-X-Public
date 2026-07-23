package me.fakerqu.mediafileapi

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

/**
 * 在 Android 设备上执行的插桩测试。
 *
 * 参见[测试文档](http://d.android.com/tools/testing)。
 */
@RunWith(AndroidJUnit4::class)
class ExampleInstrumentedTest {
  @Test
  fun useAppContext() {
    // 被测应用的上下文。
    val appContext = InstrumentationRegistry.getInstrumentation().targetContext
    assertEquals("me.fakerqu.mediafileapi.test", appContext.packageName)
  }
}
