import org.gradle.api.tasks.testing.Test

plugins { alias(libs.plugins.android.library) }

android {
  namespace = "me.fakerqu.mediafileapi"
  // Miuix 0.9.5 的 AAR 要求 compileSdk 37.2，与主 App 的 srx.compileSdkMinor 对齐。
  compileSdk {
    version =
        release(37) {
          minorApiLevel = providers.gradleProperty("srx.compileSdkMinor").orNull?.toIntOrNull() ?: 2
        }
  }

  defaultConfig {
    minSdk = 31

    testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
  }
  compileOptions {
    sourceCompatibility = JavaVersion.VERSION_21
    targetCompatibility = JavaVersion.VERSION_21
  }
  kotlin { compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_21) } }
}

dependencies {
  implementation(libs.androidx.core.ktx)
  testImplementation(libs.junit)
  androidTestImplementation(libs.androidx.espresso.core)
  androidTestImplementation(libs.androidx.junit)
}

tasks.withType<Test>().configureEach { failOnNoDiscoveredTests = false }
