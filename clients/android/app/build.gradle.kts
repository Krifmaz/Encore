plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
}

// Version from the release pipeline: -PencoreVersionName=0.30.0 -PencoreVersionCode=260300000 (release.yml: 260000000 +
// major × 1000000 + minor × 10000 + patch × 10, above the old date-based codes).
val encoreVersionName: String = providers.gradleProperty("encoreVersionName").orElse("0.0.1-dev").get()
val encoreVersionCode: Int = providers.gradleProperty("encoreVersionCode").orElse("1").get().toInt()

// Release signing from the environment (CI decodes ANDROID_KEYSTORE_BASE64 to a file and sets the path).
// Without a keystore the release build is signed with the debug key; CI decides whether that is allowed
// (the release pipeline fails when any of the four secrets is missing).
val keystorePath: String? = System.getenv("ANDROID_KEYSTORE_PATH")?.takeIf { it.isNotBlank() }
val hasReleaseKey: Boolean = keystorePath?.let { file(it).isFile } == true

android {
    namespace = "app.encore.android"
    compileSdk = 36

    defaultConfig {
        // Kept from the Tauri app so the native build installs as an update (PLAN §2.1).
        applicationId = "app.encore.client"
        minSdk = 26
        targetSdk = 36
        versionCode = encoreVersionCode
        versionName = encoreVersionName
    }

    signingConfigs {
        create("release") {
            if (hasReleaseKey) {
                storeFile = file(keystorePath!!)
                storePassword = System.getenv("ANDROID_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("ANDROID_KEY_ALIAS")
                keyPassword = System.getenv("ANDROID_KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        getByName("release") {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = if (hasReleaseKey) signingConfigs.getByName("release") else signingConfigs.getByName("debug")
        }
        getByName("debug") {
            applicationIdSuffix = ".debug"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    lint {
        // Media3 marks parts of its API @UnstableApi (an androidx opt-in that only lint enforces); we use
        // them on purpose (OkHttpDataSource, bitmap loaders, playback resumption).
        disable += "UnsafeOptInUsageError"
    }

    packaging {
        resources {
            excludes += "/META-INF/{AL2.0,LGPL2.1}"
        }
    }
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.activity.compose)

    implementation(libs.androidx.media3.exoplayer)
    implementation(libs.androidx.media3.session)
    implementation(libs.androidx.media3.datasource.okhttp)
    // Encore Lyrics: a template car app (car/LyricsCarAppService.kt) beside the media app
    implementation(libs.androidx.car.app)

    implementation(libs.androidx.datastore.preferences)
    implementation(libs.okhttp)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
    implementation(libs.coil.compose)
    implementation(libs.coil.network.okhttp)
    implementation(libs.coil.network.cache.control)   // covers follow the server's Cache-Control / ETag

    // Cadenza on the phone (JNI binding + libencore_cadenza.so when built with cargo-ndk); CadenzaEngine adapts it
    implementation(project(":cadenza"))

    // JUnit 4 for app/src/test (pure Kotlin rules; no Android classes, so no Robolectric). Not in the APK.
    testImplementation(libs.junit)
}
