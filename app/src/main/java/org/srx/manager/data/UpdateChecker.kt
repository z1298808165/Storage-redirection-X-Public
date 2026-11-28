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

/** 更新的两个载体：管理 App 与模块 zip 各自独立判断，互不牵连。 */
enum class UpdateComponent {
  App,
  Module,
}

/** 本机已安装的两个版本；模块未安装时 [module] 为空，此时只判定 App。 */
data class InstalledVersions(val app: String, val module: String)

/** 某个组件从哪个版本升到哪个版本。 */
data class ComponentUpdate(
    val component: UpdateComponent,
    val fromVersion: String,
    val toVersion: String,
)

/**
 * 一次检查的结论。
 *
 * 只有「确实比已装版本新」的组件才会出现在 [appUpdate] / [moduleUpdate] 里，两者都为 null 时 [UpdateChecker] 直接返回
 * null。弹窗因此只渲染有变化的组件，且整次检查只有一个对象 —— 结构上 不可能出现「App 一个弹窗、模块一个弹窗」。
 */
data class UpdateAvailability(
    val release: ReleaseUpdate,
    val appUpdate: ComponentUpdate?,
    val moduleUpdate: ComponentUpdate?,
)

/** 发布条目本身的元信息；版本判定已经拆到 [UpdateAvailability] 的组件字段里。 */
data class ReleaseUpdate(
    val tagName: String,
    val versionName: String,
    val title: String,
    val htmlUrl: String,
    val channel: UpdateChannel,
    val prerelease: Boolean,
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
      installed: InstalledVersions,
      channel: UpdateChannel,
  ): UpdateAvailability? =
      withContext(Dispatchers.IO) {
        findUpdate(
            manifest = fetchManifest(manifestUrl),
            repository = repository,
            installed = installed,
            channel = channel,
        )
      }

  internal fun checkManifest(
      manifestJson: String,
      repository: String,
      installed: InstalledVersions,
      channel: UpdateChannel,
  ): UpdateAvailability? =
      findUpdate(
          manifest = ManifestJson.decodeFromString<UpdateManifestDto>(manifestJson),
          repository = repository,
          installed = installed,
          channel = channel,
      )

  private fun findUpdate(
      manifest: UpdateManifestDto,
      repository: String,
      installed: InstalledVersions,
      channel: UpdateChannel,
  ): UpdateAvailability? {
    // 用清单的检测版本挑最新条目：清单写入侧保证它等于 App 版本与模块版本的较大者，
    // 因此不会因为只看某一个组件而选中较旧的条目。
    val candidate =
        manifest
            .entries()
            .asSequence()
            .filter { it.matches(channel) }
            .mapNotNull { entry ->
              val release = entry.release
              val version =
                  SemVersion.parse(release.version.ifBlank { release.tag })
                      ?: return@mapNotNull null
              ReleaseCandidate(entry, version)
            }
            .maxByOrNull { it.version } ?: return null
    val release = candidate.entry.release

    // v1 清单没有组件字段：此时 App 与模块共用条目版本，等价于旧行为。
    // 一旦清单带了组件字段，缺失的那一侧就表示「本次没有该组件的信息」，不能拿检测版本顶替
    // ——检测版本恒等于较大者，顶替会让没重编的组件被误报成有更新。
    val hasComponentFields = release.app != null || release.module != null
    val appTarget = if (hasComponentFields) release.app?.version.orEmpty() else release.version
    val moduleTarget =
        if (hasComponentFields) release.module?.version.orEmpty() else release.version
    val appUpdate = componentUpdate(UpdateComponent.App, installed.app, appTarget)
    val moduleUpdate = componentUpdate(UpdateComponent.Module, installed.module, moduleTarget)
    if (appUpdate == null && moduleUpdate == null) return null

    val releaseRepository =
        release.repository.takeIf { it.isNotBlank() }
            ?: manifest.repository.takeIf { it.isNotBlank() }
            ?: repository
    val tag = release.tag.ifBlank { release.version }
    return UpdateAvailability(
        release =
            ReleaseUpdate(
                tagName = tag,
                versionName = release.version.ifBlank { tag },
                title = release.title.takeIf { it.isNotBlank() } ?: tag,
                htmlUrl =
                    release.url.takeIf { it.isNotBlank() }
                        ?: "https://github.com/$releaseRepository/releases/tag/$tag",
                channel = candidate.entry.channel,
                prerelease = candidate.entry.channel == UpdateChannel.Beta || release.prerelease,
                releaseNotes = release.releaseNotes,
            ),
        appUpdate = appUpdate,
        moduleUpdate = moduleUpdate,
    )
  }

  /**
   * 单个组件的判定。
   *
   * 已装版本读不出来（模块未安装、版本串为空）时不判定该组件：宁可漏报一次，也不要把「读不到版本」当成「有更新」去打扰用户。
   */
  private fun componentUpdate(
      component: UpdateComponent,
      installedVersion: String,
      targetVersion: String,
  ): ComponentUpdate? {
    val installed = installedVersion.trim()
    val target = targetVersion.trim()
    if (installed.isBlank() || target.isBlank()) return null
    if (compareComponentVersions(installed, target) <= 0) return null
    return ComponentUpdate(component = component, fromVersion = installed, toVersion = target)
  }

  /**
   * 组件版本比较：返回值 > 0 表示 target 比 installed 新。
   *
   * App 走日期时间线（202611281259），模块走 semver，两者不在同一命名空间：同一空间内按各自规则比较，跨空间时把日期线视为更新——历史发布都是 semver，App
   * 切到日期线的那一刻必须被判成「有新版本」。
   */
  private fun compareComponentVersions(installed: String, target: String): Int {
    val installedSemver = SemVersion.parse(installed)
    val targetSemver = SemVersion.parse(target)
    if (installedSemver != null && targetSemver != null) {
      return targetSemver.compareTo(installedSemver)
    }
    val installedIsDate = installed.isDateTimeVersion()
    val targetIsDate = target.isDateTimeVersion()
    if (installedIsDate && targetIsDate) {
      // 日期线定长（yyyyMMddHHmm）：先比长度再比字符，等价于按整数比较。
      if (installed.length != target.length) return target.length.compareTo(installed.length)
      return target.compareTo(installed)
    }
    if (targetIsDate != installedIsDate) return if (targetIsDate) 1 else -1
    return target.compareTo(installed)
  }

  private fun String.isDateTimeVersion(): Boolean = length >= 8 && all { it.isDigit() }

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
    throw lastError ?: IOException("更新清单请求失败")
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
      val releaseNotes: String = "",
      val app: ComponentDto? = null,
      val module: ComponentDto? = null,
  )

  @Serializable
  private data class ComponentDto(
      val version: String = "",
      val url: String = "",
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
