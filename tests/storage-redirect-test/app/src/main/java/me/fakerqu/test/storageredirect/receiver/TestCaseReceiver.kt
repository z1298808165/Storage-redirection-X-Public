package me.fakerqu.test.storageredirect.receiver

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.content.ContextCompat
import me.fakerqu.test.storageredirect.TestService
import me.fakerqu.test.storageredirect.test.TestCaseArgs

/**
 * 接收外部测试广播并启动 [TestService] 在后台执行用例。
 *
 * 示例：
 *
 * ```
 * # 先 create，从 log 或 metadata 取得 uri，再 read
 * adb shell am broadcast -a me.fakerqu.test.storageredirection.TEST_CASE \
 *   --es test_case mediastore_create_image
 *
 * adb shell am broadcast -a me.fakerqu.test.storageredirection.TEST_CASE \
 *   --es test_case mediastore_read_image \
 *   --es media_uri "content://media/external/images/media/12345" \
 *   --es expected_payload "storage-redirect-test:IMAGE:initial"
 *
 * adb shell am broadcast -a me.fakerqu.test.storageredirection.TEST_CASE \
 *   --es test_case file_write \
 *   --es file_path "/sdcard/Android/data/me.fakerqu.test.storageredirect/files/demo.txt" \
 *   --es payload "hello"
 * ```
 */
class TestCaseReceiver : BroadcastReceiver() {
  override fun onReceive(
      context: Context,
      intent: Intent?,
  ) {
    if (intent?.action != ACTION_TEST_CASE) return
    val serviceIntent = TestService.createIntent(context, intent)
    ContextCompat.startForegroundService(context, serviceIntent)
  }

  companion object {
    const val ACTION_TEST_CASE = "me.fakerqu.test.storageredirection.TEST_CASE"

    /** 用例 id，见 [me.fakerqu.test.storageredirect.test.TestCase] */
    const val EXTRA_TEST_CASE = "test_case"

    /** 与 [TestCaseArgs] 中 EXTRA_* 同名，通过广播 --es 传入 */
    val EXTRA_MEDIA_URI: String
      get() = TestCaseArgs.EXTRA_MEDIA_URI

    val EXTRA_FILE_PATH: String
      get() = TestCaseArgs.EXTRA_FILE_PATH

    val EXTRA_FILE_DIR: String
      get() = TestCaseArgs.EXTRA_FILE_DIR

    val EXTRA_FILE_NAME: String
      get() = TestCaseArgs.EXTRA_FILE_NAME

    val EXTRA_RELATIVE_PATH: String
      get() = TestCaseArgs.EXTRA_RELATIVE_PATH

    val EXTRA_PAYLOAD: String
      get() = TestCaseArgs.EXTRA_PAYLOAD

    val EXTRA_EXPECTED_PAYLOAD: String
      get() = TestCaseArgs.EXTRA_EXPECTED_PAYLOAD
  }
}
