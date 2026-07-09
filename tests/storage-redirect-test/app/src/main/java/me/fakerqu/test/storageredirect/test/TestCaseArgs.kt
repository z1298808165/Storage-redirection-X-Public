package me.fakerqu.test.storageredirect.test

import android.content.Intent
import android.net.Uri

/** 测试用例运行时参数。MediaStore 操作用 [mediaUri]；File 操作用 [filePath] / [fileDir]。 */
data class TestCaseArgs(
    val mediaUri: Uri? = null,
    val filePath: String? = null,
    val targetFilePath: String? = null,
    val fileDir: String? = null,
    val fileName: String? = null,
    val relativePath: String? = null,
    val payload: ByteArray? = null,
    val expectedPayload: ByteArray? = null,
    val expectedPath: String? = null,
    val length: Long? = null,
    val mode: Int? = null,
    val keepPending: Boolean = false,
) {
  fun payloadOr(default: ByteArray): ByteArray = payload ?: default

  fun expectedOr(default: ByteArray): ByteArray = expectedPayload ?: default

  companion object {
    const val EXTRA_MEDIA_URI = "media_uri"
    const val EXTRA_FILE_PATH = "file_path"
    const val EXTRA_TARGET_FILE_PATH = "target_file_path"
    const val EXTRA_FILE_DIR = "file_dir"
    const val EXTRA_FILE_NAME = "file_name"
    const val EXTRA_RELATIVE_PATH = "relative_path"
    const val EXTRA_PAYLOAD = "payload"
    const val EXTRA_EXPECTED_PAYLOAD = "expected_payload"
    const val EXTRA_EXPECTED_PATH = "expected_path"
    const val EXTRA_LENGTH = "length"
    const val EXTRA_MODE = "mode"
    const val EXTRA_KEEP_PENDING = "keep_pending"

    fun fromIntent(intent: Intent?): TestCaseArgs {
      if (intent == null) return TestCaseArgs()
      return TestCaseArgs(
          mediaUri = intent.getStringExtra(EXTRA_MEDIA_URI)?.let(Uri::parse),
          filePath = intent.getStringExtra(EXTRA_FILE_PATH),
          targetFilePath = intent.getStringExtra(EXTRA_TARGET_FILE_PATH),
          fileDir = intent.getStringExtra(EXTRA_FILE_DIR),
          fileName = intent.getStringExtra(EXTRA_FILE_NAME),
          relativePath = intent.getStringExtra(EXTRA_RELATIVE_PATH),
          payload = intent.getStringExtra(EXTRA_PAYLOAD)?.toByteArray(Charsets.UTF_8),
          expectedPayload =
              intent.getStringExtra(EXTRA_EXPECTED_PAYLOAD)?.toByteArray(Charsets.UTF_8),
          expectedPath = intent.getStringExtra(EXTRA_EXPECTED_PATH),
          length = intent.getStringExtra(EXTRA_LENGTH)?.toLongOrNull(),
          mode = intent.getStringExtra(EXTRA_MODE)?.let(::parseMode),
          keepPending =
              intent.getStringExtra(EXTRA_KEEP_PENDING)?.lowercase()?.let {
                it == "1" || it == "true" || it == "yes"
              } ?: false,
      )
    }

    fun copyExtras(
        from: Intent?,
        to: Intent,
    ) {
      if (from == null) return
      listOf(
              EXTRA_MEDIA_URI,
              EXTRA_FILE_PATH,
              EXTRA_TARGET_FILE_PATH,
              EXTRA_FILE_DIR,
              EXTRA_FILE_NAME,
              EXTRA_RELATIVE_PATH,
              EXTRA_PAYLOAD,
              EXTRA_EXPECTED_PAYLOAD,
              EXTRA_EXPECTED_PATH,
              EXTRA_LENGTH,
              EXTRA_MODE,
              EXTRA_KEEP_PENDING,
              me.fakerqu.test.storageredirect.receiver.TestCaseReceiver.EXTRA_TEST_CASE,
          )
          .forEach { key -> from.getStringExtra(key)?.let { to.putExtra(key, it) } }
    }

    private fun parseMode(value: String): Int? {
      val trimmed = value.trim()
      return when {
        trimmed.startsWith("0o", ignoreCase = true) -> trimmed.drop(2).toIntOrNull(8)

        trimmed.startsWith("0") && trimmed.length > 1 -> trimmed.toIntOrNull(8)

        else -> trimmed.toIntOrNull()
      }
    }
  }
}
