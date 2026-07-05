@file:Suppress("UnstableApiUsage")

enableFeaturePreview("TYPESAFE_PROJECT_ACCESSORS")

pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
        maven("https://maven.aliyun.com/repository/google")
        maven("https://maven.aliyun.com/repository/gradle-plugin")
        maven("https://maven.aliyun.com/repository/central")
        maven("https://mirrors.cloud.tencent.com/nexus/repository/maven-public/")
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        exclusiveContent {
            forRepository {
                mavenCentral()
            }
            filter {
                includeGroup("top.yukonga.miuix.kmp")
            }
        }
        google()
        mavenCentral()
        maven("https://maven.aliyun.com/repository/google")
        maven("https://maven.aliyun.com/repository/central")
        maven("https://mirrors.cloud.tencent.com/nexus/repository/maven-public/")
    }
}

rootProject.name = "StorageRedirectX"
include(":app")
include(":storageRedirectTestMediaFileApi")
project(":storageRedirectTestMediaFileApi").projectDir = file("tests/storage-redirect-test/media-file-api")
include(":storageRedirectTestApp")
project(":storageRedirectTestApp").projectDir = file("tests/storage-redirect-test/app")
