package me.fakerqu.test.storageredirect.ui.screen

import android.database.Cursor
import android.provider.MediaStore
import android.util.Log
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.state.ToggleableState
import androidx.core.database.getBlobOrNull
import androidx.core.database.getFloatOrNull
import androidx.core.database.getIntOrNull
import androidx.core.database.getLongOrNull
import kotlin.io.encoding.Base64
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import top.yukonga.miuix.kmp.basic.BasicComponent
import top.yukonga.miuix.kmp.basic.Checkbox
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton

@Composable
fun MediaStoreScreen(viewModel: MediaAccessViewModel) {
  val allMedias = remember { mutableStateMapOf<Long, List<String>>() }

  Scaffold() { paddingValues ->
    val coroutineScope = rememberCoroutineScope { Dispatchers.IO }
    var includeAllRow by remember { mutableStateOf(false) }
    LazyColumn(modifier = Modifier.padding(paddingValues)) {
      item {
        Row() {
          Checkbox(
              if (includeAllRow) ToggleableState.On else ToggleableState.Off,
              onClick = { includeAllRow = !includeAllRow },
          )
          Text("是否包含所有列（关闭后可能会查询到更多）")
        }
      }
      item {
        TextButton(
            "获取MediaStore",
            onClick = {
              coroutineScope.launch {
                try {
                  viewModel.getAllMediaByMediaStore(includeAllRow)?.use { cursor ->
                    cursor.moveToPosition(-1)
                    allMedias.clear()
                    while (cursor.moveToNext()) {
                      cursor.getId()?.let { allMedias[it] = cursor.getRowString() }
                    }
                  }
                } catch (e: Exception) {
                  Log.e("SRT", "failed get media by mediastore", e)
                }
              }
            },
        )
      }
      items(allMedias.entries.toList()) { (id, item) ->
        BasicComponent(title = item.joinToString { it })
        Image(viewModel.loadImageThumbnail(id).asImageBitmap(), contentDescription = "")
      }
    }
  }
}

private fun Cursor.getId(): Long? {
  return getLongOrNull(getColumnIndex(MediaStore.Images.ImageColumns._ID))
}

private fun Cursor.getRowString(): List<String> {
  return columnNames.mapIndexed { index, string ->
    "$string:${
            when (getType(index)) {
                Cursor.FIELD_TYPE_STRING -> "s:${getString(index)}"
                Cursor.FIELD_TYPE_NULL -> "null:Null"
                Cursor.FIELD_TYPE_INTEGER -> "i:${getIntOrNull(index)?.toString() ?: "Null"}"
                Cursor.FIELD_TYPE_FLOAT -> "f:${getFloatOrNull(index)?.toString() ?: "Null"}"
                Cursor.FIELD_TYPE_BLOB -> "blob:${getBlobOrNull(index)?.let { Base64.encode(it) } ?: "Null"}"
                else -> "UnknownType"
            }
        }"
  }
}
