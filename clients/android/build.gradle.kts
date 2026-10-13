// AGP 9 compiles Kotlin itself ("built-in Kotlin"), so org.jetbrains.kotlin.android is NOT applied anywhere.
// AGP 9 brings Kotlin Gradle plugin 2.2.10 at runtime; the classpath entry below raises it to the Kotlin we
// pin in gradle/libs.versions.toml (the Compose compiler plugin must match the Kotlin version exactly).
// Keep the version here in sync with `kotlin` in libs.versions.toml.
buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.21")
    }
}

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.kotlin.compose) apply false
    alias(libs.plugins.kotlin.serialization) apply false
}
