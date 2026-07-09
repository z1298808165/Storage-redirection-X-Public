import org.gradle.api.tasks.testing.Test

plugins { alias(libs.plugins.android.library) }

android {
  namespace = "me.fakerqu.mediafileapi"
  compileSdk { version = release(37) }

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
