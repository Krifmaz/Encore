// Encore for Android – the native app (Kotlin, Compose, Media3). See docs/android-native/PLAN.md.
// A separate Gradle build from the Tauri client in clients/app.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.PREFER_SETTINGS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "encore-android"
include(":app")
