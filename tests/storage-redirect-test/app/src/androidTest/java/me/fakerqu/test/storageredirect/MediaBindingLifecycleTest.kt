package me.fakerqu.test.storageredirect

import android.content.ContentProviderOperation
import android.content.ContentUris
import android.content.ContentValues
import android.net.Uri
import android.provider.MediaStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.util.UUID
import org.junit.After
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class MediaBindingLifecycleTest {
  private val resolver = InstrumentationRegistry.getInstrumentation().targetContext.contentResolver
  private val collection = MediaStore.Files.getContentUri("external_primary")
  private val prefix = "srx_binding_${UUID.randomUUID()}_"
  private val cleanup = mutableListOf<Uri>()

  private fun values(label: String) =
      ContentValues().apply {
        put("_display_name", "$prefix$label.txt")
        put("relative_path", "Documents/SrtMediaRoutingProbe/")
        put("mime_type", "text/plain")
        put("is_pending", 1)
      }

  private fun find(label: String): List<Uri> = buildList {
    val queryArgs =
        android.os.Bundle().apply {
          putInt(MediaStore.QUERY_ARG_MATCH_PENDING, MediaStore.MATCH_INCLUDE)
          putString(android.content.ContentResolver.QUERY_ARG_SQL_SELECTION, "_display_name=?")
          putStringArray(
              android.content.ContentResolver.QUERY_ARG_SQL_SELECTION_ARGS,
              arrayOf("$prefix$label.txt"),
          )
        }
    resolver.query(collection, arrayOf("_id"), queryArgs, null)!!.use {
      while (it.moveToNext()) add(ContentUris.withAppendedId(collection, it.getLong(0)))
    }
  }

  private fun publishAndRead(uri: Uri, label: String) {
    val payload = "private-$label-$prefix".toByteArray()
    resolver.openOutputStream(uri, "w")!!.use { it.write(payload) }
    assertEquals(
        1,
        resolver.update(uri, ContentValues().apply { put("is_pending", 0) }, null, null),
    )
    assertArrayEquals(payload, resolver.openInputStream(uri)!!.use { it.readBytes() })
    resolver.query(uri, arrayOf("_size", "is_pending"), null, null, null)!!.use {
      assertTrue(it.moveToFirst())
      assertEquals(payload.size.toLong(), it.getLong(0))
      assertEquals(0, it.getInt(1))
    }
  }

  @After
  fun clean() {
    for (uri in cleanup.distinct()) resolver.delete(uri, null, null)
  }

  @Test
  fun bulkInsertTracksEachFile() {
    val labels = listOf("first", "second")
    try {
      assertEquals(2, resolver.bulkInsert(collection, labels.map(::values).toTypedArray()))
      for (label in labels) publishAndRead(find(label).single(), label)
    } finally {
      labels.forEach { cleanup.addAll(find(it)) }
    }
  }

  @Test
  fun allowedBatchFailureKeepsOnlySuccessfulItems() {
    val insert = { label: String ->
      ContentProviderOperation.newInsert(collection).withValues(values(label)).build()
    }
    val failure =
        ContentProviderOperation.newAssertQuery(collection)
            .withSelection("_display_name=?", arrayOf("${prefix}missing.txt"))
            .withExpectedCount(1)
            .withExceptionAllowed(true)
            .build()
    val result =
        resolver.applyBatch("media", arrayListOf(insert("before"), failure, insert("after")))
    for (item in result) item.uri?.let { cleanup.add(it) }
    assertNotNull(result[1].exception)
    publishAndRead(requireNotNull(result[0].uri), "before")
    publishAndRead(requireNotNull(result[2].uri), "after")
  }

  @Test
  fun rejectedBindingLeavesNoItemOrNotification() {
    val args = InstrumentationRegistry.getArguments()
    val rejected = args.getString("srxRejectedName")
    org.junit.Assume.assumeTrue(rejected != null)
    requireNotNull(rejected)
    val observerThread = android.os.HandlerThread("media-binding-observer").apply { start() }
    val changed = java.util.concurrent.ConcurrentLinkedQueue<Uri>()
    val observer =
        object : android.database.ContentObserver(android.os.Handler(observerThread.looper)) {
          override fun onChange(selfChange: Boolean, uris: Collection<Uri>, flags: Int) {
            changed.addAll(uris)
          }

          override fun onChange(selfChange: Boolean, uri: Uri?) {
            if (uri != null) changed.add(uri)
          }
        }
    resolver.registerContentObserver(collection, true, observer)
    try {
      val entries =
          listOf("first", "rejected", "last").map { label ->
            val fields = values(label)
            if (label == "rejected") fields.put("_display_name", rejected)
            ContentProviderOperation.newInsert(collection)
                .withValues(fields)
                .withExceptionAllowed(label == "rejected")
                .build()
          }
      val results = resolver.applyBatch("media", ArrayList(entries))
      for (result in results) result.uri?.let { cleanup.add(it) }
      assertNotNull(results[1].exception)
      assertNull(results[1].uri)
      resolver.query(collection, arrayOf("_id"), "_display_name=?", arrayOf(rejected), null)!!.use {
        assertEquals(0, it.count)
      }
      publishAndRead(requireNotNull(results[0].uri), "first")
      publishAndRead(requireNotNull(results[2].uri), "last")
      val drained = java.util.concurrent.CountDownLatch(1)
      android.os.Handler(observerThread.looper).post { drained.countDown() }
      assertTrue(drained.await(2, java.util.concurrent.TimeUnit.SECONDS))
      val surviving = cleanup.map { it.lastPathSegment }.toSet()
      val missing =
          changed.filter {
            it.pathSegments.dropLast(1) == collection.pathSegments &&
                it.lastPathSegment?.toLongOrNull() != null &&
                it.lastPathSegment !in surviving
          }
      assertTrue("Unexpected file notifications $missing", missing.isEmpty())
    } finally {
      resolver.unregisterContentObserver(observer)
      observerThread.quitSafely()
      observerThread.join(2000)
      for (label in listOf("first", "last")) cleanup.addAll(find(label))
    }
  }

  @Test
  fun failedBatchRollsBackPrecedingInsert() {
    val insert =
        ContentProviderOperation.newInsert(collection).withValues(values("rolled_back")).build()
    val failure =
        ContentProviderOperation.newAssertQuery(collection)
            .withSelection("_display_name=?", arrayOf("${prefix}missing.txt"))
            .withExpectedCount(1)
            .build()
    try {
      assertThrows(android.content.OperationApplicationException::class.java) {
        resolver.applyBatch("media", arrayListOf(insert, failure))
      }
      assertTrue(find("rolled_back").isEmpty())
    } finally {
      cleanup.addAll(find("rolled_back"))
    }
  }
}
