@file:Suppress("UnstableApiUsage")

import java.io.File
import java.util.Properties
import org.gradle.api.file.DuplicatesStrategy
import org.gradle.api.tasks.Sync
import org.gradle.api.tasks.testing.Test

plugins {
  alias(libs.plugins.android.application)
  alias(libs.plugins.kotlin.compose)
  alias(libs.plugins.kotlin.serialization)
}

val cargoVersion =
    rootProject.file("Cargo.toml").readText().let { content ->
      Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(content)?.groupValues?.get(1) ?: "0.1.0"
    }

val autoManifestCommitPrefixes = listOf("CI：更新更新清单", "发布：更新更新清单")
val buildCountOffsets =
    mapOf(
        // Previous CI builds for Cargo.toml 1.2.56 already published 1.2.57-ci.284.
        // Continue that visible version line without reserving Cargo.toml 1.2.58 early.
        "1.2.57" to 285,
    )
val legacyCiVersionCodeOverrides = setOf("1.2.57")

data class ResolvedBuildVersion(
    val baseVersion: String,
    val buildCount: Int,
    val versionName: String,
    val versionCode: Int,
)

fun runGit(vararg args: String): String? =
    runCatching {
          val process =
              ProcessBuilder("git", *args)
                  .directory(rootProject.projectDir)
                  .redirectErrorStream(true)
                  .start()
          val output = process.inputStream.bufferedReader().use { it.readText() }.trim()
          if (process.waitFor() == 0) output else null
        }
        .getOrNull()

fun cargoVersionFromText(text: String): String? =
    Regex("""(?m)^version\s*=\s*"([^"]+)"""").find(text)?.groupValues?.get(1)

fun cargoVersionAtCommit(commit: String): String? =
    runGit("show", "$commit:Cargo.toml")?.let(::cargoVersionFromText)

fun headCargoVersion(): String? = runGit("show", "HEAD:Cargo.toml")?.let(::cargoVersionFromText)

fun versionStartCommit(version: String): String? {
  val commits =
      runGit("rev-list", "--first-parent", "--reverse", "HEAD", "--", "Cargo.toml")
          ?.lineSequence()
          ?.filter { it.isNotBlank() }
          ?.toList() ?: return null
  var previousVersion: String? = null
  var start: String? = null
  for (commit in commits) {
    val commitVersion = cargoVersionAtCommit(commit)
    if (commitVersion == version && previousVersion != version) {
      start = commit
    }
    previousVersion = commitVersion
  }
  return start
}

fun isAutoManifestCommit(commit: String): Boolean {
  val subject = runGit("log", "-1", "--pretty=%s", commit) ?: return false
  return autoManifestCommitPrefixes.any(subject::startsWith)
}

fun isWorktreeDirty(): Boolean = !runGit("status", "--porcelain").isNullOrBlank()

fun validateBaseVersion(version: String): List<Int> {
  val numbers = version.split('.').map { part -> part.toIntOrNull() }
  if (numbers.size != 3 || numbers.any { it == null }) {
    throw GradleException("Cargo.toml version must be MAJOR.MINOR.PATCH, got: $version")
  }
  return numbers.map { requireNotNull(it) }
}

fun baseVersionCode(version: String): Int {
  val (major, minor, patch) = validateBaseVersion(version)
  return (major * 1_000_000) + (minor * 10_000) + (patch * 100)
}

fun ciVersionCode(baseVersion: String, buildCount: Int): Int {
  if (baseVersion in legacyCiVersionCodeOverrides) {
    return baseVersionCode(baseVersion) - 1
  }
  if (buildCount !in 1..99) {
    throw GradleException(
        "CI build count must be between 1 and 99. Bump Cargo.toml version before continuing."
    )
  }
  return baseVersionCode(baseVersion) - 100 + buildCount
}

fun versionCodeFrom(versionName: String): Int {
  val baseVersion = versionName.substringBefore('-')
  val ciBuild = Regex("""-ci\.(\d+)$""").find(versionName)?.groupValues?.get(1)?.toIntOrNull()
  return if (ciBuild != null) ciVersionCode(baseVersion, ciBuild) else baseVersionCode(baseVersion)
}

fun resolveBuildVersion(baseVersion: String, includeDirty: Boolean): ResolvedBuildVersion {
  val headVersion = headCargoVersion()
  val start = if (headVersion == baseVersion) versionStartCommit(baseVersion) else null
  var count =
      if (start == null) {
        0
      } else {
        runGit("rev-list", "--first-parent", "--reverse", "$start^..HEAD")
            ?.lineSequence()
            ?.filter { it.isNotBlank() }
            ?.count { !isAutoManifestCommit(it) } ?: 0
      }

  if (includeDirty && isWorktreeDirty()) {
    if (headVersion != baseVersion) {
      count = 0
    }
    count += 1
  }

  val buildCount = count.coerceAtLeast(1) + (buildCountOffsets[baseVersion] ?: 0)
  return ResolvedBuildVersion(
      baseVersion = baseVersion,
      buildCount = buildCount,
      versionName = "$baseVersion-ci.$buildCount",
      versionCode = ciVersionCode(baseVersion, buildCount),
  )
}

val configuredAppVersionName =
    providers.environmentVariable("VERSION").orNull
        ?: providers.gradleProperty("srx.versionName").orNull
val configuredAppVersionCode =
    providers.environmentVariable("VERSION_CODE").orNull
        ?: providers.gradleProperty("srx.versionCode").orNull
val defaultBuildVersion = resolveBuildVersion(cargoVersion, includeDirty = true)
val appVersionName = configuredAppVersionName ?: defaultBuildVersion.versionName
val appVersionCode =
    configuredAppVersionCode?.toIntOrNull()
        ?: configuredAppVersionName?.let(::versionCodeFrom)
        ?: defaultBuildVersion.versionCode
val appCompileSdk = providers.gradleProperty("srx.compileSdk").orNull?.toIntOrNull() ?: 37
val appTargetSdk = providers.gradleProperty("srx.targetSdk").orNull?.toIntOrNull() ?: appCompileSdk
val defaultOfficialReleaseRepository = "z1298808165/Storage-redirection-X-Public"
val defaultReleaseBranch = "SRX-R"

fun gitOriginUrl(): String? {
  return runCatching {
        val process =
            ProcessBuilder("git", "remote", "get-url", "origin")
                .directory(rootProject.projectDir)
                .redirectErrorStream(true)
                .start()
        val output = process.inputStream.bufferedReader().use { it.readText() }.trim()
        if (process.waitFor() == 0) output.takeIf { it.isNotBlank() } else null
      }
      .getOrNull()
}

fun githubRepositoryFromRemote(remoteUrl: String?): String? {
  val normalized = remoteUrl?.trim()?.removeSuffix(".git") ?: return null
  val patterns =
      listOf(
          Regex("""https?://github\.com/([^/]+)/([^/]+)$"""),
          Regex("""git@github\.com:([^/]+)/([^/]+)$"""),
          Regex("""ssh://git@github\.com/([^/]+)/([^/]+)$"""),
      )
  return patterns.firstNotNullOfOrNull { pattern ->
    pattern.matchEntire(normalized)?.let { match ->
      "${match.groupValues[1]}/${match.groupValues[2]}"
    }
  }
}

fun gitCurrentBranch(): String? {
  return runCatching {
        val process =
            ProcessBuilder("git", "branch", "--show-current")
                .directory(rootProject.projectDir)
                .redirectErrorStream(true)
                .start()
        val output = process.inputStream.bufferedReader().use { it.readText() }.trim()
        if (process.waitFor() == 0) output.takeIf { it.isNotBlank() } else null
      }
      .getOrNull()
}

fun rawGitHubFileUrl(repository: String, branch: String, path: String): String =
    "https://raw.githubusercontent.com/$repository/$branch/$path"

val releaseRepository =
    (providers.gradleProperty("srx.releaseRepository").orNull
            ?: providers.environmentVariable("SRX_RELEASE_REPOSITORY").orNull
            ?: providers.environmentVariable("GITHUB_REPOSITORY").orNull
            ?: defaultOfficialReleaseRepository)
        .trim()
val officialReleaseRepository =
    (providers.gradleProperty("srx.officialReleaseRepository").orNull
            ?: providers.environmentVariable("SRX_OFFICIAL_RELEASE_REPOSITORY").orNull
            ?: defaultOfficialReleaseRepository)
        .trim()
val releaseBranch =
    (providers.gradleProperty("srx.releaseBranch").orNull
            ?: providers.environmentVariable("SRX_RELEASE_BRANCH").orNull
            ?: providers.environmentVariable("GITHUB_REF_NAME").orNull?.takeIf {
              providers.environmentVariable("GITHUB_REF_TYPE").orNull == "branch"
            }
            ?: defaultReleaseBranch)
        .trim()
val updateManifestUrl =
    (providers.gradleProperty("srx.updateManifestUrl").orNull
            ?: providers.environmentVariable("SRX_UPDATE_MANIFEST_URL").orNull
            ?: rawGitHubFileUrl(releaseRepository, releaseBranch, "update.json"))
        .trim()

fun buildConfigString(value: String): String =
    "\"${value.replace("\\", "\\\\").replace("\"", "\\\"")}\""

if (!Regex("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$").matches(releaseRepository)) {
  throw GradleException("SRX release repository must be in owner/repo format.")
}

if (!Regex("^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$").matches(officialReleaseRepository)) {
  throw GradleException("SRX official release repository must be in owner/repo format.")
}

if (releaseBranch.isBlank() || releaseBranch.any { it.isISOControl() }) {
  throw GradleException("SRX release branch must not be blank or contain control characters.")
}

if (!Regex("^https?://.+").matches(updateManifestUrl)) {
  throw GradleException("SRX update manifest URL must be an absolute HTTP(S) URL.")
}

data class SigningSetting(val value: String, val baseDir: File)

fun String?.takeIfNotBlank(): String? = this?.takeIf { it.isNotBlank() }

val configuredSigningPropertiesFile =
    providers.environmentVariable("SRX_APP_SIGNING_PROPERTIES").orNull
        ?: providers.gradleProperty("srx.signing.propertiesFile").orNull
val defaultUserSigningPropertiesFile =
    File(
        System.getProperty("user.home"),
        ".srx_core/signing/keystore.properties",
    )
val signingPropertiesFileCandidates =
    listOfNotNull(
        configuredSigningPropertiesFile?.let { rootProject.file(it) },
        rootProject.file("keystore.properties"),
        defaultUserSigningPropertiesFile,
    )
val signingPropertiesFile = signingPropertiesFileCandidates.firstOrNull { it.isFile }
val signingProperties =
    Properties().apply {
      if (signingPropertiesFile != null) {
        signingPropertiesFile.inputStream().use(::load)
      }
    }

fun signingValue(
    propertyName: String,
    envName: String,
    gradlePropertyName: String,
): SigningSetting? =
    providers.environmentVariable(envName).orNull.takeIfNotBlank()?.let {
      SigningSetting(it, rootProject.projectDir)
    }
        ?: providers.gradleProperty(gradlePropertyName).orNull.takeIfNotBlank()?.let {
          SigningSetting(it, rootProject.projectDir)
        }
        ?: signingProperties.getProperty(propertyName).takeIfNotBlank()?.let {
          SigningSetting(it, signingPropertiesFile?.parentFile ?: rootProject.projectDir)
        }

fun resolveSigningFile(setting: SigningSetting): File {
  val file = File(setting.value)
  return if (file.isAbsolute) file else setting.baseDir.resolve(setting.value)
}

val releaseStoreFileSetting =
    signingValue("storeFile", "SRX_APP_SIGNING_STORE_FILE", "srx.signing.storeFile")
val releaseStoreFile = releaseStoreFileSetting?.let(::resolveSigningFile)
val releaseStorePassword =
    signingValue(
            "storePassword",
            "SRX_APP_SIGNING_STORE_PASSWORD",
            "srx.signing.storePassword",
        )
        ?.value
val releaseKeyAlias =
    signingValue("keyAlias", "SRX_APP_SIGNING_KEY_ALIAS", "srx.signing.keyAlias")?.value
val releaseKeyPassword =
    signingValue(
            "keyPassword",
            "SRX_APP_SIGNING_KEY_PASSWORD",
            "srx.signing.keyPassword",
        )
        ?.value
val hasReleaseSigningValues =
    listOf(
            releaseStoreFileSetting?.value,
            releaseStorePassword,
            releaseKeyAlias,
            releaseKeyPassword,
        )
        .all { !it.isNullOrBlank() }

fun isReleaseSigningTaskRequested(): Boolean =
    gradle.startParameter.taskNames.any { taskName ->
      val normalized = taskName.substringAfterLast(':').lowercase()
      normalized == "assemblerelease" ||
          normalized == "bundlerelease" ||
          normalized.startsWith("package") && normalized.endsWith("release")
    }

if (
    hasReleaseSigningValues && releaseStoreFile?.isFile != true && isReleaseSigningTaskRequested()
) {
  throw GradleException("Configured SRX release signing keystore does not exist.")
}

val hasReleaseSigning = hasReleaseSigningValues && releaseStoreFile?.isFile == true

android {
  namespace = "org.srx.manager"

  compileSdk {
    version =
        release(appCompileSdk) {
          minorApiLevel = providers.gradleProperty("srx.compileSdkMinor").orNull?.toIntOrNull()
        }
  }

  defaultConfig {
    applicationId = "org.srx.manager"
    minSdk = 31
    targetSdk = appTargetSdk
    versionCode = appVersionCode
    versionName = appVersionName
    buildConfigField("String", "RELEASE_REPOSITORY", buildConfigString(releaseRepository))
    buildConfigField(
        "String",
        "OFFICIAL_RELEASE_REPOSITORY",
        buildConfigString(officialReleaseRepository),
    )
    buildConfigField("String", "RELEASE_BRANCH", buildConfigString(releaseBranch))
    buildConfigField("String", "UPDATE_MANIFEST_URL", buildConfigString(updateManifestUrl))
  }

  buildFeatures {
    buildConfig = true
    compose = true
  }

  signingConfigs {
    if (hasReleaseSigning) {
      create("release") {
        storeFile = requireNotNull(releaseStoreFile)
        storePassword = releaseStorePassword
        keyAlias = releaseKeyAlias
        keyPassword = releaseKeyPassword
      }
    }
  }

  buildTypes {
    debug { applicationIdSuffix = ".debug" }
    release {
      isMinifyEnabled = true
      isShrinkResources = true
      if (hasReleaseSigning) {
        signingConfig = signingConfigs.getByName("release")
      }
      proguardFiles(
          getDefaultProguardFile("proguard-android-optimize.txt"),
          "proguard-rules.pro",
      )
    }
  }

  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
  }

  kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_21) } }

  packaging {
    resources.excludes +=
        setOf(
            "META-INF/AL2.0",
            "META-INF/LGPL2.1",
        )
  }
}

dependencies {
  implementation(libs.androidx.activity.compose)
  implementation(libs.androidx.core.ktx)
  implementation(libs.androidx.datastore.preferences)
  implementation(libs.androidx.lifecycle.runtime.compose)
  implementation(libs.androidx.lifecycle.runtime.ktx)
  implementation(libs.androidx.lifecycle.viewmodel.compose)
  implementation(libs.androidx.lifecycle.viewmodel.navigation3)
  implementation(libs.androidx.navigation3.runtime)
  implementation(libs.androidx.navigationevent.compose)
  implementation(libs.kotlinx.coroutines.android)
  implementation(libs.kotlinx.serialization.json)

  implementation(platform(libs.androidx.compose.bom))
  implementation(libs.androidx.compose.material.icons.extended)
  implementation(libs.androidx.compose.ui)
  implementation(libs.androidx.compose.ui.tooling.preview)

  implementation(libs.miuix.ui)
  implementation(libs.miuix.icons)
  implementation(libs.miuix.navigation3.ui)
  implementation(libs.miuix.preference)
  implementation(libs.miuix.blur)
  implementation(libs.appiconloader)
  implementation(libs.hiddenapibypass)

  testImplementation(libs.junit)

  debugImplementation(libs.androidx.compose.ui.test.manifest)
  debugImplementation(libs.androidx.compose.ui.tooling)
}

val safeDebugUnitTestDir = gradle.gradleUserHomeDir.resolve("srx-core/testDebugUnitTest")
val safeDebugUnitTestClasses = safeDebugUnitTestDir.resolve("test-classes")
val safeDebugUnitTestRuntime = safeDebugUnitTestDir.resolve("runtime")
val safeDebugUnitTestJavaRes = safeDebugUnitTestDir.resolve("java-res")

val syncDebugUnitTestClasspathForArgfile by
    tasks.registering(Sync::class) {
      duplicatesStrategy = DuplicatesStrategy.EXCLUDE

      dependsOn(
          "compileDebugUnitTestKotlin",
          "compileDebugUnitTestJavaWithJavac",
          "bundleDebugClassesToRuntimeJar",
          "processDebugJavaRes",
          "processDebugResources",
      )

      into(safeDebugUnitTestDir)
      into("test-classes") {
        from(
            layout.buildDirectory.dir(
                "intermediates/built_in_kotlinc/debugUnitTest/compileDebugUnitTestKotlin/classes"
            )
        )
        from(
            layout.buildDirectory.dir(
                "intermediates/javac/debugUnitTest/compileDebugUnitTestJavaWithJavac/classes"
            )
        )
      }
      into("runtime") {
        from(
            layout.buildDirectory.file(
                "intermediates/runtime_app_classes_jar/debug/bundleDebugClassesToRuntimeJar/classes.jar"
            )
        ) {
          rename { "app-classes.jar" }
        }
        from(
            layout.buildDirectory.file(
                "intermediates/compile_and_runtime_r_class_jar/debug/processDebugResources/R.jar"
            )
        )
      }
      into("java-res") {
        from(layout.buildDirectory.dir("intermediates/java_res/debug/processDebugJavaRes/out"))
      }
    }

val testDebugUnitTestSafeClasspath by
    tasks.registering(Test::class) {
      description =
          "Runs manager unit tests from an ASCII classpath for Java argfile compatibility."
      dependsOn(syncDebugUnitTestClasspathForArgfile)

      testClassesDirs = files(safeDebugUnitTestClasses)
      include("org/srx/manager/**/*Test.class")
      classpath =
          files(
              safeDebugUnitTestClasses,
              safeDebugUnitTestRuntime.resolve("app-classes.jar"),
              safeDebugUnitTestRuntime.resolve("R.jar"),
              safeDebugUnitTestJavaRes,
          ) + tasks.named<Test>("testDebugUnitTest").get().classpath
    }

tasks.withType<Test>().configureEach {
  if (name == "testDebugUnitTest") {
    dependsOn(testDebugUnitTestSafeClasspath)
    exclude("org/srx/manager/**/*Test.class")
    failOnNoDiscoveredTests = false
  }
}
