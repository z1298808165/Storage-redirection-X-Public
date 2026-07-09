package org.srx.manager.data

import java.io.ByteArrayOutputStream
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class BackupArchiveCodecTest {
  private val json = Json {
    encodeDefaults = true
    explicitNulls = false
  }

  @Test
  fun zipArchiveRoundTripsBackupJson() {
    val text = "{\"magic\":\"storage.redirect.x.backup\"}\n"

    val bytes = BackupArchiveCodec.encodeZip(text)

    assertTrue(bytes[0] == 0x50.toByte() && bytes[1] == 0x4b.toByte())
    assertEquals(text, BackupArchiveCodec.decode(bytes))
  }

  @Test
  fun legacyJsonBytesDecodeAsText() {
    val text = "{\"magic\":\"storage.redirect.x.backup\"}\n"

    assertEquals(text, BackupArchiveCodec.decode(text.toByteArray(Charsets.UTF_8)))
  }

  @Test
  fun boundedReaderAcceptsBackupSizedInput() {
    val text = "{\"magic\":\"storage.redirect.x.backup\"}\n"

    val bytes = BackupArchiveCodec.readBytesBounded(text.byteInputStream())

    assertEquals(text, bytes.toString(Charsets.UTF_8))
  }

  @Test
  fun zippedPayloadKeepsIntegrityDigestStable() {
    val data =
        SrxConfigNormalizer.normalizeBackupData(
            BackupData(
                global = GlobalConfig(fileMonitorEnabled = true),
                apps = mapOf("org.srx.demo" to AppConfig(users = mapOf("0" to UserProfile()))),
            ),
        )
    val canonical = SrxConfigNormalizer.stableJson(json, data)
    val payload =
        BackupPayload(
            magic = "storage.redirect.x.backup",
            schema = 2,
            module = BackupModuleInfo(id = "storage.redirect.x", version = "test"),
            createdAt = "2026-01-01T00:00:00Z",
            summary = BackupSummary(appCount = 1, userCount = 1),
            integrity =
                BackupIntegrity(
                    algorithm = "SHA-256",
                    value = SrxConfigNormalizer.sha256Hex(canonical),
                ),
            data = data,
        )

    val decoded =
        BackupArchiveCodec.decode(BackupArchiveCodec.encodeZip(json.encodeToString(payload) + "\n"))
    val decodedPayload = json.decodeFromString<BackupPayload>(decoded)

    assertTrue(
        decodedPayload.integrity.value in
            SrxConfigNormalizer.backupDigestCandidates(json, decodedPayload.data)
    )
  }

  @Test(expected = IllegalArgumentException::class)
  fun zipArchiveWithoutBackupJsonIsRejected() {
    val bytes =
        ByteArrayOutputStream().use { output ->
          ZipOutputStream(output).use { zip ->
            zip.putNextEntry(ZipEntry("other.json"))
            zip.write("{}".toByteArray(Charsets.UTF_8))
            zip.closeEntry()
          }
          output.toByteArray()
        }

    BackupArchiveCodec.decode(bytes)
  }

  @Test(expected = IllegalArgumentException::class)
  fun oversizedBackupTextIsRejectedBeforeZip() {
    BackupArchiveCodec.encodeZip("x".repeat(BackupMaxBytes + 1))
  }

  @Test(expected = IllegalArgumentException::class)
  fun oversizedInputStreamIsRejectedWhileReading() {
    BackupArchiveCodec.readBytesBounded(ByteArray(BackupMaxBytes + 1).inputStream())
  }

  @Test(expected = IllegalArgumentException::class)
  fun oversizedZippedBackupJsonIsRejectedWhileReading() {
    val bytes =
        ByteArrayOutputStream().use { output ->
          ZipOutputStream(output).use { zip ->
            zip.putNextEntry(ZipEntry("backup.json"))
            zip.write("x".repeat(BackupMaxBytes + 1).toByteArray(Charsets.UTF_8))
            zip.closeEntry()
          }
          output.toByteArray()
        }

    BackupArchiveCodec.decode(bytes)
  }
}
