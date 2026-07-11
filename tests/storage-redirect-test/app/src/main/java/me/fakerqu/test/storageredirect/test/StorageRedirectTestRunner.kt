package me.fakerqu.test.storageredirect.test

import android.content.Context
import android.net.Uri
import android.util.Log
import java.io.File
import me.fakerqu.mediafileapi.MediaStoreApi

class StorageRedirectTestRunner(
    private val context: Context,
) {
  fun run(
      testCase: TestCase,
      args: TestCaseArgs = TestCaseArgs(),
  ): List<TestResult> {
    val results =
        if (testCase == TestCase.ALL_EXCEPT_DELETE) {
          runAllExceptDelete(args)
        } else {
          listOf(testCase.run(context, args))
        }
    logSummary(results)
    return results
  }

  /**
   * ALL 模式：不依赖外部 URI/路径，先 create 再将其作为参数传给 read/write。 MediaStore query
   * 用例保留为独立用例，由设备侧脚本逐项触发，避免全量查询干扰长链路聚合。
   */
  private fun runAllExceptDelete(overrides: TestCaseArgs): List<TestResult> {
    val results = mutableListOf<TestResult>()
    val mediaStore = MediaStoreTestCases(context)
    val fileCases = FileTestCases(context)
    val createdMedia = mutableListOf<Uri>()
    val bootstrapDirs = mutableListOf<File>()

    try {
      val mediaTypes =
          listOf(
              MediaStoreApi.MediaType.IMAGE to
                  listOf(
                      TestCase.MEDIASTORE_CREATE_IMAGE,
                      TestCase.MEDIASTORE_READ_IMAGE,
                      TestCase.MEDIASTORE_WRITE_IMAGE,
                  ),
              MediaStoreApi.MediaType.VIDEO to
                  listOf(
                      TestCase.MEDIASTORE_CREATE_VIDEO,
                      TestCase.MEDIASTORE_READ_VIDEO,
                      TestCase.MEDIASTORE_WRITE_VIDEO,
                  ),
              MediaStoreApi.MediaType.AUDIO to
                  listOf(
                      TestCase.MEDIASTORE_CREATE_AUDIO,
                      TestCase.MEDIASTORE_READ_AUDIO,
                      TestCase.MEDIASTORE_WRITE_AUDIO,
                  ),
              MediaStoreApi.MediaType.FILE to
                  listOf(
                      TestCase.MEDIASTORE_CREATE_FILE,
                      TestCase.MEDIASTORE_READ_FILE,
                      TestCase.MEDIASTORE_WRITE_FILE,
                  ),
              MediaStoreApi.MediaType.DOWNLOAD to
                  listOf(
                      TestCase.MEDIASTORE_CREATE_DOWNLOAD,
                      TestCase.MEDIASTORE_READ_DOWNLOAD,
                      TestCase.MEDIASTORE_WRITE_DOWNLOAD,
                  ),
          )

      for ((mediaType, chain) in mediaTypes) {
        val createCase = chain.first()
        val createResult = runLogged(createCase, overrides.copy(keepPending = true))
        results += createResult
        val uri = createResult.metadata["uri"]?.let(Uri::parse)
        if (uri == null) {
          chain.drop(1).forEach { skipped ->
            results += skipped.fail("skipped: create did not return uri")
          }
          continue
        }
        createdMedia += uri
        val initial = TestFixtures.initialPayload(mediaType)
        val updated = TestFixtures.updatedPayload(mediaType)
        chain.drop(1).forEach { case ->
          val caseArgs =
              when {
                case.id.contains("_read_") ->
                    TestCaseArgs(
                        mediaUri = uri,
                        expectedPayload = initial,
                    )

                case.id.contains("_write_") ->
                    TestCaseArgs(
                        mediaUri = uri,
                        payload = updated,
                        expectedPayload = updated,
                    )

                case.id.contains("_delete_") -> TestCaseArgs(mediaUri = uri)
                else -> TestCaseArgs(mediaUri = uri)
              }
          results += runLogged(case, caseArgs)
        }
      }

      val fileDir = fileCases.prepareBootstrapDir("all_file_list")
      bootstrapDirs += fileDir
      File(fileDir, "a.txt").writeText("a")
      File(fileDir, "nested/b.txt").apply {
        parentFile?.mkdirs()
        writeText("b")
      }
      results +=
          runLogged(
              TestCase.FILE_LIST_DIR,
              TestCaseArgs(fileDir = fileDir.absolutePath),
          )

      val fileRoot = fileCases.prepareBootstrapDir("all_file_rw")
      bootstrapDirs += fileRoot
      val filePath = File(fileRoot, "target.txt").absolutePath
      val filePayload = TestFixtures.filePayload("all")
      results +=
          runLogged(
              TestCase.FILE_CREATE,
              TestCaseArgs(filePath = filePath, payload = filePayload),
          )
      results +=
          runLogged(
              TestCase.FILE_READ,
              TestCaseArgs(filePath = filePath, expectedPayload = filePayload),
          )
      results +=
          runLogged(
              TestCase.FILE_WRITE,
              TestCaseArgs(
                  filePath = filePath,
                  payload = TestFixtures.filePayload("all-updated"),
                  expectedPayload = TestFixtures.filePayload("all-updated"),
              ),
          )
      results +=
          runLogged(
              TestCase.FILE_OVERWRITE,
              TestCaseArgs(
                  filePath = filePath,
                  payload = "overwrite".toByteArray(),
              ),
          )
      results +=
          runLogged(
              TestCase.FILE_STAT,
              TestCaseArgs(filePath = filePath),
          )
      results +=
          runLogged(
              TestCase.FILE_ACCESS,
              TestCaseArgs(filePath = filePath),
          )
      results +=
          runLogged(
              TestCase.FILE_TRUNCATE,
              TestCaseArgs(filePath = filePath, length = 4),
          )
      results +=
          runLogged(
              TestCase.FILE_FTRUNCATE,
              TestCaseArgs(filePath = filePath, length = 8),
          )
    } finally {
      cleanupAllArtifacts(createdMedia, bootstrapDirs)
    }

    return results
  }

  private fun cleanupAllArtifacts(
      createdMedia: List<Uri>,
      bootstrapDirs: List<File>,
  ) {
    createdMedia.forEach { uri ->
      try {
        context.contentResolver.delete(uri, null, null)
      } catch (e: Exception) {
        Log.w(TAG, "failed to delete test media $uri", e)
      }
    }
    bootstrapDirs.forEach { dir ->
      try {
        dir.deleteRecursively()
      } catch (e: Exception) {
        Log.w(TAG, "failed to delete test dir ${dir.absolutePath}", e)
      }
    }
  }

  private fun runLogged(
      testCase: TestCase,
      args: TestCaseArgs = TestCaseArgs(),
  ): TestResult {
    Log.i(TAG, "running ${testCase.id}")
    return testCase.run(context, args).also { result ->
      Log.i(TAG, "completed ${result.toLogLine()}")
    }
  }

  private fun logSummary(results: List<TestResult>) {
    val passed = results.count { it.passed }
    val failed = results.size - passed
    Log.i(TAG, "summary: total=${results.size} passed=$passed failed=$failed")
    results.forEach { result ->
      if (result.passed) {
        Log.i(TAG, result.toLogLine())
      } else {
        Log.w(TAG, result.toLogLine())
      }
    }
  }

  companion object {
    private const val TAG = "StorageRedirectTest"
  }
}
