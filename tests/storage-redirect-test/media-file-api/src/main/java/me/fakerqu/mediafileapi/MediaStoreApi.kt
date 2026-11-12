package me.fakerqu.mediafileapi

import android.graphics.Bitmap
import android.net.Uri
import android.util.Size
import java.io.InputStream

interface MediaStoreApi {
  enum class MediaType {
    IMAGE,
    VIDEO,
    AUDIO,
    FILE,
    DOWNLOAD,
  }

  enum class VolumeType {
    EXTERNAL,
    INTERNAL,
  }

  enum class ColumnType {
    STRING,
    INT,
    FLOAT,
    NULL,
    BLOB,
  }

  data class MediaColumnItem(val columnName: String, val value: Any?, val columnType: ColumnType) {
    fun getValueAsInt(): Int? = if (ColumnType.INT == columnType) value as Int? else null

    fun getValueAsFloat(): Float? = if (ColumnType.FLOAT == columnType) value as Float? else null

    fun getValueAsString(): String? =
        if (ColumnType.STRING == columnType) value as String? else null

    fun getValueAsBlob(): ByteArray? =
        if (ColumnType.BLOB == columnType) value as ByteArray? else null

    fun isNull(): Boolean = ColumnType.NULL == columnType || value == null
  }

  /** 查询结果可为空列表；提供器未返回游标或查询失败时向调用方抛出异常。 */
  fun getMedia(
      mediaType: MediaType,
      volumeType: VolumeType,
      projection: Array<String>,
      limit: Int = 0,
  ): List<List<MediaColumnItem>>

  fun loadThumbnail(uri: Uri, size: Size): Bitmap?

  fun readMedia(uri: Uri): InputStream?

  fun writeMedia(uri: Uri, content: ByteArray): Boolean

  /**
   * 在 MediaStore 中创建新媒体条目并写入 [content]。
   *
   * 提供器和 I/O 异常向调用方传播，供其区分权限拒绝与其它错误。
   *
   * @return 成功时返回新条目的 content URI；参数无效或提供器返回空结果时返回 null。
   * @throws IllegalStateException 发布条目时未恰好更新一行。
   */
  fun createMedia(
      mediaType: MediaType,
      volumeType: VolumeType,
      fileName: String,
      content: ByteArray,
      relativePath: String? = null,
      keepPending: Boolean = false,
  ): Uri?

  /** 以相对 DATA 路径创建媒体；返回值和异常语义见 [createMedia]。 */
  fun createMediaWithRelativeData(
      mediaType: MediaType,
      volumeType: VolumeType,
      relativeDataPath: String,
      content: ByteArray,
      keepPending: Boolean = false,
  ): Uri?

  /**
   * 删除 MediaStore 中的指定条目（含底层文件）。
   *
   * @param uri 条目的 content URI（如 [createMedia] 的返回值，或由集合 URI + `_ID` 拼接）
   * @return 成功删除至少一行时返回 true；无权限、条目不存在或需用户授权时返回 false
   */
  fun deleteMedia(uri: Uri): Boolean
}
