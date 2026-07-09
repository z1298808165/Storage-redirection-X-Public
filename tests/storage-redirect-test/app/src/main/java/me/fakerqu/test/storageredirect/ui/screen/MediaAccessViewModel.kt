package me.fakerqu.test.storageredirect.ui.screen

import android.app.Application
import android.content.ContentUris
import android.database.Cursor
import android.graphics.Bitmap
import android.provider.MediaStore
import android.util.Size
import androidx.lifecycle.AndroidViewModel

class MediaAccessViewModel(
    application: Application,
) : AndroidViewModel(application) {
  fun getAllMediaByMediaStore(allRows: Boolean): Cursor? =
      getApplication<Application>()
          .contentResolver
          .query(
              MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
              if (allRows) {
                null
              } else {
                arrayOf(
                    MediaStore.Images.ImageColumns._ID,
                )
              },
              null,
              null,
              null,
          )

  fun loadImageThumbnail(imageId: Long): Bitmap {
    val uri = ContentUris.withAppendedId(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, imageId)
    return getApplication<Application>().contentResolver.loadThumbnail(uri, Size(200, 200), null)
  }
}
