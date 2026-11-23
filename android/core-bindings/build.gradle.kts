// Android library that wraps the Rust core (crates/urdevice-ffi).
//
// Two generated inputs feed this module, both built from a clean checkout:
//  - jniLibs: `cargo ndk` builds liburdevice_ffi.so for every ABI.
//  - Kotlin:  `uniffi-bindgen` generates the bindings from the host library.
// Both land under build/generated, so nothing generated is committed.

import com.android.build.api.variant.LibraryVariant
import com.urdevice.build.tasks.CargoNdkBuild
import com.urdevice.build.tasks.UniFfiBindgen
import java.io.File

plugins {
    alias(libs.plugins.android.library)
}

/** Repository root, which is also the Cargo workspace root. */
val cargoWorkspace: File = rootProject.projectDir.parentFile

/** Path to cargo binary. */
val cargoPath: String =
    File(System.getProperty("user.home"), ".cargo/bin/cargo").let {
        if (it.exists()) it.absolutePath else "cargo"
    }

/** ABIs to build. Override with `-Purdevice.abis=arm64-v8a` for faster local builds. */
val rustAbis: List<String> =
    (findProperty("urdevice.abis") as String? ?: "arm64-v8a,armeabi-v7a,x86_64")
        .split(',')
        .map(String::trim)
        .filter(String::isNotEmpty)

android {
    namespace = "com.urdevice.core"
    compileSdk = libs.versions.compile.sdk.get().toInt()
    ndkVersion = libs.versions.ndk.get()

    defaultConfig {
        minSdk = libs.versions.min.sdk.get().toInt()
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // UniFFI's Kotlin bindings load the native library through JNA.
    api("${libs.jna.get()}@aar")
}

/** Rust sources that the native library and the bindings depend on. */
fun ConfigurableFileCollection.rustSources() {
    from(File(cargoWorkspace, "Cargo.toml"), File(cargoWorkspace, "Cargo.lock"))
    from(fileTree(File(cargoWorkspace, "crates")) { include("**/*.rs", "**/Cargo.toml", "**/*.toml") })
}

val generateUniFfiBindings = tasks.register<UniFfiBindgen>("generateUniFfiBindings") {
    group = "rust"
    description = "Generates Kotlin bindings for urdevice-ffi with UniFFI."
    workspace.set(cargoWorkspace)
    cargoBin.set(cargoPath)
    sources.rustSources()
    outputDir.set(layout.buildDirectory.dir("generated/uniffi/kotlin"))
}

androidComponents {
    val ndk = sdkComponents.ndkDirectory
    onVariants { variant: LibraryVariant ->
        val name = variant.name.replaceFirstChar(Char::uppercase)
        val cargoBuild = tasks.register<CargoNdkBuild>("cargoNdkBuild$name") {
            group = "rust"
            description = "Builds urdevice-ffi for $rustAbis with cargo-ndk (${variant.name})."
            workspace.set(cargoWorkspace)
            ndkDir.set(ndk)
            sources.rustSources()
            abis.set(rustAbis)
            release.set(variant.buildType == "release")
            minSdk.set(libs.versions.min.sdk.get().toInt())
            cargoBin.set(cargoPath)
            outputDir.set(layout.buildDirectory.dir("generated/rust/${variant.name}/jniLibs"))
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoBuild, CargoNdkBuild::outputDir)
        variant.sources.kotlin?.addGeneratedSourceDirectory(generateUniFfiBindings, UniFfiBindgen::outputDir)
    }
}
