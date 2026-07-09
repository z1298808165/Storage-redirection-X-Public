package org.srx.manager.data

import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.util.zip.ZipEntry
import java.util.zip.ZipInputStream
import java.util.zip.ZipOutputStream

internal object BackupArchiveCodec {
  private const val BackupZipEntryName = "backup.json"

  fun encodeZip(text: String): ByteArray {
    val rawBytes = text.toByteArray(Charsets.UTF_8)
    if (rawBytes.size > BackupMaxBytes) throw IllegalArgumentException("备份文件过大")
    return ByteArrayOutputStream().use { output ->
      ZipOutputStream(output).use { zip ->
        zip.putNextEntry(ZipEntry(BackupZipEntryName))
        zip.write(rawBytes)
        zip.closeEntry()
      }
      output.toByteArray()
    }
  }

  fun decode(bytes: ByteArray): String {
    if (bytes.size > BackupMaxBytes) throw IllegalArgumentException("备份文件过大")
    val isZip = bytes.size >= 4 && bytes[0] == 0x50.toByte() && bytes[1] == 0x4b.toByte()
    if (!isZip) return bytes.toString(Charsets.UTF_8)

    ZipInputStream(bytes.inputStream()).use { zip ->
      while (true) {
        val entry = zip.nextEntry ?: break
        if (!entry.isDirectory && entry.name == BackupZipEntryName) {
          val text = zip.readEntryTextBounded()
          zip.closeEntry()
          return text
        }
        zip.closeEntry()
      }
    }
    throw IllegalArgumentException("备份包缺少 $BackupZipEntryName")
  }

  fun readBytesBounded(input: InputStream): ByteArray {
    val output = ByteArrayOutputStream()
    val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
    var total = 0
    while (true) {
      val count = input.read(buffer)
      if (count < 0) break
      total += count
      if (total > BackupMaxBytes) {
        throw IllegalArgumentException("备份文件过大")
      }
      output.write(buffer, 0, count)
    }
    return output.toByteArray()
  }

  private fun ZipInputStream.readEntryTextBounded(): String {
    return readBytesBounded(this).toString(Charsets.UTF_8)
  }
}
