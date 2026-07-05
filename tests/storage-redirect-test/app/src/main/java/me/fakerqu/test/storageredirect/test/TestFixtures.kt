package me.fakerqu.test.storageredirect.test

import android.provider.MediaStore
import me.fakerqu.media_store_api.IMediaStoreApi
import java.util.Base64

object TestFixtures {
    private const val PAYLOAD_PREFIX = "storage-redirect-test:"

    fun projection(mediaType: IMediaStoreApi.MediaType): Array<String> = when (mediaType) {
        IMediaStoreApi.MediaType.IMAGE ->
            arrayOf(
                MediaStore.Images.ImageColumns._ID,
                MediaStore.MediaColumns.DISPLAY_NAME,
                MediaStore.Images.ImageColumns.RELATIVE_PATH,
                MediaStore.Images.ImageColumns.TITLE,
                MediaStore.Images.ImageColumns.DATA,
                MediaStore.Images.ImageColumns.BUCKET_ID,
            )

        IMediaStoreApi.MediaType.VIDEO ->
            arrayOf(
                MediaStore.Video.VideoColumns._ID,
                MediaStore.MediaColumns.DISPLAY_NAME,
                MediaStore.Video.VideoColumns.RELATIVE_PATH,
                MediaStore.Video.VideoColumns.TITLE,
                MediaStore.Video.VideoColumns.DATA,
                MediaStore.Video.VideoColumns.BUCKET_ID,
            )

        IMediaStoreApi.MediaType.AUDIO ->
            arrayOf(
                MediaStore.Audio.AudioColumns._ID,
                MediaStore.MediaColumns.DISPLAY_NAME,
                MediaStore.Audio.AudioColumns.RELATIVE_PATH,
                MediaStore.Audio.AudioColumns.TITLE,
                MediaStore.Audio.AudioColumns.DATA,
            )

        IMediaStoreApi.MediaType.FILE ->
            arrayOf(
                MediaStore.Files.FileColumns._ID,
                MediaStore.MediaColumns.DISPLAY_NAME,
                MediaStore.Files.FileColumns.RELATIVE_PATH,
                MediaStore.Files.FileColumns.TITLE,
                MediaStore.Files.FileColumns.DATA,
            )

        IMediaStoreApi.MediaType.DOWNLOAD ->
            arrayOf(
                MediaStore.Downloads._ID,
                MediaStore.MediaColumns.DISPLAY_NAME,
                MediaStore.Downloads.RELATIVE_PATH,
                MediaStore.Downloads.TITLE,
                MediaStore.Downloads.DATA,
            )
    }

    fun fileName(mediaType: IMediaStoreApi.MediaType): String =
        "srt_${mediaType.name.lowercase()}_${System.currentTimeMillis()}${extension(mediaType)}"

    fun initialPayload(mediaType: IMediaStoreApi.MediaType): ByteArray =
        mediaPayload(mediaType) ?: "$PAYLOAD_PREFIX${mediaType.name}:initial".toByteArray()

    fun updatedPayload(mediaType: IMediaStoreApi.MediaType): ByteArray =
        mediaPayload(mediaType) ?: "$PAYLOAD_PREFIX${mediaType.name}:updated".toByteArray()

    fun filePayload(tag: String): ByteArray =
        "$PAYLOAD_PREFIX:file:$tag".toByteArray()

    private fun extension(mediaType: IMediaStoreApi.MediaType): String = when (mediaType) {
        IMediaStoreApi.MediaType.IMAGE -> ".jpg"
        IMediaStoreApi.MediaType.VIDEO -> ".mp4"
        IMediaStoreApi.MediaType.AUDIO -> ".mp3"
        IMediaStoreApi.MediaType.FILE -> ".txt"
        IMediaStoreApi.MediaType.DOWNLOAD -> ".bin"
    }

    private fun mediaPayload(mediaType: IMediaStoreApi.MediaType): ByteArray? = when (mediaType) {
        IMediaStoreApi.MediaType.IMAGE -> IMAGE_PAYLOAD.copyOf()
        IMediaStoreApi.MediaType.VIDEO -> VIDEO_PAYLOAD.copyOf()
        IMediaStoreApi.MediaType.AUDIO -> AUDIO_PAYLOAD.copyOf()
        IMediaStoreApi.MediaType.FILE,
        IMediaStoreApi.MediaType.DOWNLOAD -> null
    }

    private val IMAGE_PAYLOAD: ByteArray = decodeBase64(
        "/9j/4AAQSkZJRgABAgAAAQABAAD//gAQTGF2YzYxLjE5LjEwMAD/2wBDAAgEBAQEBAUFBQUFBQYGBgYGBgYGBgYGBgYHBwcICAgHBwcGBgcHCAgICAkJCQgICAgJCQoKCgwMCwsODg4RERT/xABLAAEBAAAAAAAAAAAAAAAAAAAACAEBAAAAAAAAAAAAAAAAAAAAABABAAAAAAAAAAAAAAAAAAAAABEBAAAAAAAAAAAAAAAAAAAAAP/AABEIAAIAAgMBIgACEQADEQD/2gAMAwEAAhEDEQA/AJ/AB//Z",
    )

    private val VIDEO_PAYLOAD: ByteArray = decodeBase64(
        "AAAAIGZ0eXBpc29tAAACAGlzb21pc28yYXZjMW1wNDEAAAMUbW9vdgAAAGxtdmhkAAAAAAAAAAAAAAAAAAAD6AAAA+gAAQAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAAj90cmFrAAAAXHRraGQAAAADAAAAAAAAAAAAAAABAAAAAAAAA+gAAAAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAABAAAAAQAAAAAAAkZWR0cwAAABxlbHN0AAAAAAAAAAEAAAPoAAAAAAABAAAAAAG3bWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAABAAAAAQABVxAAAAAAALWhkbHIAAAAAAAAAAHZpZGUAAAAAAAAAAAAAAABWaWRlb0hhbmRsZXIAAAABYm1pbmYAAAAUdm1oZAAAAAEAAAAAAAAAAAAAACRkaW5mAAAAHGRyZWYAAAAAAAAAAQAAAAx1cmwgAAAAAQAAASJzdGJsAAAAvnN0c2QAAAAAAAAAAQAAAK5hdmMxAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAAAABAAEABIAAAASAAAAAAAAAABFUxhdmM2MS4xOS4xMDAgbGlieDI2NAAAAAAAAAAAAAAAGP//AAAANGF2Y0MBZAAK/+EAF2dkAAqs2V7ARAAAAwAEAAADAAg8SJZYAQAGaOvjyyLA/fj4AAAAABBwYXNwAAAAAQAAAAEAAAAUYnRydAAAAAAAABYoAAAWKAAAABhzdHRzAAAAAAAAAAEAAAABAABAAAAAABxzdHNjAAAAAAAAAAEAAAABAAAAAQAAAAEAAAAUc3RzegAAAAAAAALFAAAAAQAAABRzdGNvAAAAAAAAAAEAAANEAAAAYXVkdGEAAABZbWV0YQAAAAAAAAAhaGRscgAAAAAAAAAAbWRpcmFwcGwAAAAAAAAAAAAAAAAsaWxzdAAAACSpdG9vAAAAHGRhdGEAAAABAAAAAExhdmY2MS43LjEwMAAAAAhmcmVlAAACzW1kYXQAAAKtBgX//6ncRem95tlIt5Ys2CDZI+7veDI2NCAtIGNvcmUgMTY0IHIzMTkyIGMyNGUwNmMgLSBILjI2NC9NUEVHLTQgQVZDIGNvZGVjIC0gQ29weWxlZnQgMjAwMy0yMDI0IC0gaHR0cDovL3d3dy52aWRlb2xhbi5vcmcveDI2NC5odG1sIC0gb3B0aW9uczogY2FiYWM9MSByZWY9MyBkZWJsb2NrPTE6MDowIGFuYWx5c2U9MHgzOjB4MTEzIG1lPWhleCBzdWJtZT03IHBzeT0xIHBzeV9yZD0xLjAwOjAuMDAgbWl4ZWRfcmVmPTEgbWVfcmFuZ2U9MTYgY2hyb21hX21lPTEgdHJlbGxpcz0xIDh4OGRjdD0xIGNxbT0wIGRlYWR6b25lPTIxLDExIGZhc3RfcHNraXA9MSBjaHJvbWFfcXBfb2Zmc2V0PS0yIHRocmVhZHM9MSBsb29rYWhlYWRfdGhyZWFkcz0xIHNsaWNlZF90aHJlYWRzPTAgbnI9MCBkZWNpbWF0ZT0xIGludGVybGFjZWQ9MCBibHVyYXlfY29tcGF0PTAgY29uc3RyYWluZWRfaW50cmE9MCBiZnJhbWVzPTMgYl9weXJhbWlkPTIgYl9hZGFwdD0xIGJfYmlhcz0wIGRpcmVjdD0xIHdlaWdodGI9MSBvcGVuX2dvcD0wIHdlaWdodHA9MiBrZXlpbnQ9MjUwIGtleWludF9taW49MSBzY2VuZWN1dD00MCBpbnRyYV9yZWZyZXNoPTAgcmNfbG9va2FoZWFkPTQwIHJjPWNyZiBtYnRyZWU9MSBjcmY9MjMuMCBxY29tcD0wLjYwIHFwbWluPTAgcXBtYXg9NjkgcXBzdGVwPTQgaXBfcmF0aW89MS40MCBhcT0xOjEuMDAAgAAAABBliIQAFf/+98nvwKbr29+B",
    )

    private val AUDIO_PAYLOAD: ByteArray = decodeBase64(
        "SUQzBAAAAAAAIlRTU0UAAAAOAAADTGF2ZjYxLjcuMTAwAAAAAAAAAAAAAAD/4zjAAAAAAAAAAAAASW5mbwAAAA8AAAAEAAAB+ACSkpKSkpKSkpKSkpKSkpKSkpKSkpKSkpK2tra2tra2tra2tra2tra2tra2tra2tra229vb29vb29vb29vb29vb29vb29vb29vb2/////////////////////////////////8AAAAATGF2YzYxLjE5AAAAAAAAAAAAAAAAJAOgAAAAAAAAAfhBGl4cAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD/4xjEAAAAA0gAAAAATEFNRTMuMTAwVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVX/4xjEOwAAA0gAAAAAVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVX/4xjEdgAAA0gAAAAAVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVX/4xjEsQAAA0gAAAAAVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVU=",
    )

    private fun decodeBase64(value: String): ByteArray = Base64.getDecoder().decode(value)
}
