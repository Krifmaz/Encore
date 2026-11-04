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

// kit-compose (Krifmaz/kit, android/kit-compose): Encore's look and ease-of-use features as a shared library. It is
// built from the kit's source at the commit or tag in kit.properties, checked out once into build/kit/<rev>
// (git, no shell) and included here, so Gradle builds dev.krifmaz.kit:kit-compose from it: no GitHub Packages token
// needed on CI or on a laptop. -PkitDir=<path to a kit checkout> uses a local checkout instead (kit development).
val kitRev: String = java.util.Properties().apply { file("kit.properties").inputStream().use { load(it) } }.getProperty("rev")
    ?: error("kit.properties has no rev")
val kitDir: File = providers.gradleProperty("kitDir").orNull?.let { file(it) } ?: file("build/kit/$kitRev").also { dir ->
    if (!File(dir, "android/settings.gradle.kts").isFile) {
        val tmp = file("build/kit/.$kitRev.tmp")
        tmp.deleteRecursively()
        tmp.mkdirs()
        fun git(vararg args: String) {
            val p = ProcessBuilder(listOf("git", "-C", tmp.path) + args).inheritIO().start()
            check(p.waitFor() == 0) { "Couldn't get the kit at $kitRev: git ${args.joinToString(" ")} failed" }
        }
        git("init", "--quiet")
        git("fetch", "--quiet", "--depth", "1", "https://github.com/Krifmaz/kit.git", kitRev)
        git("checkout", "--quiet", "FETCH_HEAD")
        dir.deleteRecursively()
        check(tmp.renameTo(dir)) { "Couldn't move the kit into $dir" }
    }
}
includeBuild(File(kitDir, "android"))
