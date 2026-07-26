import org.gradle.api.tasks.Exec

plugins {
  alias(libs.plugins.android.application) apply false
  alias(libs.plugins.android.library) apply false
  alias(libs.plugins.kotlin.compose) apply false
  alias(libs.plugins.kotlin.serialization) apply false
  alias(libs.plugins.spotless)
}

fun findExecutableOnPath(name: String): File? {
  val path = System.getenv("PATH") ?: return null
  val pathExtensions =
      if (System.getProperty("os.name").startsWith("Windows", ignoreCase = true)) {
        (System.getenv("PATHEXT") ?: ".COM;.EXE;.BAT;.CMD").split(';').filter { it.isNotBlank() }
      } else {
        listOf("")
      }
  return path
      .split(File.pathSeparatorChar)
      .asSequence()
      .filter { it.isNotBlank() }
      .flatMap { dir ->
        pathExtensions.asSequence().map { ext -> File(dir, name + ext.lowercase()) }
      }
      .firstOrNull { it.isFile && it.canExecute() }
}

fun configuredExecutable(
    propertyName: String,
    environmentName: String,
    executableName: String,
): File? =
    providers.gradleProperty(propertyName).orNull?.let(::file)
        ?: providers.environmentVariable(environmentName).orNull?.let(::file)
        ?: findExecutableOnPath(executableName)

val spotlessNodeExecutable =
    configuredExecutable(
        propertyName = "srx.spotless.nodeExecutable",
        environmentName = "SPOTLESS_NODE_EXECUTABLE",
        executableName = "node",
    )
val spotlessNpmExecutable =
    configuredExecutable(
        propertyName = "srx.spotless.npmExecutable",
        environmentName = "SPOTLESS_NPM_EXECUTABLE",
        executableName = "npm",
    )

val kotlinSourceFiles =
    files(
        fileTree("app/src") {
          include("**/*.kt")
          exclude("**/build/**", "**/.gradle/**", "**/.kotlin/**")
        },
        fileTree("tests/storage-redirect-test") {
          include("**/*.kt")
          exclude("**/build/**", "**/.gradle/**", "**/.kotlin/**")
        },
    )
val javaSourceFiles =
    files(
        fileTree("java_src") { include("**/*.java") },
        fileTree("tools") { include("**/*.java") },
    )
val kotlinGradleFiles =
    files(
        file("build.gradle.kts"),
        file("settings.gradle.kts"),
        fileTree("app") { include("*.gradle.kts") },
        fileTree("tests/storage-redirect-test") {
          include("**/*.gradle.kts")
          exclude("**/build/**", "**/.gradle/**", "**/.kotlin/**")
        },
    )
val rootDataFiles =
    files(
        projectDir.listFiles().orEmpty().filter { file ->
          file.isFile && file.extension in setOf("json", "toml")
        }
    )
val webAndDataFiles =
    files(
        fileTree("assets") { include("**/*.js", "**/*.json") },
        file(".github/tests/storage-redirect-scenarios.json"),
        fileTree("docs") { include("**/*.json") },
        fileTree("scripts") { include("**/*.js") },
        rootDataFiles,
        fileTree("gradle") { include("**/*.toml") },
        fileTree("vendor") {
          include("**/*.toml")
          exclude("**/target/**")
        },
    )

spotless {
  lineEndings = com.diffplug.spotless.LineEnding.UNIX

  kotlin {
    target(kotlinSourceFiles)
    ktfmt()
    trimTrailingWhitespace()
    endWithNewline()
  }

  java {
    target(javaSourceFiles)
    googleJavaFormat()
    trimTrailingWhitespace()
    endWithNewline()
  }

  kotlinGradle {
    target(kotlinGradleFiles)
    ktfmt()
    trimTrailingWhitespace()
    endWithNewline()
  }

  format("webAndData") {
    target(webAndDataFiles)
    val prettierConfig =
        prettier(
                mapOf(
                    "prettier" to "3.9.4",
                    "prettier-plugin-toml" to "2.0.6",
                ),
            )
            .config(
                mapOf(
                    "plugins" to listOf("prettier-plugin-toml"),
                    "printWidth" to 100,
                    "tabWidth" to 2,
                    "useTabs" to false,
                ),
            )
    spotlessNodeExecutable?.let(prettierConfig::nodeExecutable)
    spotlessNpmExecutable?.let(prettierConfig::npmExecutable)
    trimTrailingWhitespace()
    endWithNewline()
  }
}

val checkNaming by
    tasks.registering(Exec::class) {
      group = "verification"
      description = "Checks package names, interface prefixes, and overly long Kotlin declarations."
      workingDir(projectDir)
      commandLine(spotlessNodeExecutable ?: "node", "scripts/check-kotlin-naming.js")
      inputs.files(
          fileTree(projectDir) {
            include("app/src/**/*.kt")
            include("tests/storage-redirect-test/**/*.kt")
            exclude("**/build/**", "**/.gradle/**", "**/.kotlin/**")
          }
      )
      inputs.file("scripts/check-kotlin-naming.js")
      outputs.upToDateWhen { true }
    }

tasks.named("spotlessCheck") { dependsOn(checkNaming) }
