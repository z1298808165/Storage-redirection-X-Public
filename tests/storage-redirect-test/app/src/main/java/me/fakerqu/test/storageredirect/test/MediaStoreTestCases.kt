package me.fakerqu.test.storageredirect.test

import android.content.Context
import android.net.Uri
import android.provider.MediaStore
import android.util.Size
import me.fakerqu.media_store_api.IMediaStoreApi
import me.fakerqu.media_store_api.MediaStoreApiImpl

class MediaStoreTestCases(context: Context) {

    private val api = MediaStoreApiImpl(context)
    private val volume = IMediaStoreApi.VolumeType.EXTERNAL

    fun queryImage(): TestResult =
        query(TestCase.MEDIASTORE_QUERY_IMAGE, IMediaStoreApi.MediaType.IMAGE)

    fun queryVideo(): TestResult =
        query(TestCase.MEDIASTORE_QUERY_VIDEO, IMediaStoreApi.MediaType.VIDEO)

    fun queryAudio(): TestResult =
        query(TestCase.MEDIASTORE_QUERY_AUDIO, IMediaStoreApi.MediaType.AUDIO)

    fun queryFile(): TestResult =
        query(TestCase.MEDIASTORE_QUERY_FILE, IMediaStoreApi.MediaType.FILE)

    fun queryDownload(): TestResult =
        query(TestCase.MEDIASTORE_QUERY_DOWNLOAD, IMediaStoreApi.MediaType.DOWNLOAD)

    fun queryReadOnlyImage(args: TestCaseArgs): TestResult =
        queryPath(TestCase.MEDIASTORE_QUERY_READ_ONLY_IMAGE, IMediaStoreApi.MediaType.IMAGE, args)

    fun queryImagePath(args: TestCaseArgs): TestResult =
        queryPath(TestCase.MEDIASTORE_QUERY_PATH_IMAGE, IMediaStoreApi.MediaType.IMAGE, args)

    fun queryVideoPath(args: TestCaseArgs): TestResult =
        queryPath(TestCase.MEDIASTORE_QUERY_PATH_VIDEO, IMediaStoreApi.MediaType.VIDEO, args)

    fun queryAudioPath(args: TestCaseArgs): TestResult =
        queryPath(TestCase.MEDIASTORE_QUERY_PATH_AUDIO, IMediaStoreApi.MediaType.AUDIO, args)

    fun queryFilePath(args: TestCaseArgs): TestResult =
        queryPath(TestCase.MEDIASTORE_QUERY_PATH_FILE, IMediaStoreApi.MediaType.FILE, args)

    fun queryDownloadPath(args: TestCaseArgs): TestResult =
        queryPath(TestCase.MEDIASTORE_QUERY_PATH_DOWNLOAD, IMediaStoreApi.MediaType.DOWNLOAD, args)

    fun createImage(args: TestCaseArgs): TestResult =
        create(TestCase.MEDIASTORE_CREATE_IMAGE, IMediaStoreApi.MediaType.IMAGE, args)

    fun createImageDenied(args: TestCaseArgs): TestResult =
        createDenied(TestCase.MEDIASTORE_CREATE_IMAGE_DENIED, IMediaStoreApi.MediaType.IMAGE, args)

    fun createVideo(args: TestCaseArgs): TestResult =
        create(TestCase.MEDIASTORE_CREATE_VIDEO, IMediaStoreApi.MediaType.VIDEO, args)

    fun createVideoDenied(args: TestCaseArgs): TestResult =
        createDenied(TestCase.MEDIASTORE_CREATE_VIDEO_DENIED, IMediaStoreApi.MediaType.VIDEO, args)

    fun createAudio(args: TestCaseArgs): TestResult =
        create(TestCase.MEDIASTORE_CREATE_AUDIO, IMediaStoreApi.MediaType.AUDIO, args)

    fun createAudioDenied(args: TestCaseArgs): TestResult =
        createDenied(TestCase.MEDIASTORE_CREATE_AUDIO_DENIED, IMediaStoreApi.MediaType.AUDIO, args)

    fun createFile(args: TestCaseArgs): TestResult =
        create(TestCase.MEDIASTORE_CREATE_FILE, IMediaStoreApi.MediaType.FILE, args)

    fun createFileDenied(args: TestCaseArgs): TestResult =
        createDenied(TestCase.MEDIASTORE_CREATE_FILE_DENIED, IMediaStoreApi.MediaType.FILE, args)

    fun createDownload(args: TestCaseArgs): TestResult =
        create(TestCase.MEDIASTORE_CREATE_DOWNLOAD, IMediaStoreApi.MediaType.DOWNLOAD, args)

    fun createDownloadDenied(args: TestCaseArgs): TestResult =
        createDenied(TestCase.MEDIASTORE_CREATE_DOWNLOAD_DENIED, IMediaStoreApi.MediaType.DOWNLOAD, args)

    fun readImage(args: TestCaseArgs): TestResult =
        read(TestCase.MEDIASTORE_READ_IMAGE, IMediaStoreApi.MediaType.IMAGE, args)

    fun readVideo(args: TestCaseArgs): TestResult =
        read(TestCase.MEDIASTORE_READ_VIDEO, IMediaStoreApi.MediaType.VIDEO, args)

    fun readAudio(args: TestCaseArgs): TestResult =
        read(TestCase.MEDIASTORE_READ_AUDIO, IMediaStoreApi.MediaType.AUDIO, args)

    fun readFile(args: TestCaseArgs): TestResult =
        read(TestCase.MEDIASTORE_READ_FILE, IMediaStoreApi.MediaType.FILE, args)

    fun readDownload(args: TestCaseArgs): TestResult =
        read(TestCase.MEDIASTORE_READ_DOWNLOAD, IMediaStoreApi.MediaType.DOWNLOAD, args)

    fun writeImage(args: TestCaseArgs): TestResult =
        write(TestCase.MEDIASTORE_WRITE_IMAGE, IMediaStoreApi.MediaType.IMAGE, args)

    fun writeVideo(args: TestCaseArgs): TestResult =
        write(TestCase.MEDIASTORE_WRITE_VIDEO, IMediaStoreApi.MediaType.VIDEO, args)

    fun writeAudio(args: TestCaseArgs): TestResult =
        write(TestCase.MEDIASTORE_WRITE_AUDIO, IMediaStoreApi.MediaType.AUDIO, args)

    fun writeFile(args: TestCaseArgs): TestResult =
        write(TestCase.MEDIASTORE_WRITE_FILE, IMediaStoreApi.MediaType.FILE, args)

    fun writeDownload(args: TestCaseArgs): TestResult =
        write(TestCase.MEDIASTORE_WRITE_DOWNLOAD, IMediaStoreApi.MediaType.DOWNLOAD, args)

    fun deleteImage(args: TestCaseArgs): TestResult =
        delete(TestCase.MEDIASTORE_DELETE_IMAGE, args)

    fun deleteVideo(args: TestCaseArgs): TestResult =
        delete(TestCase.MEDIASTORE_DELETE_VIDEO, args)

    fun deleteAudio(args: TestCaseArgs): TestResult =
        delete(TestCase.MEDIASTORE_DELETE_AUDIO, args)

    fun deleteFile(args: TestCaseArgs): TestResult =
        delete(TestCase.MEDIASTORE_DELETE_FILE, args)

    fun deleteDownload(args: TestCaseArgs): TestResult =
        delete(TestCase.MEDIASTORE_DELETE_DOWNLOAD, args)

    fun thumbnailImage(args: TestCaseArgs): TestResult =
        thumbnail(TestCase.MEDIASTORE_THUMBNAIL_IMAGE, args)

    fun thumbnailVideo(args: TestCaseArgs): TestResult =
        thumbnail(TestCase.MEDIASTORE_THUMBNAIL_VIDEO, args)

    private fun query(testCase: TestCase, mediaType: IMediaStoreApi.MediaType): TestResult =
        testCase.measure {
            val rows = api.getMedia(
                mediaType,
                volume,
                TestFixtures.projection(mediaType),
                limit = MAX_QUERY_ROWS,
            )
            testCase.pass(
                message = "query completed",
                metadata = mapOf(
                    "mediaType" to mediaType.name,
                    "rowCount" to rows.size.toString(),
                    "sampleRows" to rows.take(MAX_QUERY_SAMPLE_ROWS)
                        .joinToString(separator = " | ") { row ->
                            row.joinToString { item -> "${item.columnName}=${item.value}" }
                        },
                ),
            )
        }

    private fun queryPath(
        testCase: TestCase,
        mediaType: IMediaStoreApi.MediaType,
        args: TestCaseArgs,
    ): TestResult = testCase.measure {
        val expectedPath = args.expectedPath
            ?: return@measure args.missingExpectedPathResult(testCase)
        val fileName = args.fileName ?: expectedPath.substringAfterLast("/")
        if (fileName.isBlank()) {
            return@measure testCase.fail(
                message = "missing required parameter: ${TestCaseArgs.EXTRA_FILE_NAME}",
                metadata = mapOf("hint" to "pass file name via --es ${TestCaseArgs.EXTRA_FILE_NAME}"),
            )
        }
        val rows = api.getMedia(mediaType, volume, TestFixtures.projection(mediaType))
        val candidates = rows
            .map { row -> row.associate { it.columnName to (it.value?.toString() ?: "") } }
            .filter { row ->
                row[MediaStore.MediaColumns.DISPLAY_NAME] == fileName ||
                    row[MediaStore.MediaColumns.DATA]?.endsWith("/$fileName") == true
            }
        if (candidates.isEmpty()) {
            return@measure testCase.fail(
                message = "media row not found",
                metadata = mapOf(
                    "mediaType" to mediaType.name,
                    "fileName" to fileName,
                    "rowCount" to rows.size.toString(),
                ),
            )
        }
        val matched = candidates.firstOrNull { row ->
            row[MediaStore.MediaColumns.DATA] == expectedPath
        } ?: candidates.first()
        val actualPath = matched[MediaStore.MediaColumns.DATA].orEmpty()
        if (actualPath != expectedPath) {
            return@measure testCase.fail(
                message = "DATA path mismatch",
                metadata = queryPathMetadata(mediaType, fileName, expectedPath, matched),
            )
        }
        testCase.pass(
            message = "query path matched expected DATA",
            metadata = queryPathMetadata(mediaType, fileName, expectedPath, matched),
        )
    }

    private fun create(
        testCase: TestCase,
        mediaType: IMediaStoreApi.MediaType,
        args: TestCaseArgs,
    ): TestResult = testCase.measure {
        val payload = args.payloadOr(TestFixtures.initialPayload(mediaType))
        val fileName = args.fileName ?: TestFixtures.fileName(mediaType)
        val uri = api.createMedia(
            mediaType,
            volume,
            fileName,
            payload,
            args.relativePath,
            args.keepPending,
        )
            ?: return@measure testCase.fail("createMedia returned null")
        testCase.pass(
            message = "create succeeded",
            metadata = mapOf(
                "uri" to uri.toString(),
                "fileName" to fileName,
                "relativePath" to args.relativePath.orEmpty(),
            ),
        )
    }

    private fun createDenied(
        testCase: TestCase,
        mediaType: IMediaStoreApi.MediaType,
        args: TestCaseArgs,
    ): TestResult = testCase.measure {
        val payload = args.payloadOr(TestFixtures.initialPayload(mediaType))
        val fileName = args.fileName ?: TestFixtures.fileName(mediaType)
        val uri = api.createMedia(
            mediaType,
            volume,
            fileName,
            payload,
            args.relativePath,
            args.keepPending,
        )
        if (uri != null) {
            return@measure testCase.fail(
                message = "createMedia unexpectedly succeeded",
                metadata = mapOf(
                    "uri" to uri.toString(),
                    "fileName" to fileName,
                    "relativePath" to args.relativePath.orEmpty(),
                ),
            )
        }
        testCase.pass(
            message = "create denied as expected",
            metadata = mapOf(
                "fileName" to fileName,
                "relativePath" to args.relativePath.orEmpty(),
            ),
        )
    }

    private fun read(
        testCase: TestCase,
        mediaType: IMediaStoreApi.MediaType,
        args: TestCaseArgs,
    ): TestResult = testCase.measure {
        val uri = args.requireMediaUri(testCase) ?: return@measure args.missingUriResult(testCase)
        val expected = args.expectedPayload
        val readBack = readMediaBytesWithRetry(uri)
        if (readBack == null) {
            return@measure testCase.fail(
                message = "readMedia returned null",
                metadata = uriMetadata(uri),
            )
        }
        if (expected != null && !readBack.contentEquals(expected)) {
            return@measure testCase.fail(
                message = "payload mismatch",
                metadata = uriMetadata(uri) + mapOf(
                    "expectedSize" to expected.size.toString(),
                    "actualSize" to readBack.size.toString(),
                ),
            )
        }
        testCase.pass(
            message = if (expected != null) "read matched expected payload" else "read completed",
            metadata = uriMetadata(uri) + mapOf("bytesRead" to readBack.size.toString()),
        )
    }

    private fun write(
        testCase: TestCase,
        mediaType: IMediaStoreApi.MediaType,
        args: TestCaseArgs,
    ): TestResult = testCase.measure {
        val uri = args.requireMediaUri(testCase) ?: return@measure args.missingUriResult(testCase)
        val payload = args.payloadOr(TestFixtures.updatedPayload(mediaType))
        if (!writeMediaWithRetry(uri, payload)) {
            return@measure testCase.fail(
                message = "writeMedia returned false",
                metadata = uriMetadata(uri),
            )
        }
        if (args.expectedPayload != null) {
            val readBack = readMediaBytesWithRetry(uri)
            if (readBack == null || !readBack.contentEquals(args.expectedPayload)) {
                return@measure testCase.fail(
                    message = "read after write did not match expected_payload",
                    metadata = uriMetadata(uri),
                )
            }
        }
        testCase.pass(
            message = "write succeeded",
            metadata = uriMetadata(uri) + mapOf("bytesWritten" to payload.size.toString()),
        )
    }

    private fun delete(testCase: TestCase, args: TestCaseArgs): TestResult = testCase.measure {
        val uri = args.requireMediaUri(testCase) ?: return@measure args.missingUriResult(testCase)
        val deleted = api.deleteMedia(uri)
        if (!deleted) {
            return@measure testCase.fail(
                message = "deleteMedia returned false",
                metadata = uriMetadata(uri),
            )
        }
        testCase.pass(message = "delete succeeded", metadata = uriMetadata(uri))
    }

    private fun thumbnail(testCase: TestCase, args: TestCaseArgs): TestResult = testCase.measure {
        val uri = args.requireMediaUri(testCase) ?: return@measure args.missingUriResult(testCase)
        val bitmap = api.loadThumbnail(uri, Size(200, 200))
        if (bitmap == null) {
            return@measure testCase.fail(
                message = "loadThumbnail returned null",
                metadata = uriMetadata(uri),
            )
        }
        testCase.pass(
            message = "thumbnail loaded",
            metadata = uriMetadata(uri) + mapOf(
                "width" to bitmap.width.toString(),
                "height" to bitmap.height.toString(),
            ),
        )
    }

    private fun readMediaBytesWithRetry(uri: Uri): ByteArray? {
        var lastError: Exception? = null
        repeat(IO_RETRY_COUNT) { index ->
            try {
                api.readMedia(uri)?.use { return it.readBytes() }
            } catch (e: Exception) {
                lastError = e
            }
            if (index < IO_RETRY_COUNT - 1) {
                Thread.sleep(IO_RETRY_DELAY_MS)
            }
        }
        lastError?.let { throw it }
        return null
    }

    private fun writeMediaWithRetry(uri: Uri, payload: ByteArray): Boolean {
        repeat(IO_RETRY_COUNT) { index ->
            if (api.writeMedia(uri, payload)) {
                return true
            }
            if (index < IO_RETRY_COUNT - 1) {
                Thread.sleep(IO_RETRY_DELAY_MS)
            }
        }
        return false
    }

    private fun uriMetadata(uri: Uri): Map<String, String> = mapOf("uri" to uri.toString())

    private fun queryPathMetadata(
        mediaType: IMediaStoreApi.MediaType,
        fileName: String,
        expectedPath: String,
        row: Map<String, String>,
    ): Map<String, String> {
        val metadata = mutableMapOf(
            "mediaType" to mediaType.name,
            "fileName" to fileName,
            "expectedPath" to expectedPath,
            "actualPath" to row[MediaStore.MediaColumns.DATA].orEmpty(),
            "relativePath" to row[MediaStore.MediaColumns.RELATIVE_PATH].orEmpty(),
        )
        bucketIdColumn(mediaType)?.let { column ->
            metadata["bucketId"] = row[column].orEmpty()
        }
        return metadata
    }

    private fun bucketIdColumn(mediaType: IMediaStoreApi.MediaType): String? = when (mediaType) {
        IMediaStoreApi.MediaType.IMAGE -> MediaStore.Images.ImageColumns.BUCKET_ID
        IMediaStoreApi.MediaType.VIDEO -> MediaStore.Video.VideoColumns.BUCKET_ID
        IMediaStoreApi.MediaType.AUDIO,
        IMediaStoreApi.MediaType.FILE,
        IMediaStoreApi.MediaType.DOWNLOAD -> null
    }

    companion object {
        private const val IO_RETRY_COUNT = 8
        private const val IO_RETRY_DELAY_MS = 150L
        private const val MAX_QUERY_ROWS = 50
        private const val MAX_QUERY_SAMPLE_ROWS = 10
    }
}

private fun TestCaseArgs.requireMediaUri(testCase: TestCase): Uri? = mediaUri

private fun TestCaseArgs.missingUriResult(testCase: TestCase): TestResult =
    testCase.fail(
        message = "missing required parameter: ${TestCaseArgs.EXTRA_MEDIA_URI}",
        metadata = mapOf("hint" to "pass content URI via broadcast --es ${TestCaseArgs.EXTRA_MEDIA_URI}"),
    )

private fun TestCaseArgs.missingExpectedPathResult(testCase: TestCase): TestResult =
    testCase.fail(
        message = "missing required parameter: ${TestCaseArgs.EXTRA_EXPECTED_PATH}",
        metadata = mapOf("hint" to "pass expected DATA path via --es ${TestCaseArgs.EXTRA_EXPECTED_PATH}"),
    )
