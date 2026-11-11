package me.fakerqu.test.storageredirect.test

import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.RandomAccessFile
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/** 在空夹具目录中校验文件变更与打开描述符的语义，保留完成标记供 root 校验物理落点。 */
internal fun runFileMutationLifecycle(root: File) {
  check(root.isDirectory && root.list()?.isEmpty() == true) { "fixture directory must be empty" }
  val original = File(root, "original 空格.txt")
  val payload = "srx-文件-content".toByteArray(Charsets.UTF_8) + byteArrayOf(0, 1, -1)
  check(original.createNewFile()) { "create failed" }
  check(!original.createNewFile()) { "exclusive create overwrote existing file" }
  FileOutputStream(original).use { stream ->
    stream.write(payload)
    stream.fd.sync()
  }
  check(original.readBytes().contentEquals(payload)) { "write/read content mismatch" }

  FileOutputStream(original, true).use { it.write(byteArrayOf(2, 3)) }
  check(original.readBytes().contentEquals(payload + byteArrayOf(2, 3))) { "append mismatch" }
  RandomAccessFile(original, "rw").use {
    it.setLength(4)
    check(original.readBytes().contentEquals(payload.copyOf(4))) { "truncate mismatch" }
    it.setLength(8)
  }
  check(original.readBytes().contentEquals(payload.copyOf(4).copyOf(8))) {
    "zero extension mismatch"
  }

  val nested = File(root, "nested dir")
  check(nested.mkdir()) { "mkdir failed" }
  val renamed = File(nested, "renamed.txt")
  check(original.renameTo(renamed)) { "rename failed" }
  check(!original.exists() && renamed.readBytes().contentEquals(payload.copyOf(4).copyOf(8))) {
    "rename lost content or retained source"
  }
  check(!nested.delete() && renamed.isFile) { "nonempty directory was deleted" }

  val replacement = File(root, "replacement.tmp")
  FileOutputStream(replacement).use { stream ->
    stream.write(payload)
    stream.fd.sync()
  }
  RandomAccessFile(renamed, "rw").use { old ->
    Files.move(
        replacement.toPath(),
        renamed.toPath(),
        StandardCopyOption.ATOMIC_MOVE,
        StandardCopyOption.REPLACE_EXISTING,
    )
    check(!replacement.exists()) { "atomic replacement retained temporary file" }
    val originalBytes = ByteArray(8)
    old.readFully(originalBytes)
    check(originalBytes.contentEquals(payload.copyOf(4).copyOf(8))) {
      "atomic replacement changed existing descriptor"
    }
    old.setLength(3)
    old.seek(0)
    old.write(byteArrayOf(7, 8, 9))
    check(old.length() == 3L) { "detached descriptor truncate/write failed" }
    old.seek(0)
    val written = ByteArray(3)
    old.readFully(written)
    check(written.contentEquals(byteArrayOf(7, 8, 9))) { "detached descriptor read mismatch" }
  }
  check(renamed.readBytes().contentEquals(payload)) { "atomic replacement content mismatch" }

  val child = File(nested, "child/leaf.txt")
  check(child.parentFile!!.mkdir()) { "child directory create failed" }
  child.writeBytes(payload)
  check(child.readBytes().contentEquals(payload)) { "child lookup failed" }
  val nestedEntries = nested.list()?.toSet()
  check(nestedEntries == setOf(renamed.name, "child")) {
    "nested directory listing mismatch: $nestedEntries"
  }
  val sibling = File(root, "nested dirSibling")
  check(sibling.mkdir()) { "prefix sibling create failed" }
  val siblingFile = File(sibling, "leaf.txt").apply { writeBytes(byteArrayOf(4, 5)) }
  val movedDirectory = File(root, "moved dir")
  val movedChild = File(movedDirectory, "child/leaf.txt")
  val movedFile = File(movedDirectory, renamed.name)
  RandomAccessFile(child, "rw").use { opened ->
    check(nested.renameTo(movedDirectory) && !nested.exists()) {
      "populated directory rename failed"
    }
    check(movedFile.readBytes().contentEquals(payload)) { "renamed directory lost immediate child" }
    check(movedChild.readBytes().contentEquals(payload)) { "renamed directory lost descendant" }
    check(!child.exists()) { "renamed descendant retained old path" }
    val movedEntries = movedDirectory.list()?.toSet()
    check(movedEntries == setOf(renamed.name, "child")) {
      "renamed directory listing mismatch: $movedEntries"
    }
    check(!movedDirectory.renameTo(sibling)) { "rename replaced nonempty directory" }
    check(!movedDirectory.renameTo(File(movedDirectory, "child/recursive"))) {
      "directory moved into its descendant"
    }

    // 重用旧路径，避免陈旧的子孙 inode 错误命中同名新文件。
    check(child.parentFile!!.mkdirs()) { "old directory path reuse failed" }
    child.writeBytes(byteArrayOf(6))
    opened.setLength(3)
    opened.seek(0)
    opened.write(byteArrayOf(7, 8, 9))
    opened.fd.sync()
    opened.seek(0)
    val readBack = ByteArray(3)
    opened.readFully(readBack)
    check(readBack.contentEquals(byteArrayOf(7, 8, 9))) { "renamed descendant descriptor mismatch" }
    check(movedChild.readBytes().contentEquals(readBack)) {
      "renamed descendant write missed new path"
    }
    check(child.readBytes().contentEquals(byteArrayOf(6))) {
      "renamed descriptor changed reused path"
    }
    check(siblingFile.readBytes().contentEquals(byteArrayOf(4, 5))) {
      "directory rename changed prefix sibling"
    }
    check(movedFile.readBytes().contentEquals(payload)) { "failed directory rename changed source" }
  }
  check(child.delete() && child.parentFile!!.delete() && nested.delete()) {
    "reused path cleanup failed"
  }
  check(siblingFile.delete() && sibling.delete()) { "prefix sibling cleanup failed" }
  check(movedChild.delete() && movedChild.parentFile!!.delete()) { "descendant cleanup failed" }
  FileInputStream(movedFile).use { opened ->
    check(movedFile.delete() && !movedFile.exists()) { "unlink failed" }
    movedFile.writeBytes(byteArrayOf(10))
    check(opened.readBytes().contentEquals(payload)) { "unlink invalidated open descriptor" }
    check(movedFile.readBytes().contentEquals(byteArrayOf(10))) {
      "unlinked descriptor changed reused path"
    }
  }
  check(movedFile.delete()) { "recreated file cleanup failed" }
  check(movedDirectory.delete()) { "rmdir failed" }
  check(root.list()?.isEmpty() == true) { "mutation left unexpected entries" }
  File(root, "complete.txt").writeText("srx-mutation-lifecycle-v1", Charsets.UTF_8)
}
