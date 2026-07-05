package me.fakerqu.test.storageredirect.test

import android.content.Context
import me.fakerqu.media_store_api.IMediaStoreApi

/**
 * 每个枚举值对应一个独立操作，可通过广播传入 [id] 及 [TestCaseArgs] 中的参数。
 */
enum class TestCase(val id: String) {
    ALL_EXCEPT_DELETE("all"),

    MEDIASTORE_QUERY_IMAGE("mediastore_query_image"),
    MEDIASTORE_QUERY_VIDEO("mediastore_query_video"),
    MEDIASTORE_QUERY_AUDIO("mediastore_query_audio"),
    MEDIASTORE_QUERY_FILE("mediastore_query_file"),
    MEDIASTORE_QUERY_DOWNLOAD("mediastore_query_download"),
    MEDIASTORE_QUERY_READ_ONLY_IMAGE("mediastore_query_read_only_image"),
    MEDIASTORE_QUERY_PATH_IMAGE("mediastore_query_path_image"),
    MEDIASTORE_QUERY_PATH_VIDEO("mediastore_query_path_video"),
    MEDIASTORE_QUERY_PATH_AUDIO("mediastore_query_path_audio"),
    MEDIASTORE_QUERY_PATH_FILE("mediastore_query_path_file"),
    MEDIASTORE_QUERY_PATH_DOWNLOAD("mediastore_query_path_download"),

    MEDIASTORE_CREATE_IMAGE("mediastore_create_image"),
    MEDIASTORE_CREATE_VIDEO("mediastore_create_video"),
    MEDIASTORE_CREATE_AUDIO("mediastore_create_audio"),
    MEDIASTORE_CREATE_FILE("mediastore_create_file"),
    MEDIASTORE_CREATE_DOWNLOAD("mediastore_create_download"),
    MEDIASTORE_CREATE_IMAGE_DENIED("mediastore_create_image_denied"),
    MEDIASTORE_CREATE_VIDEO_DENIED("mediastore_create_video_denied"),
    MEDIASTORE_CREATE_AUDIO_DENIED("mediastore_create_audio_denied"),
    MEDIASTORE_CREATE_FILE_DENIED("mediastore_create_file_denied"),
    MEDIASTORE_CREATE_DOWNLOAD_DENIED("mediastore_create_download_denied"),

    MEDIASTORE_READ_IMAGE("mediastore_read_image"),
    MEDIASTORE_READ_VIDEO("mediastore_read_video"),
    MEDIASTORE_READ_AUDIO("mediastore_read_audio"),
    MEDIASTORE_READ_FILE("mediastore_read_file"),
    MEDIASTORE_READ_DOWNLOAD("mediastore_read_download"),

    MEDIASTORE_WRITE_IMAGE("mediastore_write_image"),
    MEDIASTORE_WRITE_VIDEO("mediastore_write_video"),
    MEDIASTORE_WRITE_AUDIO("mediastore_write_audio"),
    MEDIASTORE_WRITE_FILE("mediastore_write_file"),
    MEDIASTORE_WRITE_DOWNLOAD("mediastore_write_download"),

    MEDIASTORE_DELETE_IMAGE("mediastore_delete_image"),
    MEDIASTORE_DELETE_VIDEO("mediastore_delete_video"),
    MEDIASTORE_DELETE_AUDIO("mediastore_delete_audio"),
    MEDIASTORE_DELETE_FILE("mediastore_delete_file"),
    MEDIASTORE_DELETE_DOWNLOAD("mediastore_delete_download"),

    MEDIASTORE_THUMBNAIL_IMAGE("mediastore_thumbnail_image"),
    MEDIASTORE_THUMBNAIL_VIDEO("mediastore_thumbnail_video"),

    FILE_LIST_DIR("file_list_dir"),
    FILE_CREATE("file_create"),
    FILE_READ("file_read"),
    FILE_WRITE("file_write"),
    FILE_WRITE_DENIED("file_write_denied"),
    FILE_DELETE("file_delete"),
    FILE_DELETE_DENIED("file_delete_denied"),
    FILE_MKDIR("file_mkdir"),
    FILE_MKDIR_DENIED("file_mkdir_denied"),
    FILE_RENAME("file_rename"),
    FILE_RENAME_DENIED("file_rename_denied"),
    FILE_STAT("file_stat"),
    FILE_ACCESS("file_access"),
    FILE_READLINK("file_readlink"),
    FILE_TRUNCATE("file_truncate"),
    FILE_TRUNCATE_DENIED("file_truncate_denied"),
    FILE_FTRUNCATE("file_ftruncate"),
    FILE_FTRUNCATE_DENIED("file_ftruncate_denied"),
    FILE_CHMOD("file_chmod"),
    FILE_CHMOD_DENIED("file_chmod_denied"),
    FILE_FCHMOD("file_fchmod"),
    FILE_FCHMOD_DENIED("file_fchmod_denied"),
    FILE_LINK("file_link"),
    FILE_LINK_DENIED("file_link_denied"),
    FILE_SYMLINK("file_symlink"),
    FILE_SYMLINK_DENIED("file_symlink_denied");

    fun getMediaType(): IMediaStoreApi.MediaType? = when (this) {
        MEDIASTORE_QUERY_IMAGE, MEDIASTORE_QUERY_READ_ONLY_IMAGE, MEDIASTORE_QUERY_PATH_IMAGE, MEDIASTORE_CREATE_IMAGE, MEDIASTORE_CREATE_IMAGE_DENIED, MEDIASTORE_READ_IMAGE,
        MEDIASTORE_WRITE_IMAGE, MEDIASTORE_DELETE_IMAGE, MEDIASTORE_THUMBNAIL_IMAGE ->
            IMediaStoreApi.MediaType.IMAGE

        MEDIASTORE_QUERY_VIDEO, MEDIASTORE_QUERY_PATH_VIDEO, MEDIASTORE_CREATE_VIDEO, MEDIASTORE_CREATE_VIDEO_DENIED, MEDIASTORE_READ_VIDEO,
        MEDIASTORE_WRITE_VIDEO, MEDIASTORE_DELETE_VIDEO, MEDIASTORE_THUMBNAIL_VIDEO ->
            IMediaStoreApi.MediaType.VIDEO

        MEDIASTORE_QUERY_AUDIO, MEDIASTORE_QUERY_PATH_AUDIO, MEDIASTORE_CREATE_AUDIO, MEDIASTORE_CREATE_AUDIO_DENIED, MEDIASTORE_READ_AUDIO,
        MEDIASTORE_WRITE_AUDIO, MEDIASTORE_DELETE_AUDIO ->
            IMediaStoreApi.MediaType.AUDIO

        MEDIASTORE_QUERY_FILE, MEDIASTORE_QUERY_PATH_FILE, MEDIASTORE_CREATE_FILE, MEDIASTORE_CREATE_FILE_DENIED, MEDIASTORE_READ_FILE,
        MEDIASTORE_WRITE_FILE, MEDIASTORE_DELETE_FILE ->
            IMediaStoreApi.MediaType.FILE

        MEDIASTORE_QUERY_DOWNLOAD, MEDIASTORE_QUERY_PATH_DOWNLOAD, MEDIASTORE_CREATE_DOWNLOAD, MEDIASTORE_CREATE_DOWNLOAD_DENIED, MEDIASTORE_READ_DOWNLOAD,
        MEDIASTORE_WRITE_DOWNLOAD, MEDIASTORE_DELETE_DOWNLOAD ->
            IMediaStoreApi.MediaType.DOWNLOAD

        else -> null
    }

    fun requiresMediaUri(): Boolean = when (this) {
        MEDIASTORE_READ_IMAGE, MEDIASTORE_READ_VIDEO, MEDIASTORE_READ_AUDIO,
        MEDIASTORE_READ_FILE, MEDIASTORE_READ_DOWNLOAD,
        MEDIASTORE_WRITE_IMAGE, MEDIASTORE_WRITE_VIDEO, MEDIASTORE_WRITE_AUDIO,
        MEDIASTORE_WRITE_FILE, MEDIASTORE_WRITE_DOWNLOAD,
        MEDIASTORE_DELETE_IMAGE, MEDIASTORE_DELETE_VIDEO, MEDIASTORE_DELETE_AUDIO,
        MEDIASTORE_DELETE_FILE, MEDIASTORE_DELETE_DOWNLOAD,
        MEDIASTORE_THUMBNAIL_IMAGE, MEDIASTORE_THUMBNAIL_VIDEO -> true

        else -> false
    }

    fun requiresFilePath(): Boolean = this in setOf(
        FILE_READ, FILE_WRITE, FILE_WRITE_DENIED, FILE_DELETE, FILE_DELETE_DENIED, FILE_CREATE,
        FILE_MKDIR, FILE_MKDIR_DENIED, FILE_RENAME, FILE_RENAME_DENIED,
        FILE_STAT, FILE_ACCESS, FILE_READLINK, FILE_TRUNCATE, FILE_TRUNCATE_DENIED,
        FILE_FTRUNCATE, FILE_FTRUNCATE_DENIED, FILE_CHMOD, FILE_CHMOD_DENIED,
        FILE_FCHMOD, FILE_FCHMOD_DENIED, FILE_LINK, FILE_LINK_DENIED,
        FILE_SYMLINK, FILE_SYMLINK_DENIED,
    )

    fun requiresFileDir(): Boolean = this == FILE_LIST_DIR

    fun run(context: Context, args: TestCaseArgs = TestCaseArgs()): TestResult {
        val mediaStore = MediaStoreTestCases(context)
        val file = FileTestCases(context)
        return when (this) {
            ALL_EXCEPT_DELETE -> error("ALL must be handled by StorageRedirectTestRunner")

            MEDIASTORE_QUERY_IMAGE -> mediaStore.queryImage()
            MEDIASTORE_QUERY_VIDEO -> mediaStore.queryVideo()
            MEDIASTORE_QUERY_AUDIO -> mediaStore.queryAudio()
            MEDIASTORE_QUERY_FILE -> mediaStore.queryFile()
            MEDIASTORE_QUERY_DOWNLOAD -> mediaStore.queryDownload()
            MEDIASTORE_QUERY_READ_ONLY_IMAGE -> mediaStore.queryReadOnlyImage(args)
            MEDIASTORE_QUERY_PATH_IMAGE -> mediaStore.queryImagePath(args)
            MEDIASTORE_QUERY_PATH_VIDEO -> mediaStore.queryVideoPath(args)
            MEDIASTORE_QUERY_PATH_AUDIO -> mediaStore.queryAudioPath(args)
            MEDIASTORE_QUERY_PATH_FILE -> mediaStore.queryFilePath(args)
            MEDIASTORE_QUERY_PATH_DOWNLOAD -> mediaStore.queryDownloadPath(args)

            MEDIASTORE_CREATE_IMAGE -> mediaStore.createImage(args)
            MEDIASTORE_CREATE_VIDEO -> mediaStore.createVideo(args)
            MEDIASTORE_CREATE_AUDIO -> mediaStore.createAudio(args)
            MEDIASTORE_CREATE_FILE -> mediaStore.createFile(args)
            MEDIASTORE_CREATE_DOWNLOAD -> mediaStore.createDownload(args)
            MEDIASTORE_CREATE_IMAGE_DENIED -> mediaStore.createImageDenied(args)
            MEDIASTORE_CREATE_VIDEO_DENIED -> mediaStore.createVideoDenied(args)
            MEDIASTORE_CREATE_AUDIO_DENIED -> mediaStore.createAudioDenied(args)
            MEDIASTORE_CREATE_FILE_DENIED -> mediaStore.createFileDenied(args)
            MEDIASTORE_CREATE_DOWNLOAD_DENIED -> mediaStore.createDownloadDenied(args)

            MEDIASTORE_READ_IMAGE -> mediaStore.readImage(args)
            MEDIASTORE_READ_VIDEO -> mediaStore.readVideo(args)
            MEDIASTORE_READ_AUDIO -> mediaStore.readAudio(args)
            MEDIASTORE_READ_FILE -> mediaStore.readFile(args)
            MEDIASTORE_READ_DOWNLOAD -> mediaStore.readDownload(args)

            MEDIASTORE_WRITE_IMAGE -> mediaStore.writeImage(args)
            MEDIASTORE_WRITE_VIDEO -> mediaStore.writeVideo(args)
            MEDIASTORE_WRITE_AUDIO -> mediaStore.writeAudio(args)
            MEDIASTORE_WRITE_FILE -> mediaStore.writeFile(args)
            MEDIASTORE_WRITE_DOWNLOAD -> mediaStore.writeDownload(args)

            MEDIASTORE_DELETE_IMAGE -> mediaStore.deleteImage(args)
            MEDIASTORE_DELETE_VIDEO -> mediaStore.deleteVideo(args)
            MEDIASTORE_DELETE_AUDIO -> mediaStore.deleteAudio(args)
            MEDIASTORE_DELETE_FILE -> mediaStore.deleteFile(args)
            MEDIASTORE_DELETE_DOWNLOAD -> mediaStore.deleteDownload(args)

            MEDIASTORE_THUMBNAIL_IMAGE -> mediaStore.thumbnailImage(args)
            MEDIASTORE_THUMBNAIL_VIDEO -> mediaStore.thumbnailVideo(args)

            FILE_LIST_DIR -> file.listDir(args)
            FILE_CREATE -> file.create(args)
            FILE_READ -> file.read(args)
            FILE_WRITE -> file.write(args)
            FILE_WRITE_DENIED -> file.writeDenied(args)
            FILE_DELETE -> file.delete(args)
            FILE_DELETE_DENIED -> file.deleteDenied(args)
            FILE_MKDIR -> file.mkdir(args)
            FILE_MKDIR_DENIED -> file.mkdirDenied(args)
            FILE_RENAME -> file.rename(args)
            FILE_RENAME_DENIED -> file.renameDenied(args)
            FILE_STAT -> file.stat(args)
            FILE_ACCESS -> file.access(args)
            FILE_READLINK -> file.readlink(args)
            FILE_TRUNCATE -> file.truncate(args)
            FILE_TRUNCATE_DENIED -> file.truncateDenied(args)
            FILE_FTRUNCATE -> file.ftruncate(args)
            FILE_FTRUNCATE_DENIED -> file.ftruncateDenied(args)
            FILE_CHMOD -> file.chmod(args)
            FILE_CHMOD_DENIED -> file.chmodDenied(args)
            FILE_FCHMOD -> file.fchmod(args)
            FILE_FCHMOD_DENIED -> file.fchmodDenied(args)
            FILE_LINK -> file.link(args)
            FILE_LINK_DENIED -> file.linkDenied(args)
            FILE_SYMLINK -> file.symlink(args)
            FILE_SYMLINK_DENIED -> file.symlinkDenied(args)
        }
    }

    companion object {
        fun fromId(id: String?): TestCase = entries.firstOrNull { it.id == id } ?: ALL_EXCEPT_DELETE

        val executableCases: List<TestCase> = entries.filter { it != ALL_EXCEPT_DELETE }
    }
}
