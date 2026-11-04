// Macrobenchmark of the release app (PERFORMANCE.md): startup time and frame timing of the journeys in :journeys.
// The Baseline Profile plugin gives this module the app's `benchmarkRelease` variant (minified like release, signed
// with the debug key, not debuggable), so the numbers are a release build's. Run on a phone with:
//   gradle -p clients/android :benchmark:connectedBenchmarkReleaseAndroidTest
// (PERFORMANCE.md lists the instrumentation arguments: the server to sign in to, the compilation mode, iterations).
plugins {
    alias(libs.plugins.android.test)
    alias(libs.plugins.baselineprofile)
}

android {
    namespace = "app.encore.android.benchmark"
    compileSdk = 36

    defaultConfig {
        minSdk = 28
        targetSdk = 36
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // Macrobenchmark errors to let pass, e.g. -Pencore.suppressErrors=EMULATOR,DEBUGGABLE (benchmark/ci/run.sh).
        // Set here because a comma-separated -Pandroid.testInstrumentationRunnerArguments value is cut at the comma.
        providers.gradleProperty("encore.suppressErrors").orNull?.let {
            testInstrumentationRunnerArguments["androidx.benchmark.suppressErrors"] = it
        }
    }

    // the test APK instruments itself and drives the app over UiAutomator (a Macrobenchmark must not run in the
    // app's own process)
    targetProjectPath = ":app"
    experimentalProperties["android.experimental.self-instrumenting"] = true

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
