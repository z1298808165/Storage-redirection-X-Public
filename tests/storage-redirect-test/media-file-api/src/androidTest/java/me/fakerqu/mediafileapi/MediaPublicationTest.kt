package me.fakerqu.mediafileapi

import android.content.ContentProvider
import android.content.ContentResolver
import android.content.ContentValues
import android.content.ContextWrapper
import android.content.pm.ProviderInfo
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class MediaPublicationTest {
  @Test
  fun emptyCursorAndMissingCursorStayDistinct() {
    withProvider { provider, api ->
      assertEquals(
          emptyList<List<MediaStoreApi.MediaColumnItem>>(),
          api.getMedia(
              MediaStoreApi.MediaType.IMAGE,
              MediaStoreApi.VolumeType.EXTERNAL,
              emptyArray(),
          ),
      )
      provider.missingCursor = true
      assertThrows(IOException::class.java) {
        api.getMedia(MediaStoreApi.MediaType.IMAGE, MediaStoreApi.VolumeType.EXTERNAL, emptyArray())
      }
    }
  }

  @Test
  fun publicationRequiresExactlyOneUpdatedRow() {
    for (relativeData in listOf(false, true)) {
      for (updatedRows in listOf(0, 2, -1)) {
        withProvider { provider, api ->
          provider.updatedRows = updatedRows
          assertThrows(IllegalStateException::class.java) { create(api, relativeData) }
          assertEquals(1, provider.publications)
          assertEquals(1, provider.deletedItems)
        }
      }
    }
  }

  @Test
  fun successfulPublicationAndPendingRowsStayDistinct() {
    for (relativeData in listOf(false, true)) {
      for (keepPending in listOf(false, true)) {
        withProvider { provider, api ->
          assertEquals(provider.item, create(api, relativeData, keepPending))
          assertEquals(if (keepPending) 0 else 1, provider.publications)
          assertEquals(0, provider.deletedItems)
          assertEquals("payload", provider.file.readText())
        }
      }
    }
  }

  private fun create(
      api: AndroidMediaStoreApi,
      relativeData: Boolean,
      keepPending: Boolean = false,
  ): Uri? =
      if (relativeData) {
        api.createMediaWithRelativeData(
            MediaStoreApi.MediaType.IMAGE,
            MediaStoreApi.VolumeType.EXTERNAL,
            "Pictures/test.jpg",
            "payload".toByteArray(),
            keepPending,
        )
      } else {
        api.createMedia(
            MediaStoreApi.MediaType.IMAGE,
            MediaStoreApi.VolumeType.EXTERNAL,
            "test.jpg",
            "payload".toByteArray(),
            "Pictures/",
            keepPending,
        )
      }

  private fun withProvider(block: (PublicationProvider, AndroidMediaStoreApi) -> Unit) {
    val context = InstrumentationRegistry.getInstrumentation().targetContext
    val file = File.createTempFile("publication", ".jpg", context.cacheDir)
    try {
      val provider = PublicationProvider(file)
      provider.attachInfo(context, ProviderInfo().apply { authority = "media" })
      val resolver = ContentResolver.wrap(provider)
      val wrapped =
          object : ContextWrapper(context) {
            override fun getContentResolver(): ContentResolver = resolver
          }
      block(provider, AndroidMediaStoreApi(wrapped))
    } finally {
      check(file.delete())
    }
  }

  private class PublicationProvider(val file: File) : ContentProvider() {
    val item: Uri = Uri.parse("content://media/external/images/media/1")
    var updatedRows = 1
    var publications = 0
    var deletedItems = 0
    var missingCursor = false

    override fun onCreate(): Boolean = true

    override fun getType(uri: Uri): String = "image/jpeg"

    override fun insert(uri: Uri, values: ContentValues?): Uri = item

    override fun query(
        uri: Uri,
        projection: Array<out String>?,
        selection: String?,
        selectionArgs: Array<out String>?,
        sortOrder: String?,
    ): Cursor? = if (missingCursor) null else MatrixCursor(projection ?: emptyArray())

    override fun update(
        uri: Uri,
        values: ContentValues?,
        selection: String?,
        selectionArgs: Array<out String>?,
    ): Int {
      publications++
      return updatedRows
    }

    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?): Int {
      if (uri == item) deletedItems++
      return 1
    }

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor =
        ParcelFileDescriptor.open(
            file,
            ParcelFileDescriptor.MODE_READ_WRITE or ParcelFileDescriptor.MODE_TRUNCATE,
        )
  }
}
