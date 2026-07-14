package org.srx.manager.data

import java.io.IOException
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json

enum class UpdateChannel {
  Stable,
  Beta,
  All,
}

data class ReleaseUpdate(
    val tagName: String,
    val versionName: String,
    val title: String,
    val htmlUrl: String,
    val channel: UpdateChannel,
    val prerelease: Boolean,
    val downloadUrl: String? = null,
    val releaseNotes: String = "",
)

class UpdateChecker
internal constructor(
    private val userAgent: String,
    private val manifestBodyFetcher: ((String) -> String)? = null,
) {
  suspend fun check(
      manifestUrl: String,
      repository: String,
      currentVersionName: String,
      channel: UpdateChannel,
  ): ReleaseUpdate? =
      withContext(Dispatchers.IO) {
        findUpdate(
            manifest = fetchManifest(manifestUrl),
            repository = repository,
            currentVersionName = currentVersionName,
            channel = channel,
        )
      }

  internal fun checkManifest(
      manifestJson: String,
      repository: String,
      currentVersionName: String,
      channel: UpdateChannel,
  ): ReleaseUpdate? =
      findUpdate(
          manifest = ManifestJson.decodeFromString<UpdateManifestDto>(manifestJson),
          repository = repository,
          currentVersionName = currentVersionName,
          channel = channel,
      )

  private fun findUpdate(
      manifest: UpdateManifestDto,
      repository: String,
      currentVersionName: String,
      channel: UpdateChannel,
  ): ReleaseUpdate? {
    val currentVersion = SemVersion.parse(currentVersionName) ?: SemVersion.Zero
    return manifest
        .entries()
        .asSequence()
        .filter { it.matches(channel) }
        .mapNotNull { entry ->
          val release = entry.release
          val version =
              SemVersion.parse(release.version.ifBlank { release.tag }) ?: return@mapNotNull null
          if (version > currentVersion) ReleaseCandidate(entry, version) else null
        }
        .maxByOrNull { it.version }
        ?.let { candidate ->
          val release = candidate.entry.release
          val releaseRepository =
              release.repository.takeIf { it.isNotBlank() }
                  ?: manifest.repository.takeIf { it.isNotBlank() }
                  ?: repository
          val tag = release.tag.ifBlank { release.version }
          ReleaseUpdate(
              tagName = tag,
              versionName = release.version.ifBlank { tag },
              title = release.title.takeIf { it.isNotBlank() } ?: tag,
              htmlUrl =
                  release.url.takeIf { it.isNotBlank() }
                      ?: "https://github.com/$releaseRepository/releases/tag/$tag",
              channel = candidate.entry.channel,
              prerelease = candidate.entry.channel == UpdateChannel.Beta || release.prerelease,
              downloadUrl = release.downloadUrl.takeIf { it.isNotBlank() },
              releaseNotes = release.releaseNotes,
          )
        }
  }

  private fun ManifestEntry.matches(channel: UpdateChannel): Boolean =
      when (channel) {
        UpdateChannel.Stable -> this.channel == UpdateChannel.Stable
        UpdateChannel.Beta -> this.channel == UpdateChannel.Beta
        UpdateChannel.All -> true
      }

  private fun fetchManifest(manifestUrl: String): UpdateManifestDto {
    val urls = updateManifestUrls(manifestUrl)
    var lastError: Exception? = null
    for (url in urls) {
      try {
        val body = manifestBodyFetcher?.invoke(url) ?: fetchManifestBody(url)
        return ManifestJson.decodeFromString<UpdateManifestDto>(body)
      } catch (error: CancellationException) {
        throw error
      } catch (error: Exception) {
        lastError = error
      }
    }

    if (urls.size > 1) {
      throw IOException(
          "更新清单获取失败：首选源与备用镜像均不可用（最后错误：${lastError?.message ?: "未知错误"}）",
          lastError,
      )
    }
    throw lastError ?: IOException("Update manifest request failed")
  }

  private fun fetchManifestBody(manifestUrl: String): String {
    val connection =
        (URL(manifestUrl).openConnection() as HttpURLConnection).apply {
          requestMethod = "GET"
          connectTimeout = 10_000
          readTimeout = 10_000
          setRequestProperty("Accept", "application/json")
          setRequestProperty("User-Agent", userAgent)
        }
    try {
      val code = connection.responseCode
      if (code !in 200..299) {
        throw IOException(updateManifestError(code))
      }
      return connection.inputStream.bufferedReader().use { it.readText() }
    } finally {
      connection.disconnect()
    }
  }

  private fun updateManifestError(code: Int): String =
      when (code) {
        403 -> "更新清单访问被 GitHub 限制：HTTP 403，请稍后或切换网络"
        404 -> "更新清单请求失败：HTTP 404，当前网络或缓存节点未找到该文件"
        else -> "更新清单响应异常：HTTP $code"
      }

  private fun UpdateManifestDto.entries(): List<ManifestEntry> =
      listOfNotNull(
          stable?.takeIf { it.hasVersion() }?.let { ManifestEntry(UpdateChannel.Stable, it) },
          beta
              ?.takeIf { it.hasVersion() }
              ?.let { ManifestEntry(UpdateChannel.Beta, it.copy(prerelease = true)) },
      ) +
          releases.mapNotNull { release ->
            release
                .takeIf { it.hasVersion() }
                ?.let {
                  ManifestEntry(
                      channel =
                          if (
                              it.prerelease ||
                                  SemVersion.parse(it.version.ifBlank { it.tag })?.isPreRelease ==
                                      true
                          ) {
                            UpdateChannel.Beta
                          } else {
                            UpdateChannel.Stable
                          },
                      release = it,
                  )
                }
          }

  private fun ReleaseDto.hasVersion(): Boolean = version.isNotBlank() || tag.isNotBlank()

  private data class ReleaseCandidate(
      val entry: ManifestEntry,
      val version: SemVersion,
  )

  private data class ManifestEntry(
      val channel: UpdateChannel,
      val release: ReleaseDto,
  )

  @Serializable
  private data class UpdateManifestDto(
      val schema: Int = 1,
      val repository: String = "",
      val stable: ReleaseDto? = null,
      val beta: ReleaseDto? = null,
      val releases: List<ReleaseDto> = emptyList(),
  )

  @Serializable
  private data class ReleaseDto(
      val version: String = "",
      val tag: String = "",
      val title: String = "",
      val url: String = "",
      val repository: String = "",
      val prerelease: Boolean = false,
      val downloadUrl: String = "",
      val releaseNotes: String = "",
  )

  private companion object {
    val ManifestJson = Json { ignoreUnknownKeys = true }
  }
}

internal fun updateManifestUrls(manifestUrl: String): List<String> {
  val uri = runCatching { URI(manifestUrl) }.getOrNull() ?: return listOf(manifestUrl)
  val segments = uri.rawPath?.trim('/')?.split('/')?.filter { it.isNotBlank() }.orEmpty()
  if (
      !uri.scheme.equals("https", ignoreCase = true) ||
          !uri.host.equals("raw.githubusercontent.com", ignoreCase = true) ||
          uri.rawQuery != null ||
          uri.rawFragment != null ||
          segments.size != 4 ||
          segments.last() != "update.json"
  ) {
    return listOf(manifestUrl)
  }

  val mirrorUrl =
      "https://cdn.jsdelivr.net/gh/${segments[0]}/${segments[1]}@${segments[2]}/${segments[3]}"
  return listOf(manifestUrl, mirrorUrl)
}

private data class SemVersion(
    val major: Int,
    val minor: Int,
    val patch: Int,
    val preRelease: List<String>,
) : Comparable<SemVersion> {
  val isPreRelease: Boolean
    get() = preRelease.isNotEmpty()

  override fun compareTo(other: SemVersion): Int {
    compareValuesBy(this, other, SemVersion::major, SemVersion::minor, SemVersion::patch)
        .takeIf { it != 0 }
        ?.let {
          return it
        }
    if (preRelease.isEmpty() && other.preRelease.isNotEmpty()) return 1
    if (preRelease.isNotEmpty() && other.preRelease.isEmpty()) return -1
    return comparePreRelease(preRelease, other.preRelease)
  }

  private fun comparePreRelease(
      left: List<String>,
      right: List<String>,
  ): Int {
    val max = maxOf(left.size, right.size)
    for (index in 0 until max) {
      val leftPart = left.getOrNull(index) ?: return -1
      val rightPart = right.getOrNull(index) ?: return 1
      val leftNumber = leftPart.toIntOrNull()
      val rightNumber = rightPart.toIntOrNull()
      val result =
          when {
            leftNumber != null && rightNumber != null -> leftNumber.compareTo(rightNumber)
            leftNumber != null -> -1
            rightNumber != null -> 1
            else -> leftPart.compareTo(rightPart)
          }
      if (result != 0) return result
    }
    return 0
  }

  companion object {
    val Zero = SemVersion(0, 0, 0, emptyList())
    private val Pattern = Regex("""(?:^|[^0-9])v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?""")

    fun parse(value: String): SemVersion? {
      val match = Pattern.find(value.trim()) ?: return null
      val parts = match.groupValues
      return SemVersion(
          major = parts[1].toIntOrNull() ?: return null,
          minor = parts[2].toIntOrNull() ?: return null,
          patch = parts[3].toIntOrNull() ?: return null,
          preRelease =
              parts.getOrNull(4)?.takeIf { it.isNotBlank() }?.split('.')?.filter { it.isNotBlank() }
                  ?: emptyList(),
      )
    }
  }
}
