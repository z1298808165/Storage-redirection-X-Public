package me.fakerqu.test.storageredirect.test

import android.content.Context
import android.system.OsConstants
import java.io.File
import java.io.RandomAccessFile
import me.fakerqu.mediafileapi.AndroidFileApi

class FileTestCases(
    private val context: Context,
) {
  private val api = AndroidFileApi()
  private val defaultRoot: File
    get() =
        File(
            context.getExternalFilesDir("srt_file_tests")
                ?: error("external files dir unavailable"),
            "workspace",
        )

  fun listDir(args: TestCaseArgs): TestResult =
      TestCase.FILE_LIST_DIR.measure {
        val dirPath = args.fileDir ?: args.filePath
        if (dirPath.isNullOrBlank()) {
          return@measure TestCase.FILE_LIST_DIR.fail(
              message =
                  "missing required parameter: ${TestCaseArgs.EXTRA_FILE_DIR} or ${TestCaseArgs.EXTRA_FILE_PATH}",
          )
        }
        val dir = File(dirPath)
        if (!dir.isDirectory) {
          return@measure TestCase.FILE_LIST_DIR.fail(
              message = "path is not a directory",
              metadata = mapOf("path" to dirPath),
          )
        }
        val listed = dir.listFiles()?.toList().orEmpty()
        val entries =
            listed.map { file -> file.name.ifBlank { "." } }.sorted().take(MAX_LISTED_ENTRIES)
        TestCase.FILE_LIST_DIR.pass(
            message = "list completed",
            metadata =
                mapOf(
                    "path" to dirPath,
                    "listedCount" to listed.size.toString(),
                    "entries" to entries.joinToString("|"),
                ),
        )
      }

  fun create(args: TestCaseArgs): TestResult =
      TestCase.FILE_CREATE.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_CREATE)
                ?: return@measure args.missingPathResult(TestCase.FILE_CREATE)
        val created = api.createFile(targetPath)
        if (!created.existsEventually()) {
          return@measure TestCase.FILE_CREATE.fail(
              message = "createFile failed",
              metadata = mapOf("path" to targetPath),
          )
        }
        args.payload?.let { payload -> created.writeBytes(payload) }
        TestCase.FILE_CREATE.pass(
            message = "file created",
            metadata = mapOf("path" to targetPath),
        )
      }

  fun read(args: TestCaseArgs): TestResult =
      TestCase.FILE_READ.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_READ)
                ?: return@measure args.missingPathResult(TestCase.FILE_READ)
        val target = File(targetPath)
        if (!target.isFile) {
          return@measure TestCase.FILE_READ.fail(
              message = "file does not exist",
              metadata = mapOf("path" to targetPath),
          )
        }
        val readBack = api.readFile(targetPath).use { it.readBytes() }
        val expected = args.expectedPayload
        if (expected != null && !readBack.contentEquals(expected)) {
          return@measure TestCase.FILE_READ.fail(
              message = "read content mismatch",
              metadata = pathMetadata(targetPath),
          )
        }
        TestCase.FILE_READ.pass(
            message = if (expected != null) "read matched expected payload" else "read completed",
            metadata = pathMetadata(targetPath) + mapOf("bytesRead" to readBack.size.toString()),
        )
      }

  fun write(args: TestCaseArgs): TestResult =
      TestCase.FILE_WRITE.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_WRITE)
                ?: return@measure args.missingPathResult(TestCase.FILE_WRITE)
        val target = File(targetPath)
        if (!target.exists()) {
          api.createFile(targetPath)
        }
        val payload = args.payloadOr(TestFixtures.filePayload("write"))
        api.writeFile(targetPath).use {
          it.write(payload)
          it.flush()
        }
        if (args.expectedPayload != null) {
          val readBack = target.readBytes()
          if (!readBack.contentEquals(args.expectedPayload)) {
            return@measure TestCase.FILE_WRITE.fail(
                message = "read after write did not match expected_payload",
                metadata = pathMetadata(targetPath),
            )
          }
        }
        TestCase.FILE_WRITE.pass(
            message = "write succeeded",
            metadata = pathMetadata(targetPath) + mapOf("bytesWritten" to payload.size.toString()),
        )
      }

  fun overwrite(args: TestCaseArgs): TestResult =
      TestCase.FILE_OVERWRITE.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_OVERWRITE)
                ?: return@measure args.missingPathResult(TestCase.FILE_OVERWRITE)
        val target = File(targetPath)
        if (!target.isFile) {
          return@measure TestCase.FILE_OVERWRITE.fail(
              message = "file does not exist before overwrite",
              metadata = pathMetadata(targetPath),
          )
        }
        val original = target.readBytes()
        val payload = args.payloadOr(TestFixtures.filePayload("overwrite"))
        if (payload.size > original.size) {
          return@measure TestCase.FILE_OVERWRITE.fail(
              message = "payload exceeds existing file size",
              metadata =
                  pathMetadata(targetPath) +
                      mapOf(
                          "originalSize" to original.size.toString(),
                          "payloadSize" to payload.size.toString(),
                      ),
          )
        }
        RandomAccessFile(target, "rw").use {
          it.seek(0)
          it.write(payload)
          it.fd.sync()
        }
        val readBack = target.readBytes()
        val expected = original.copyOf().also { payload.copyInto(it) }
        if (!readBack.contentEquals(expected)) {
          return@measure TestCase.FILE_OVERWRITE.fail(
              message = "read after overwrite did not preserve existing suffix",
              metadata = pathMetadata(targetPath),
          )
        }
        TestCase.FILE_OVERWRITE.pass(
            message = "in-place overwrite succeeded",
            metadata =
                pathMetadata(targetPath) +
                    mapOf(
                        "bytesWritten" to payload.size.toString(),
                        "fileSize" to readBack.size.toString(),
                    ),
        )
      }

  fun writeThenOverwrite(args: TestCaseArgs): TestResult =
      TestCase.FILE_WRITE_THEN_OVERWRITE.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_WRITE_THEN_OVERWRITE)
                ?: return@measure args.missingPathResult(TestCase.FILE_WRITE_THEN_OVERWRITE)
        val seed =
            args.expectedPayload
                ?: return@measure TestCase.FILE_WRITE_THEN_OVERWRITE.fail(
                    message = "missing required parameter: ${TestCaseArgs.EXTRA_EXPECTED_PAYLOAD}",
                    metadata = pathMetadata(targetPath),
                )
        val payload = args.payloadOr(TestFixtures.filePayload("overwrite"))
        if (payload.size > seed.size) {
          return@measure TestCase.FILE_WRITE_THEN_OVERWRITE.fail(
              message = "payload exceeds seed size",
              metadata =
                  pathMetadata(targetPath) +
                      mapOf(
                          "seedSize" to seed.size.toString(),
                          "payloadSize" to payload.size.toString(),
                      ),
          )
        }
        val readBack =
            RandomAccessFile(File(targetPath), "rw").use { file ->
              file.write(seed)
              file.fd.sync()
              Thread.sleep(OVERWRITE_SETTLE_DELAY_MS)
              file.seek(0)
              file.write(payload)
              file.fd.sync()
              ByteArray(seed.size).also {
                file.seek(0)
                file.readFully(it)
              }
            }
        val expected = seed.copyOf().also { payload.copyInto(it) }
        if (!readBack.contentEquals(expected)) {
          return@measure TestCase.FILE_WRITE_THEN_OVERWRITE.fail(
              message = "read after overwrite did not preserve seed suffix",
              metadata = pathMetadata(targetPath),
          )
        }
        TestCase.FILE_WRITE_THEN_OVERWRITE.pass(
            message = "write and in-place overwrite succeeded",
            metadata =
                pathMetadata(targetPath) +
                    mapOf(
                        "bytesWritten" to payload.size.toString(),
                        "fileSize" to readBack.size.toString(),
                    ),
        )
      }

  fun writeDenied(args: TestCaseArgs): TestResult =
      TestCase.FILE_WRITE_DENIED.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_WRITE_DENIED)
                ?: return@measure args.missingPathResult(TestCase.FILE_WRITE_DENIED)
        val target = File(targetPath)
        val parent = target.parentFile
        if (parent != null && !parent.isDirectory) {
          return@measure TestCase.FILE_WRITE_DENIED.fail(
              message = "parent directory does not exist before write",
              metadata = pathMetadata(targetPath),
          )
        }
        val payload = args.payloadOr(TestFixtures.filePayload("write-denied"))
        try {
          api.writeFile(targetPath).use {
            it.write(payload)
            it.flush()
          }
        } catch (e: Exception) {
          return@measure TestCase.FILE_WRITE_DENIED.pass(
              message = "write denied as expected",
              metadata = pathMetadata(targetPath) + exceptionMetadata(e),
          )
        }
        TestCase.FILE_WRITE_DENIED.fail(
            message = "write unexpectedly succeeded",
            metadata = pathMetadata(targetPath),
        )
      }

  fun delete(args: TestCaseArgs): TestResult =
      TestCase.FILE_DELETE.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_DELETE)
                ?: return@measure args.missingPathResult(TestCase.FILE_DELETE)
        val target = File(targetPath)
        if (!target.exists()) {
          return@measure TestCase.FILE_DELETE.fail(
              message = "path does not exist",
              metadata = pathMetadata(targetPath),
          )
        }
        val deleted = api.deleteFile(targetPath)
        if (!deleted || target.exists()) {
          return@measure TestCase.FILE_DELETE.fail(
              message = "deleteFile failed",
              metadata = pathMetadata(targetPath),
          )
        }
        TestCase.FILE_DELETE.pass(message = "delete succeeded", metadata = pathMetadata(targetPath))
      }

  fun deleteDenied(args: TestCaseArgs): TestResult =
      TestCase.FILE_DELETE_DENIED.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_DELETE_DENIED)
                ?: return@measure args.missingPathResult(TestCase.FILE_DELETE_DENIED)
        val target = File(targetPath)
        if (!target.exists()) {
          return@measure TestCase.FILE_DELETE_DENIED.fail(
              message = "path does not exist before delete",
              metadata = pathMetadata(targetPath),
          )
        }
        try {
          val deleted = api.deleteFile(targetPath)
          if (deleted || !target.exists()) {
            return@measure TestCase.FILE_DELETE_DENIED.fail(
                message = "delete unexpectedly succeeded",
                metadata = pathMetadata(targetPath),
            )
          }
        } catch (e: Exception) {
          return@measure TestCase.FILE_DELETE_DENIED.pass(
              message = "delete denied as expected",
              metadata = pathMetadata(targetPath) + exceptionMetadata(e),
          )
        }
        TestCase.FILE_DELETE_DENIED.pass(
            message = "delete denied as expected",
            metadata = pathMetadata(targetPath),
        )
      }

  fun mkdir(args: TestCaseArgs): TestResult =
      TestCase.FILE_MKDIR.measure {
        val dirPath =
            args.requireFileDirOrPath(TestCase.FILE_MKDIR)
                ?: return@measure args.missingDirOrPathResult(TestCase.FILE_MKDIR)
        val dir = File(dirPath)
        val created = api.mkdir(dirPath)
        if (!created || !dir.isDirectory) {
          return@measure TestCase.FILE_MKDIR.fail(
              message = "mkdir failed",
              metadata = pathMetadata(dirPath),
          )
        }
        TestCase.FILE_MKDIR.pass(
            message = "directory created",
            metadata = pathMetadata(dirPath),
        )
      }

  fun mkdirDenied(args: TestCaseArgs): TestResult =
      TestCase.FILE_MKDIR_DENIED.measure {
        val dirPath =
            args.requireFileDirOrPath(TestCase.FILE_MKDIR_DENIED)
                ?: return@measure args.missingDirOrPathResult(TestCase.FILE_MKDIR_DENIED)
        val dir = File(dirPath)
        val parent = dir.parentFile
        if (parent != null && !parent.isDirectory) {
          return@measure TestCase.FILE_MKDIR_DENIED.fail(
              message = "parent directory does not exist before mkdir",
              metadata = pathMetadata(dirPath),
          )
        }
        if (dir.exists()) {
          return@measure TestCase.FILE_MKDIR_DENIED.fail(
              message = "directory already exists before mkdir",
              metadata = pathMetadata(dirPath),
          )
        }
        try {
          val created = api.mkdir(dirPath)
          if (created || dir.exists()) {
            return@measure TestCase.FILE_MKDIR_DENIED.fail(
                message = "mkdir unexpectedly succeeded",
                metadata = pathMetadata(dirPath),
            )
          }
        } catch (e: Exception) {
          return@measure TestCase.FILE_MKDIR_DENIED.pass(
              message = "mkdir denied as expected",
              metadata = pathMetadata(dirPath) + exceptionMetadata(e),
          )
        }
        TestCase.FILE_MKDIR_DENIED.pass(
            message = "mkdir denied as expected",
            metadata = pathMetadata(dirPath),
        )
      }

  fun rename(args: TestCaseArgs): TestResult =
      TestCase.FILE_RENAME.measure {
        val fromPath =
            args.requireFilePath(TestCase.FILE_RENAME)
                ?: return@measure args.missingPathResult(TestCase.FILE_RENAME)
        val toPath =
            args.requireTargetFilePath(TestCase.FILE_RENAME)
                ?: return@measure args.missingTargetPathResult(TestCase.FILE_RENAME)
        val source = File(fromPath)
        val target = File(toPath)
        if (!source.exists()) {
          return@measure TestCase.FILE_RENAME.fail(
              message = "source does not exist",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        target.parentFile?.mkdirs()
        if (target.exists()) target.deleteRecursively()
        val renamed = api.renameFile(fromPath, toPath)
        if (!renamed || source.exists() || !target.exists()) {
          return@measure TestCase.FILE_RENAME.fail(
              message = "rename failed",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        TestCase.FILE_RENAME.pass(
            message = "rename succeeded",
            metadata = renameMetadata(fromPath, toPath),
        )
      }

  fun renameDenied(args: TestCaseArgs): TestResult =
      TestCase.FILE_RENAME_DENIED.measure {
        val fromPath =
            args.requireFilePath(TestCase.FILE_RENAME_DENIED)
                ?: return@measure args.missingPathResult(TestCase.FILE_RENAME_DENIED)
        val toPath =
            args.requireTargetFilePath(TestCase.FILE_RENAME_DENIED)
                ?: return@measure args.missingTargetPathResult(TestCase.FILE_RENAME_DENIED)
        val source = File(fromPath)
        val target = File(toPath)
        if (!source.exists()) {
          return@measure TestCase.FILE_RENAME_DENIED.fail(
              message = "source does not exist before rename",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        val targetParent = target.parentFile
        if (targetParent != null && !targetParent.isDirectory) {
          return@measure TestCase.FILE_RENAME_DENIED.fail(
              message = "target parent directory does not exist before rename",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        if (target.exists()) {
          return@measure TestCase.FILE_RENAME_DENIED.fail(
              message = "target already exists before rename",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        try {
          val renamed = api.renameFile(fromPath, toPath)
          if (renamed || !source.exists() || target.exists()) {
            return@measure TestCase.FILE_RENAME_DENIED.fail(
                message = "rename unexpectedly succeeded",
                metadata = renameMetadata(fromPath, toPath),
            )
          }
        } catch (e: Exception) {
          return@measure TestCase.FILE_RENAME_DENIED.pass(
              message = "rename denied as expected",
              metadata = renameMetadata(fromPath, toPath) + exceptionMetadata(e),
          )
        }
        TestCase.FILE_RENAME_DENIED.pass(
            message = "rename denied as expected",
            metadata = renameMetadata(fromPath, toPath),
        )
      }

  fun stat(args: TestCaseArgs): TestResult =
      TestCase.FILE_STAT.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_STAT)
                ?: return@measure args.missingPathResult(TestCase.FILE_STAT)
        val stat = api.statFile(targetPath)
        TestCase.FILE_STAT.pass(
            message = "stat succeeded",
            metadata = pathMetadata(targetPath) + statMetadata(stat),
        )
      }

  fun access(args: TestCaseArgs): TestResult =
      TestCase.FILE_ACCESS.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_ACCESS)
                ?: return@measure args.missingPathResult(TestCase.FILE_ACCESS)
        val mode = args.mode ?: OsConstants.F_OK
        val accessible = api.accessFile(targetPath, mode)
        if (!accessible) {
          return@measure TestCase.FILE_ACCESS.fail(
              message = "access returned false",
              metadata = pathMetadata(targetPath) + mapOf("mode" to mode.toString()),
          )
        }
        TestCase.FILE_ACCESS.pass(
            message = "access succeeded",
            metadata = pathMetadata(targetPath) + mapOf("mode" to mode.toString()),
        )
      }

  fun readlink(args: TestCaseArgs): TestResult =
      TestCase.FILE_READLINK.measure {
        val targetPath =
            args.requireFilePath(TestCase.FILE_READLINK)
                ?: return@measure args.missingPathResult(TestCase.FILE_READLINK)
        val actual = api.readLink(targetPath)
        val expected = args.expectedPath ?: args.expectedPayload?.toString(Charsets.UTF_8)
        if (expected != null && actual != expected) {
          return@measure TestCase.FILE_READLINK.fail(
              message = "readlink target mismatch",
              metadata =
                  pathMetadata(targetPath) +
                      mapOf(
                          "expectedPath" to expected,
                          "actualPath" to actual,
                      ),
          )
        }
        TestCase.FILE_READLINK.pass(
            message = "readlink succeeded",
            metadata = pathMetadata(targetPath) + mapOf("target" to actual),
        )
      }

  fun truncate(args: TestCaseArgs): TestResult =
      truncateCommon(TestCase.FILE_TRUNCATE, args, useFd = false, expectDenied = false)

  fun truncateDenied(args: TestCaseArgs): TestResult =
      truncateCommon(TestCase.FILE_TRUNCATE_DENIED, args, useFd = false, expectDenied = true)

  fun ftruncate(args: TestCaseArgs): TestResult =
      truncateCommon(TestCase.FILE_FTRUNCATE, args, useFd = true, expectDenied = false)

  fun ftruncateDenied(args: TestCaseArgs): TestResult =
      truncateCommon(TestCase.FILE_FTRUNCATE_DENIED, args, useFd = true, expectDenied = true)

  fun chmod(args: TestCaseArgs): TestResult =
      chmodCommon(TestCase.FILE_CHMOD, args, useFd = false, expectDenied = false)

  fun chmodDenied(args: TestCaseArgs): TestResult =
      chmodCommon(TestCase.FILE_CHMOD_DENIED, args, useFd = false, expectDenied = true)

  fun fchmod(args: TestCaseArgs): TestResult =
      chmodCommon(TestCase.FILE_FCHMOD, args, useFd = true, expectDenied = false)

  fun fchmodDenied(args: TestCaseArgs): TestResult =
      chmodCommon(TestCase.FILE_FCHMOD_DENIED, args, useFd = true, expectDenied = true)

  fun link(args: TestCaseArgs): TestResult =
      linkCommon(TestCase.FILE_LINK, args, symbolic = false, expectDenied = false)

  fun linkDenied(args: TestCaseArgs): TestResult =
      linkCommon(TestCase.FILE_LINK_DENIED, args, symbolic = false, expectDenied = true)

  fun symlink(args: TestCaseArgs): TestResult =
      linkCommon(TestCase.FILE_SYMLINK, args, symbolic = true, expectDenied = false)

  fun symlinkDenied(args: TestCaseArgs): TestResult =
      linkCommon(TestCase.FILE_SYMLINK_DENIED, args, symbolic = true, expectDenied = true)

  fun prepareBootstrapDir(caseId: String): File {
    val dir = File(defaultRoot, caseId)
    if (dir.exists()) dir.deleteRecursively()
    dir.mkdirs()
    return dir
  }

  private fun pathMetadata(path: String): Map<String, String> = mapOf("path" to path)

  private fun renameMetadata(
      fromPath: String,
      toPath: String,
  ): Map<String, String> = mapOf("from" to fromPath, "to" to toPath)

  private fun statMetadata(stat: me.fakerqu.mediafileapi.FileStatInfo): Map<String, String> =
      mapOf(
          "size" to stat.size.toString(),
          "mode" to stat.mode.toString(),
          "uid" to stat.uid.toString(),
          "gid" to stat.gid.toString(),
          "modifiedSeconds" to stat.modifiedSeconds.toString(),
      )

  private fun truncateCommon(
      testCase: TestCase,
      args: TestCaseArgs,
      useFd: Boolean,
      expectDenied: Boolean,
  ): TestResult =
      testCase.measure {
        val targetPath =
            args.requireFilePath(testCase) ?: return@measure args.missingPathResult(testCase)
        val target = File(targetPath)
        if (!target.isFile) {
          return@measure testCase.fail(
              message = "file does not exist before truncate",
              metadata = pathMetadata(targetPath),
          )
        }
        val originalSize = target.length()
        val length = args.length ?: DEFAULT_TRUNCATE_LENGTH
        try {
          if (useFd) {
            api.ftruncateFile(targetPath, length)
          } else {
            api.truncateFile(targetPath, length)
          }
        } catch (e: Exception) {
          return@measure if (expectDenied) {
            testCase.pass(
                message = "truncate denied as expected",
                metadata = pathMetadata(targetPath) + exceptionMetadata(e),
            )
          } else {
            testCase.fail(
                message = "truncate failed",
                metadata = pathMetadata(targetPath) + exceptionMetadata(e),
            )
          }
        }

        val actualSize = target.length()
        if (expectDenied) {
          return@measure testCase.fail(
              message = "truncate unexpectedly succeeded",
              metadata =
                  pathMetadata(targetPath) +
                      mapOf(
                          "originalSize" to originalSize.toString(),
                          "actualSize" to actualSize.toString(),
                      ),
          )
        }
        if (actualSize != length) {
          return@measure testCase.fail(
              message = "truncate length mismatch",
              metadata =
                  pathMetadata(targetPath) +
                      mapOf(
                          "expectedSize" to length.toString(),
                          "actualSize" to actualSize.toString(),
                      ),
          )
        }
        testCase.pass(
            message = "truncate succeeded",
            metadata = pathMetadata(targetPath) + mapOf("size" to actualSize.toString()),
        )
      }

  private fun chmodCommon(
      testCase: TestCase,
      args: TestCaseArgs,
      useFd: Boolean,
      expectDenied: Boolean,
  ): TestResult =
      testCase.measure {
        val targetPath =
            args.requireFilePath(testCase) ?: return@measure args.missingPathResult(testCase)
        if (!File(targetPath).exists()) {
          return@measure testCase.fail(
              message = "path does not exist before chmod",
              metadata = pathMetadata(targetPath),
          )
        }
        val mode = args.mode ?: DEFAULT_CHMOD_MODE
        try {
          if (useFd) {
            api.fchmodFile(targetPath, mode)
          } else {
            api.chmodFile(targetPath, mode)
          }
        } catch (e: Exception) {
          return@measure if (expectDenied) {
            testCase.pass(
                message = "chmod denied as expected",
                metadata = pathMetadata(targetPath) + exceptionMetadata(e),
            )
          } else {
            testCase.fail(
                message = "chmod failed",
                metadata = pathMetadata(targetPath) + exceptionMetadata(e),
            )
          }
        }
        if (expectDenied) {
          return@measure testCase.fail(
              message = "chmod unexpectedly succeeded",
              metadata = pathMetadata(targetPath) + mapOf("mode" to mode.toString()),
          )
        }
        testCase.pass(
            message = "chmod succeeded",
            metadata = pathMetadata(targetPath) + mapOf("mode" to mode.toString()),
        )
      }

  private fun linkCommon(
      testCase: TestCase,
      args: TestCaseArgs,
      symbolic: Boolean,
      expectDenied: Boolean,
  ): TestResult =
      testCase.measure {
        val fromPath =
            args.requireFilePath(testCase) ?: return@measure args.missingPathResult(testCase)
        val toPath =
            args.requireTargetFilePath(testCase)
                ?: return@measure args.missingTargetPathResult(testCase)
        val target = File(toPath)
        target.parentFile?.mkdirs()
        if (target.exists()) {
          return@measure testCase.fail(
              message = "target already exists before link",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        try {
          if (symbolic) {
            api.symlinkFile(fromPath, toPath)
          } else {
            api.linkFile(fromPath, toPath)
          }
        } catch (e: Exception) {
          return@measure if (expectDenied) {
            testCase.pass(
                message = "link denied as expected",
                metadata = renameMetadata(fromPath, toPath) + exceptionMetadata(e),
            )
          } else {
            testCase.fail(
                message = "link failed",
                metadata = renameMetadata(fromPath, toPath) + exceptionMetadata(e),
            )
          }
        }
        if (expectDenied) {
          return@measure testCase.fail(
              message = "link unexpectedly succeeded",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        val created =
            if (symbolic) {
              runCatching { api.readLink(toPath) == fromPath }.getOrDefault(false)
            } else {
              target.exists()
            }
        if (!created) {
          return@measure testCase.fail(
              message = "link target missing after operation",
              metadata = renameMetadata(fromPath, toPath),
          )
        }
        testCase.pass(
            message = "link succeeded",
            metadata = renameMetadata(fromPath, toPath),
        )
      }

  private fun exceptionMetadata(e: Exception): Map<String, String> =
      mapOf("exception" to e.javaClass.simpleName, "error" to (e.message ?: ""))

  private fun File.existsEventually(): Boolean {
    repeat(EXISTS_RETRY_COUNT) { index ->
      if (exists()) return true
      if (index < EXISTS_RETRY_COUNT - 1) {
        Thread.sleep(EXISTS_RETRY_DELAY_MS)
      }
    }
    return exists()
  }

  companion object {
    private const val MAX_LISTED_ENTRIES = 40
    private const val EXISTS_RETRY_COUNT = 10
    private const val EXISTS_RETRY_DELAY_MS = 100L
    private const val OVERWRITE_SETTLE_DELAY_MS = 1800L
    private const val DEFAULT_TRUNCATE_LENGTH = 4L
    private const val DEFAULT_CHMOD_MODE = 384
  }
}

private fun TestCaseArgs.requireFilePath(testCase: TestCase): String? = filePath

private fun TestCaseArgs.requireTargetFilePath(testCase: TestCase): String? = targetFilePath

private fun TestCaseArgs.requireFileDirOrPath(testCase: TestCase): String? = fileDir ?: filePath

private fun TestCaseArgs.missingPathResult(testCase: TestCase): TestResult =
    testCase.fail(
        message = "missing required parameter: ${TestCaseArgs.EXTRA_FILE_PATH}",
        metadata = mapOf("hint" to "pass absolute path via --es ${TestCaseArgs.EXTRA_FILE_PATH}"),
    )

private fun TestCaseArgs.missingTargetPathResult(testCase: TestCase): TestResult =
    testCase.fail(
        message = "missing required parameter: ${TestCaseArgs.EXTRA_TARGET_FILE_PATH}",
        metadata =
            mapOf("hint" to "pass target path via --es ${TestCaseArgs.EXTRA_TARGET_FILE_PATH}"),
    )

private fun TestCaseArgs.missingDirOrPathResult(testCase: TestCase): TestResult =
    testCase.fail(
        message =
            "missing required parameter: ${TestCaseArgs.EXTRA_FILE_DIR} or ${TestCaseArgs.EXTRA_FILE_PATH}",
        metadata = mapOf("hint" to "pass directory path via --es ${TestCaseArgs.EXTRA_FILE_DIR}"),
    )
