// Cadenza on the phone (cadenza/README.md): the Kotlin JNI binding (CadenzaNative) and the Rust library
// libencore_cadenza.so built from cadenza/rust by cargo-ndk. `cargoNdkBuild` runs before preBuild; when cargo-ndk is
// not on the PATH it is SKIPPED with a warning and the APK simply has no Cadenza library (the player then plays
// without sound effects or phone upsampling). CI installs Rust, the Android targets, cargo-ndk 4.1.2 and the NDK, so
// release APKs include the libs for arm64-v8a and armeabi-v7a, with 16 KB aligned segments (rust/build.rs).
import java.io.File

plugins {
    alias(libs.plugins.android.library)
}

val rustDir: File = layout.projectDirectory.dir("rust").asFile
val jniLibsDir: File = layout.buildDirectory.dir("jniLibs").get().asFile
val abis = listOf("arm64-v8a", "armeabi-v7a")

/** `cargo-ndk` (the `cargo ndk` subcommand) on the PATH or in ~/.cargo/bin, or null. */
fun findCargoNdk(): File? {
    val exe = if (System.getProperty("os.name").lowercase().contains("win")) "cargo-ndk.exe" else "cargo-ndk"
    val dirs = (System.getenv("PATH") ?: "").split(File.pathSeparator).filter { it.isNotBlank() } +
        listOf(File(System.getProperty("user.home"), ".cargo/bin").path) +
        listOfNotNull(System.getenv("CARGO_HOME")?.let { File(it, "bin").path })
    return dirs.map { File(it, exe) }.firstOrNull { it.isFile && it.canExecute() }
}

val cargoNdk: File? = findCargoNdk()
val cargo: File? = cargoNdk?.let { File(it.parentFile, it.name.replace("cargo-ndk", "cargo")) }?.takeIf { it.isFile }
    ?: (System.getenv("PATH") ?: "").split(File.pathSeparator).map { File(it, "cargo") }.firstOrNull { it.isFile && it.canExecute() }

android {
    namespace = "app.encore.android.cadenza"
    compileSdk = 36

    defaultConfig {
        minSdk = 26
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDir(jniLibsDir)
        }
    }

}

val cargoNdkBuild = tasks.register<Exec>("cargoNdkBuild") {
    group = "build"
    description = "Builds libencore_cadenza.so for ${abis.joinToString()} with cargo-ndk (skipped without cargo-ndk)"
    inputs.files(fileTree(rustDir) { include("Cargo.toml", "Cargo.lock", "build.rs", "src/**") })
    outputs.dir(jniLibsDir)
    onlyIf {
        if (cargoNdk == null || cargo == null) {
            logger.warn(
                "cargoNdkBuild SKIPPED: cargo-ndk is not on the PATH (or cargo is missing), so this APK has no Cadenza " +
                    "library (libencore_cadenza.so): no sound effects or phone upsampling. Install with: rustup target add " +
                    "aarch64-linux-android armv7-linux-androideabi && cargo install cargo-ndk --version 4.1.2 --locked"
            )
            false
        } else {
            true
        }
    }
    workingDir = rustDir
    doFirst { jniLibsDir.mkdirs() }
    // the same env as the shell; cargo-ndk finds the NDK through ANDROID_NDK_HOME or $ANDROID_HOME/ndk/<newest>
    environment("CARGO_TERM_COLOR", "never")
    val cmd = mutableListOf("ndk")
    for (a in abis) { cmd += "-t"; cmd += a }
    cmd += listOf("-P", "26", "-o", jniLibsDir.path, "build", "--release")
    executable = cargo?.path ?: "cargo"
    setArgs(cmd)
}

tasks.named("preBuild") {
    dependsOn(cargoNdkBuild)
}
