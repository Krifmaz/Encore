// The Baseline Profile generator (PERFORMANCE.md, README "Baseline Profile"): records the classes and methods the
// journeys in :journeys use into app/src/main/generated/baselineProfiles (baseline-prof.txt, and startup-prof.txt for
// the startup DEX layout). The release build compiles them into the APK (assets/dexopt/baseline.prof), and
// profileinstaller installs them on first launch, so an APK installed by hand or by the updater starts compiled too.
// Regenerate on a phone or an emulator (Android 9+; 13+ without root) with:
//   gradle -p clients/android :app:generateBaselineProfile
plugins {
    alias(libs.plugins.android.test)
    alias(libs.plugins.baselineprofile)
}

android {
    namespace = "app.encore.android.baselineprofile"
    compileSdk = 36

    defaultConfig {
        minSdk = 28
        targetSdk = 36
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    targetProjectPath = ":app"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

baselineProfile {
    // the phone (or emulator) plugged in, not a Gradle managed device
    useConnectedDevices = true
}

dependencies {
    implementation(project(":journeys"))
    implementation(libs.androidx.test.ext.junit)
}
