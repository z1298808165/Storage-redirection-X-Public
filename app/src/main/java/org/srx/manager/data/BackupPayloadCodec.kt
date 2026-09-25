package org.srx.manager.data

import java.time.Instant
import kotlinx.serialization.SerializationException
import kotlinx.serialization.json.Json

/**
 * 备份载荷编解码边界。
 *
 * 备份文件的 magic、schema、模块标识和完整性摘要都属于同一套格式约定， 集中在这里可以避免备份格式散落到数据层多个入口后各自演化。
 */
internal object BackupPayloadCodec {
  const val Magic = "storage.redirect.x.backup"
  const val SchemaVersion = 2
  const val ModuleId = "storage.redirect.x"

  fun encode(json: Json, data: BackupData, moduleVersion: String): String {
    val canonical = SrxConfigNormalizer.stableJson(json, data)
    val payload =
        BackupPayload(
            magic = Magic,
            schema = SchemaVersion,
            module = BackupModuleInfo(id = ModuleId, version = moduleVersion),
            createdAt = Instant.now().toString(),
            summary =
                BackupSummary(
                    appCount = data.apps.size,
                    userCount = data.apps.values.sumOf { it.users.size },
                ),
            integrity =
                BackupIntegrity(
                    algorithm = "SHA-256",
                    value = SrxConfigNormalizer.sha256Hex(canonical),
                ),
            data = data,
        )
    return json.encodeToString(payload) + "\n"
  }

  fun decode(json: Json, text: String): BackupData {
    if (text.toByteArray(Charsets.UTF_8).size > BackupMaxBytes) {
      throw IllegalArgumentException("备份文件过大")
    }
    val payload =
        try {
          json.decodeFromString<BackupPayload>(text)
        } catch (_: SerializationException) {
          throw IllegalArgumentException("备份文件不是有效 JSON")
        } catch (_: IllegalArgumentException) {
          throw IllegalArgumentException("备份文件不是有效 JSON")
        }
    if (payload.magic != Magic) throw IllegalArgumentException("不是 Storage Redirect X 备份")
    if (payload.schema !in 1..SchemaVersion) throw IllegalArgumentException("备份格式版本不支持")
    if (payload.module.id != ModuleId) throw IllegalArgumentException("备份属于其它模块")
    val data = SrxConfigNormalizer.normalizeBackupData(payload.data)
    val expected = SrxConfigNormalizer.backupDigestCandidates(json, data)
    if (
        !payload.integrity.algorithm.equals("SHA-256", ignoreCase = true) ||
            payload.integrity.value !in expected
    ) {
      throw IllegalArgumentException("备份校验失败，文件可能被改动")
    }
    return data
  }
}
