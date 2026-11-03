// Encore for Android – the native app (Kotlin, Compose, Media3). See README.md.
// A Gradle build of its own (the rest of the repo is Rust).
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
include(":cadenza") // Cadenza on the phone: JNI binding + libencore_cadenza.so (cargo-ndk), see cadenza/build.gradle.kts
// Performance (PERFORMANCE.md): the user journeys both modules below run, Macrobenchmark's measurements of them,
// and (once added) the Baseline Profile generator
include(":journeys")
include(":benchmark")
