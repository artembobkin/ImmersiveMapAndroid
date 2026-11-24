plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.compose)
}

// The ABIs the engine is built for: 64-bit devices, and the emulator of an Intel machine.
val engineAbis = listOf("arm64-v8a", "x86_64")
val engineDirectory = rootProject.layout.projectDirectory.dir("engine")
val engineLibraries = layout.buildDirectory.dir("engine/jniLibs")

android {
    namespace = "io.github.artembobkin.immersivemap"
    compileSdk {
        version = release(37) {
            minorApiLevel = 2
        }
    }
    ndkVersion = "28.2.13676358"

    defaultConfig {
        // Android 10: Vulkan 1.1 on every 64-bit device.
        minSdk = 29
        ndk {
            abiFilters += engineAbis
        }
    }

    buildFeatures {
        compose = true
    }

    sourceSets {
        getByName("main") {
            jniLibs.directories.add(engineLibraries.get().asFile.path)
        }
    }
}

// The engine is Rust (`engine/`), built for Android by cargo-ndk into the
// library's native libraries. `cargo install cargo-ndk` and the Rust targets
// of `engineAbis` are needed once (README.md).
val buildEngine = tasks.register<Exec>("buildEngine") {
    description = "Builds the Rust engine for Android with cargo-ndk."
    group = "build"
    workingDir = engineDirectory.asFile
    inputs.dir(engineDirectory.dir("src"))
    inputs.file(engineDirectory.file("Cargo.toml"))
    inputs.file(engineDirectory.file("Cargo.lock"))
    outputs.dir(engineLibraries)
    // Gradle's daemon does not always inherit a shell's PATH: cargo is looked for where rustup puts it too.
    val home = System.getProperty("user.home")
    environment("PATH", listOf("$home/.cargo/bin", "/opt/homebrew/opt/rustup/bin", System.getenv("PATH")).joinToString(":"))
    val ndkDirectory = androidComponents.sdkComponents.ndkDirectory
    doFirst {
        environment("ANDROID_NDK_HOME", ndkDirectory.get().asFile.absolutePath)
    }
    commandLine(
        listOf("cargo", "ndk") + engineAbis.flatMap { listOf("-t", it) } +
            listOf("--platform", "29", "-o", engineLibraries.get().asFile.path, "build", "--release"),
    )
}

tasks.named("preBuild") {
    dependsOn(buildEngine)
}

dependencies {
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
}
