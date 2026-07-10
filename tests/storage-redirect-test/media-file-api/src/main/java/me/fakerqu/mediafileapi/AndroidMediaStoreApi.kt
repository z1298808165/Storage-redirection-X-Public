package me.fakerqu.mediafileapi

import android.app.RecoverableSecurityException
import android.content.ContentResolver
import android.content.ContentValues
import android.content.Context
import android.database.Cursor
import android.graphics.Bitmap
import android.net.Uri
import android.os.Bundle
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.provider.MediaStore
import android.util.Size
import android.webkit.MimeTypeMap
import androidx.core.database.getBlobOrNull
import androidx.core.database.getFloatOrNull
import androidx.core.database.getIntOrNull
import androidx.core.database.getStringOrNull
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.io.InputStream

class AndroidMediaStoreApi(private val context: Context) : MediaStoreApi {
  override fun getMedia(
      mediaType: MediaStoreApi.MediaType,
      volumeType: MediaStoreApi.VolumeType,
      projection: Array<String>,
      limit: Int,
  ): List<List<MediaStoreApi.MediaColumnItem>> {
    val uri = resolveCollectionUri(mediaType, volumeType)

    val queryArgs =
        if (limit > 0) {
          Bundle().apply { putInt(ContentResolver.QUERY_ARG_LIMIT, limit) }
        } else {
          null
        }

    return context.contentResolver.query(uri, projection, queryArgs, null)?.use {
      it.moveToPosition(-1)
      val resultList = mutableListOf<List<MediaStoreApi.MediaColumnItem>>()
      while (it.moveToNext() && (limit <= 0 || resultList.size < limit)) {
        resultList.add(it.mapToColumnListRow())
      }
      resultList
    } ?: emptyList()
  }

  override fun loadThumbnail(uri: Uri, size: Size): Bitmap? {
    return try {
      context.contentResolver.loadThumbnail(uri, size, null)
    } catch (e: Exception) {
      null
    }
  }

  override fun readMedia(uri: Uri): InputStream? {
    openMediaReadDescriptor(uri)?.let { fd ->
      return ParcelFileDescriptor.AutoCloseInputStream(fd)
    }
    try {
      context.contentResolver.openInputStream(uri)?.let {
        return it
      }
    } catch (_: Exception) {}
    return resolveMediaDataPath(uri)?.let { path ->
      try {
        FileInputStream(File(path))
      } catch (_: Exception) {
        null
      }
    }
  }

  override fun writeMedia(uri: Uri, content: ByteArray): Boolean {
    openMediaWriteDescriptor(uri)?.use { fd ->
      ParcelFileDescriptor.AutoCloseOutputStream(fd).use {
        it.write(content)
        it.flush()
      }
      return true
    }
    try {
      context.contentResolver.openOutputStream(uri, "wt")?.use {
        it.write(content)
        it.flush()
        return true
      }
    } catch (_: Exception) {}
    val path = resolveMediaDataPath(uri) ?: return false
    return try {
      FileOutputStream(File(path), false).use {
        it.write(content)
        it.flush()
      }
      true
    } catch (_: Exception) {
      false
    }
  }

  override fun createMedia(
      mediaType: MediaStoreApi.MediaType,
      volumeType: MediaStoreApi.VolumeType,
      fileName: String,
      content: ByteArray,
      relativePath: String?,
      keepPending: Boolean,
  ): Uri? {
    if (fileName.isBlank() || content.isEmpty()) return null
    val collectionUri = resolveCollectionUri(mediaType, volumeType)
    val mimeType = guessMimeType(fileName, mediaType)
    val targetRelativePath = normalizeRelativePath(relativePath) ?: resolveRelativePath(mediaType)
    deleteExistingMediaRows(collectionUri, targetRelativePath, fileName)
    val pendingValues =
        ContentValues().apply {
          put(MediaStore.MediaColumns.DISPLAY_NAME, fileName)
          put(MediaStore.MediaColumns.MIME_TYPE, mimeType)
          put(MediaStore.MediaColumns.RELATIVE_PATH, targetRelativePath)
          put(MediaStore.MediaColumns.IS_PENDING, 1)
          if (mediaType == MediaStoreApi.MediaType.FILE) {
            put(
                MediaStore.Files.FileColumns.MEDIA_TYPE,
                MediaStore.Files.FileColumns.MEDIA_TYPE_NONE,
            )
          }
        }
    val uri =
        try {
          context.contentResolver.insert(collectionUri, pendingValues)
        } catch (_: Exception) {
          null
        } ?: return null
    val written =
        try {
          context.contentResolver.openFileDescriptor(uri, "w")?.use { fd ->
            ParcelFileDescriptor.AutoCloseOutputStream(fd).use { stream ->
              stream.write(content)
              stream.flush()
            }
            true
          } ?: false
        } catch (_: Exception) {
          false
        }
    if (!written) {
      try {
        context.contentResolver.delete(uri, null, null)
      } catch (_: Exception) {}
      return null
    }
    if (!keepPending) {
      val publishedValues =
          ContentValues().apply {
            put(MediaStore.MediaColumns.IS_PENDING, 0)
            put(MediaStore.MediaColumns.SIZE, content.size.toLong())
          }
      return try {
        context.contentResolver.update(uri, publishedValues, null, null)
        uri
      } catch (_: Exception) {
        try {
          context.contentResolver.delete(uri, null, null)
        } catch (_: Exception) {}
        deleteExistingMediaRows(collectionUri, targetRelativePath, fileName)
        null
      }
    }
    // Keep the row pending so the owning app can immediately re-open the URI
    // for read/write smoke tests before the media scanner re-indexes it.
    return uri
  }

  override fun createMediaWithRelativeData(
      mediaType: MediaStoreApi.MediaType,
      volumeType: MediaStoreApi.VolumeType,
      relativeDataPath: String,
      content: ByteArray,
      keepPending: Boolean,
  ): Uri? {
    val normalizedData = normalizeRelativeDataPath(relativeDataPath) ?: return null
    if (content.isEmpty()) return null
    val collectionUri = resolveCollectionUri(mediaType, volumeType)
    val fileName = normalizedData.substringAfterLast('/')
    if (fileName.isBlank()) return null
    val relativePath =
        normalizedData
            .substringBeforeLast('/', missingDelimiterValue = "")
            .takeIf { it.isNotBlank() }
            ?.let { "$it/" } ?: return null
    deleteExistingMediaRows(collectionUri, relativePath, fileName)
    val values =
        ContentValues().apply {
          put(MediaStore.MediaColumns.DISPLAY_NAME, fileName)
          put(MediaStore.MediaColumns.MIME_TYPE, guessMimeType(fileName, mediaType))
          put(MediaStore.MediaColumns.DATA, normalizedData)
          put(MediaStore.MediaColumns.IS_PENDING, 1)
          if (mediaType == MediaStoreApi.MediaType.FILE) {
            put(
                MediaStore.Files.FileColumns.MEDIA_TYPE,
                MediaStore.Files.FileColumns.MEDIA_TYPE_NONE,
            )
          }
        }
    val uri =
        try {
          context.contentResolver.insert(collectionUri, values)
        } catch (_: Exception) {
          null
        } ?: return null
    val written =
        try {
          context.contentResolver.openOutputStream(uri, "w")?.use { stream ->
            stream.write(content)
            stream.flush()
            true
          } ?: false
        } catch (_: Exception) {
          false
        }
    if (!written) {
      try {
        context.contentResolver.delete(uri, null, null)
      } catch (_: Exception) {}
      return null
    }
    if (!keepPending) {
      val publishedValues =
          ContentValues().apply {
            put(MediaStore.MediaColumns.IS_PENDING, 0)
            put(MediaStore.MediaColumns.SIZE, content.size.toLong())
          }
      return try {
        context.contentResolver.update(uri, publishedValues, null, null)
        uri
      } catch (_: Exception) {
        try {
          context.contentResolver.delete(uri, null, null)
        } catch (_: Exception) {}
        deleteExistingMediaRows(collectionUri, relativePath, fileName)
        null
      }
    }
    return uri
  }

  private fun deleteExistingMediaRows(collectionUri: Uri, relativePath: String, fileName: String) {
    val normalizedRelative = relativePath.trim('/')
    val publicSuffix = "/$normalizedRelative/$fileName"
    val sandboxSuffix = "/Android/data/${context.packageName}/sdcard$publicSuffix"
    try {
      context.contentResolver.delete(
          collectionUri,
          "${MediaStore.MediaColumns.DISPLAY_NAME}=? AND " +
              "(${MediaStore.MediaColumns.RELATIVE_PATH}=? OR " +
              "${MediaStore.MediaColumns.DATA} LIKE ? OR " +
              "${MediaStore.MediaColumns.DATA} LIKE ?)",
          arrayOf(
              fileName,
              relativePath,
              "%$publicSuffix",
              "%$sandboxSuffix",
          ),
      )
    } catch (_: Exception) {}
  }

  private fun openMediaReadDescriptor(uri: Uri): ParcelFileDescriptor? =
      openMediaDescriptor(uri, listOf("r", "rw"))

  private fun openMediaWriteDescriptor(uri: Uri): ParcelFileDescriptor? =
      openMediaDescriptor(uri, listOf("rwt", "wt", "w"))

  private fun openMediaDescriptor(uri: Uri, modes: List<String>): ParcelFileDescriptor? {
    for (mode in modes) {
      try {
        context.contentResolver.openFileDescriptor(uri, mode)?.let {
          return it
        }
      } catch (_: Exception) {}
    }
    return null
  }

  private fun resolveCollectionUri(
      mediaType: MediaStoreApi.MediaType,
      volumeType: MediaStoreApi.VolumeType,
  ): Uri =
      when (mediaType) {
        MediaStoreApi.MediaType.IMAGE ->
            when (volumeType) {
              MediaStoreApi.VolumeType.EXTERNAL -> MediaStore.Images.Media.EXTERNAL_CONTENT_URI
              MediaStoreApi.VolumeType.INTERNAL -> MediaStore.Images.Media.INTERNAL_CONTENT_URI
            }

        MediaStoreApi.MediaType.AUDIO ->
            when (volumeType) {
              MediaStoreApi.VolumeType.EXTERNAL -> MediaStore.Audio.Media.EXTERNAL_CONTENT_URI
              MediaStoreApi.VolumeType.INTERNAL -> MediaStore.Audio.Media.INTERNAL_CONTENT_URI
            }

        MediaStoreApi.MediaType.VIDEO ->
            when (volumeType) {
              MediaStoreApi.VolumeType.EXTERNAL -> MediaStore.Video.Media.EXTERNAL_CONTENT_URI
              MediaStoreApi.VolumeType.INTERNAL -> MediaStore.Video.Media.INTERNAL_CONTENT_URI
            }

        MediaStoreApi.MediaType.FILE ->
            when (volumeType) {
              MediaStoreApi.VolumeType.EXTERNAL ->
                  MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)

              MediaStoreApi.VolumeType.INTERNAL ->
                  MediaStore.Files.getContentUri(MediaStore.VOLUME_INTERNAL)
            }

        MediaStoreApi.MediaType.DOWNLOAD ->
            when (volumeType) {
              MediaStoreApi.VolumeType.EXTERNAL -> MediaStore.Downloads.EXTERNAL_CONTENT_URI
              MediaStoreApi.VolumeType.INTERNAL -> MediaStore.Downloads.INTERNAL_CONTENT_URI
            }
      }

  private fun resolveRelativePath(mediaType: MediaStoreApi.MediaType): String =
      when (mediaType) {
        MediaStoreApi.MediaType.IMAGE -> "${Environment.DIRECTORY_PICTURES}/"
        MediaStoreApi.MediaType.VIDEO -> "${Environment.DIRECTORY_MOVIES}/"
        MediaStoreApi.MediaType.AUDIO -> "${Environment.DIRECTORY_MUSIC}/"
        MediaStoreApi.MediaType.FILE -> "${Environment.DIRECTORY_DOCUMENTS}/"
        MediaStoreApi.MediaType.DOWNLOAD -> "${Environment.DIRECTORY_DOWNLOADS}/"
      }

  private fun resolveMediaDataPath(uri: Uri): String? {
    val itemId = uri.lastPathSegment ?: return null
    val queryArgs =
        Bundle().apply {
          putInt(MediaStore.QUERY_ARG_MATCH_PENDING, MediaStore.MATCH_INCLUDE)
          putString(
              ContentResolver.QUERY_ARG_SQL_SELECTION,
              "_id=?",
          )
          putStringArray(
              ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS,
              arrayOf(itemId),
          )
        }
    val collectionUri = uri.toCollectionUri() ?: uri
    return try {
      context.contentResolver
          .query(
              collectionUri,
              arrayOf(MediaStore.MediaColumns.DATA),
              queryArgs,
              null,
          )
          ?.use { cursor ->
            if (cursor.moveToFirst()) {
              cursor.getStringOrNull(0)
            } else {
              null
            }
          }
    } catch (_: Exception) {
      null
    }
  }

  private fun Uri.toCollectionUri(): Uri? {
    val segments = pathSegments
    if (segments.size < 2) return null
    val collectionSegments = segments.dropLast(1)
    return buildUpon()
        .path("/${collectionSegments.joinToString("/")}")
        .query(null)
        .fragment(null)
        .build()
  }

  private fun normalizeRelativePath(relativePath: String?): String? {
    val cleaned =
        relativePath
            ?.replace('\\', '/')
            ?.split('/')
            ?.filter { it.isNotBlank() && it != "." && it != ".." }
            ?.joinToString("/")
            ?.trim()
            .orEmpty()
    return cleaned.takeIf { it.isNotBlank() }?.let { "$it/" }
  }

  private fun normalizeRelativeDataPath(path: String?): String? {
    val cleaned =
        path
            ?.replace('\\', '/')
            ?.split('/')
            ?.filter { it.isNotBlank() && it != "." && it != ".." }
            ?.joinToString("/")
            ?.trim()
            .orEmpty()
    return cleaned.takeIf { it.isNotBlank() && !it.startsWith('/') }
  }

  private fun guessMimeType(fileName: String, mediaType: MediaStoreApi.MediaType): String {
    val extension = fileName.substringAfterLast('.', "").lowercase()
    if (extension.isNotEmpty()) {
      MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension)?.let {
        return it
      }
    }
    return when (mediaType) {
      MediaStoreApi.MediaType.IMAGE -> "image/jpeg"
      MediaStoreApi.MediaType.VIDEO -> "video/mp4"
      MediaStoreApi.MediaType.AUDIO -> "audio/mpeg"
      MediaStoreApi.MediaType.FILE -> "application/octet-stream"
      MediaStoreApi.MediaType.DOWNLOAD -> "application/octet-stream"
    }
  }

  override fun deleteMedia(uri: Uri): Boolean {
    if (uri == Uri.EMPTY) return false
    return try {
      context.contentResolver.delete(uri, null, null) > 0
    } catch (_: RecoverableSecurityException) {
      false
    } catch (_: Exception) {
      false
    }
  }

  private fun Cursor.mapToColumnListRow(): List<MediaStoreApi.MediaColumnItem> {
    return columnNames.mapIndexedNotNull { index, string ->
      when (getType(index)) {
        Cursor.FIELD_TYPE_BLOB ->
            MediaStoreApi.MediaColumnItem(
                string,
                getBlobOrNull(index),
                MediaStoreApi.ColumnType.BLOB,
            )

        Cursor.FIELD_TYPE_FLOAT ->
            MediaStoreApi.MediaColumnItem(
                string,
                getFloatOrNull(index),
                MediaStoreApi.ColumnType.FLOAT,
            )

        Cursor.FIELD_TYPE_STRING ->
            MediaStoreApi.MediaColumnItem(
                string,
                getStringOrNull(index),
                MediaStoreApi.ColumnType.STRING,
            )

        Cursor.FIELD_TYPE_INTEGER ->
            MediaStoreApi.MediaColumnItem(
                string,
                getIntOrNull(index),
                MediaStoreApi.ColumnType.INT,
            )

        Cursor.FIELD_TYPE_NULL ->
            MediaStoreApi.MediaColumnItem(string, null, MediaStoreApi.ColumnType.NULL)

        else -> null
      }
    }
  }
}
