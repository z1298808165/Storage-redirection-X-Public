package me.fakerqu.test.storageredirect.test

import java.io.File
import java.io.FileNotFoundException
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder

class TestCaseSupportTest {
  @get:Rule val temporary = TemporaryFolder()

  @Test
  fun emptyDirectoryAndListingFailureAreDistinct() {
    assertEquals(0, temporary.root.listFilesOrFail().size)
    File(temporary.root, "entry").writeText("content")
    assertEquals(listOf("entry"), temporary.root.listFilesOrFail().map { it.name })
    val inaccessible =
        object : File(temporary.root, "inaccessible") {
          override fun listFiles(): Array<File>? = null
        }
    assertThrows(IOException::class.java) { inaccessible.listFilesOrFail() }
  }

  @Test
  fun deniedOperationsRequireAnAccessError() {
    for (errno in listOf("EACCES", "EPERM", "EROFS")) {
      assertTrue(isAccessDenied(IOException("open failed: $errno (denied)")))
      assertTrue(isAccessDenied(IOException("wrapped", IOException("$errno (denied)"))))
      assertTrue(isAccessDenied(IOException(errno)))
    }
    for (errno in
        listOf(
            "EIO",
            "ENOENT",
            "ENOSPC",
            "ENOMEM",
            "ENOTDIR",
            "EINVAL",
            "ENOSYS",
            "EXDEV",
            "ELOOP",
            "EEXIST",
            "ENOTEMPTY",
            "ENAMETOOLONG",
            "EBADF",
            "EINTR",
            "ETIMEDOUT",
            "EBUSY",
            "EISDIR",
            "ENFILE",
            "EMFILE",
            "EDQUOT",
            "ESTALE",
            "EFAULT",
            "ENODEV",
            "EOPNOTSUPP",
        )) {
      assertFalse(isAccessDenied(IOException("open failed: $errno (Permission denied)")))
    }
    assertFalse(isAccessDenied(IllegalStateException("EACCES")))
    assertFalse(isAccessDenied(IOException("/path/EACCES: open failed: EIO")))
    assertFalse(isAccessDenied(IOException("Permission denied")))
    assertFalse(isAccessDenied(IOException("EACCES_SUFFIX")))
    assertFalse(isAccessDenied(IOException("NOT_EACCES")))
    assertFalse(isAccessDenied(IOException("/failed: EACCES (directory)/file: open failed: EIO")))
    assertFalse(
        isAccessDenied(
            IOException("/failed: EACCES (directory)/file: open failed: EIO (Input/output error)")
        )
    )
    assertFalse(isAccessDenied(IOException("open failed: EACCES\nadditional diagnostic")))
    assertTrue(isAccessDenied(java.nio.file.AccessDeniedException("/file")))
    assertFalse(isAccessDenied(IOException("open failed: EACCES", IOException("read failed: EIO"))))
    assertTrue(isAccessDenied(IOException("open failed: EIO", IOException("read failed: EACCES"))))
    val wrapped = java.nio.file.AccessDeniedException("/file")
    wrapped.initCause(IOException("open failed: EIO"))
    assertFalse(isAccessDenied(wrapped))
  }

  @Test
  fun binderFileErrorsKeepExactReason() {
    for (reason in
        listOf("Permission denied", "Operation not permitted", "Read-only file system")) {
      assertTrue(isAccessDenied(FileNotFoundException("/path/pending: $reason")))
      assertFalse(isAccessDenied(IOException("/path/pending: $reason")))
      assertFalse(isAccessDenied(FileNotFoundException("$reason")))
      assertFalse(isAccessDenied(FileNotFoundException("/path/$reason: Input/output error")))
      assertFalse(isAccessDenied(FileNotFoundException("/path: $reason extra")))
      assertFalse(isAccessDenied(FileNotFoundException("/path: $reason\nextra")))
      val wrapped = FileNotFoundException("/path: $reason")
      wrapped.initCause(IOException("open failed: EIO"))
      assertFalse(isAccessDenied(wrapped))
    }
    for (reason in
        listOf("Input/output error", "No such file or directory", "No space left on device")) {
      assertFalse(isAccessDenied(FileNotFoundException("/path/Permission denied: $reason")))
    }
  }

  @Test
  fun cyclicExceptionCausesDoNotHangTheReporter() {
    val first = IOException("EIO")
    val second = IOException("wrapped", first)
    first.initCause(second)
    assertFalse(isAccessDenied(first))
  }
}
