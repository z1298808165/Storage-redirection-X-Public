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

spotless {
  kotlin {
    target(
        "app/src/**/*.kt",
        "tests/storage-redirect-test/**/*.kt",
    )
    targetExclude(
        "**/build/**",
        "**/.gradle/**",
        "**/.kotlin/**",
    )
    ktfmt()
    trimTrailingWhitespace()
    endWithNewline()
  }

  java {
    target(
        "java_src/**/*.java",
        "tools/**/*.java",
    )
    googleJavaFormat()
    trimTrailingWhitespace()
    endWithNewline()
  }

  kotlinGradle {
    target("*.gradle.kts", "app/*.gradle.kts", "tests/storage-redirect-test/**/*.gradle.kts")
    targetExclude(
        "**/build/**",
        "**/.gradle/**",
        "**/.kotlin/**",
    )
    ktfmt()
    trimTrailingWhitespace()
    endWithNewline()
  }

  format("webAndData") {
    target(
        "assets/**/*.js",
        "assets/**/*.json",
        "docs/**/*.json",
        "scripts/**/*.js",
        "*.json",
        "*.toml",
        "gradle/**/*.toml",
        "vendor/**/*.toml",
    )
    targetExclude(
        "**/build/**",
        "**/target/**",
        "**/.gradle/**",
        "**/.kotlin/**",
    )
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
    tasks.registering {
      group = "verification"
      description = "Checks package names, interface prefixes, and overly long Kotlin declarations."

      doLast {
        val checkedFiles =
            fileTree(projectDir) {
              include("app/src/**/*.kt")
              include("tests/storage-redirect-test/**/*.kt")
              exclude("**/build/**")
              exclude("**/.gradle/**")
              exclude("**/.kotlin/**")
            }
        val packageWithUnderscore = Regex("""(?m)^\s*package\s+\S*_\S*""")
        val prefixedInterface = Regex("""\binterface\s+I[A-Z][A-Za-z0-9_]*\b""")
        val longDeclaration =
            Regex("""\b(class|object|interface|fun|val|var)\s+([A-Za-z0-9_]{51,})\b""")
        val violations = mutableListOf<String>()

        checkedFiles.files
            .sortedBy { it.invariantSeparatorsPath }
            .forEach { sourceFile ->
              val text = sourceFile.readText()
              val relativePath = sourceFile.relativeTo(projectDir).invariantSeparatorsPath
              packageWithUnderscore.findAll(text).forEach { match ->
                violations +=
                    "$relativePath: package names must not contain underscores (${match.value.trim()})"
              }
              prefixedInterface.findAll(text).forEach { match ->
                violations += "$relativePath: avoid I-prefixed interfaces (${match.value})"
              }
              longDeclaration.findAll(text).forEach { match ->
                violations +=
                    "$relativePath: declaration name is too long (${match.groupValues[2]})"
              }
            }

        if (violations.isNotEmpty()) {
          throw GradleException(violations.joinToString(separator = System.lineSeparator()))
        }
      }
    }

tasks.named("spotlessCheck") { dependsOn(checkNaming) }
