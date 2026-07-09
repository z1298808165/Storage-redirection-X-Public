package me.fakerqu.mediafileapi

import android.system.Os
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.io.RandomAccessFile

class AndroidFileApi : FileApi {
  override fun getDirFilesRecursive(dir: String): List<File> {
    val dirFile = File(dir)
    return if (dirFile.exists() && dirFile.isDirectory) {
      dirFile.walk().map { it }.toList()
    } else {
      emptyList()
    }
  }

  override fun readFile(file: String): InputStream = FileInputStream(file)

  override fun writeFile(file: String): OutputStream = FileOutputStream(file)

  override fun createFile(path: String): File {
    if (path.endsWith("/")) {
      File(path).mkdirs()
    } else {
      File(path).createNewFile()
    }
    return File(path)
  }

  override fun deleteFile(path: String): Boolean {
    val file = File(path)
    return if (file.isFile) {
      file.delete()
    } else {
      file.deleteRecursively()
    }
  }

  override fun mkdir(path: String): Boolean {
    val file = File(path)
    return file.mkdirs() || file.isDirectory
  }

  override fun renameFile(fromPath: String, toPath: String): Boolean {
    return File(fromPath).renameTo(File(toPath))
  }

  override fun statFile(path: String): FileStatInfo {
    val stat = Os.stat(path)
    return FileStatInfo(
        size = stat.st_size,
        mode = stat.st_mode,
        uid = stat.st_uid,
        gid = stat.st_gid,
        modifiedSeconds = stat.st_mtime,
    )
  }

  override fun accessFile(path: String, mode: Int): Boolean = Os.access(path, mode)

  override fun readLink(path: String): String = Os.readlink(path)

  override fun truncateFile(path: String, length: Long) {
    RandomAccessFile(path, "rw").use { it.setLength(length) }
  }

  override fun ftruncateFile(path: String, length: Long) {
    val fd = Os.open(path, android.system.OsConstants.O_RDWR, 0)
    try {
      Os.ftruncate(fd, length)
    } finally {
      Os.close(fd)
    }
  }

  override fun chmodFile(path: String, mode: Int) {
    Os.chmod(path, mode)
  }

  override fun fchmodFile(path: String, mode: Int) {
    val fd = Os.open(path, android.system.OsConstants.O_RDONLY, 0)
    try {
      Os.fchmod(fd, mode)
    } finally {
      Os.close(fd)
    }
  }

  override fun linkFile(fromPath: String, toPath: String) {
    Os.link(fromPath, toPath)
  }

  override fun symlinkFile(targetPath: String, linkPath: String) {
    Os.symlink(targetPath, linkPath)
  }
}
