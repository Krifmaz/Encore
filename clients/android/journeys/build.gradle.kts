// The user journeys of PERFORMANCE.md, written once and run by two modules: :benchmark (Macrobenchmark measures
// them) and :baselineprofile (records them into the Baseline Profile). Plain UiAutomator over the app's Compose
// test tags (the app sets testTagsAsResourceId, so a tag is a resource id here).
plugins {
    alias(libs.plugins.android.library)
}

android {
    namespace = "app.encore.android.journeys"
    compileSdk = 36

    defaultConfig {
        // Baseline Profile generation needs Android 9+ (API 28); Macrobenchmark itself needs less
        minSdk = 28
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // `api`: the journeys are MacrobenchmarkScope extensions, so both test modules see these types
    api(libs.androidx.benchmark.macro.junit4)
    api(libs.androidx.test.uiautomator)
    api(libs.androidx.test.runner)
}
