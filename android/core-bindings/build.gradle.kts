// Android library that wraps the Rust core (crates/urdevice-ffi).
//
// Two generated inputs feed this module, both built from a clean checkout:
//  - jniLibs: `cargo ndk` builds liburdevice_ffi.so for every ABI.
//  - Kotlin:  `uniffi-bindgen` generates the bindings from the host library.
// Both land under build/generated, so nothing generated is committed.

import com.android.build.api.variant.LibraryVariant
import java.io.File
import javax.inject.Inject
import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

plugins {
    alias(libs.plugins.android.library)
}

/** Repository root, which is also the Cargo workspace root. */
val cargoWorkspace: File = rootProject.projectDir.parentFile

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

/** Builds liburdevice_ffi.so for each ABI with cargo-ndk. */
abstract class CargoNdkBuild : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations
    @get:Internal abstract val workspace: DirectoryProperty
    @get:Internal abstract val ndkDir: DirectoryProperty
    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE) abstract val sources: ConfigurableFileCollection
    @get:Input abstract val abis: ListProperty<String>
    @get:Input abstract val release: Property<Boolean>
    @get:Input abstract val minSdk: Property<Int>
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun build() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        val args = mutableListOf("cargo", "ndk", "--platform", minSdk.get().toString(), "-o", out.absolutePath)
        abis.get().forEach { args += listOf("-t", it) }
        args += listOf("build", "--locked", "-p", "urdevice-ffi", "--lib")
        if (release.get()) args += "--release"
        exec.exec {
            workingDir = workspace.get().asFile
            environment("ANDROID_NDK_HOME", ndkDir.get().asFile.absolutePath)
            commandLine(args)
        }
    }
}

/** Generates the Kotlin bindings from a host build of urdevice-ffi. */
abstract class UniFfiBindgen : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations
    @get:Internal abstract val workspace: DirectoryProperty
    @get:InputFiles @get:PathSensitive(PathSensitivity.RELATIVE) abstract val sources: ConfigurableFileCollection
    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val root = workspace.get().asFile
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()

        // The host build is unstripped, so the UniFFI metadata is still in it.
        exec.exec {
            workingDir = root
            commandLine("cargo", "build", "--locked", "-p", "urdevice-ffi", "--lib")
        }
        val os = System.getProperty("os.name").lowercase()
        val libName = when {
            os.contains("windows") -> "urdevice_ffi.dll"
            os.contains("mac") -> "liburdevice_ffi.dylib"
            else -> "liburdevice_ffi.so"
        }
        val targetDir = System.getenv("CARGO_TARGET_DIR")?.let(::File) ?: File(root, "target")
        val library = File(targetDir, "debug/$libName")

        exec.exec {
            workingDir = root
            commandLine(
                "cargo", "run", "--locked", "-q", "-p", "urdevice-ffi",
                "--features", "bindgen", "--bin", "uniffi-bindgen", "--",
                "generate", "--library", library.absolutePath,
                "--language", "kotlin", "--no-format",
                "--out-dir", out.absolutePath,
            )
        }
    }
}

val generateUniFfiBindings = tasks.register<UniFfiBindgen>("generateUniFfiBindings") {
    group = "rust"
    description = "Generates Kotlin bindings for urdevice-ffi with UniFFI."
    workspace.set(cargoWorkspace)
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
            outputDir.set(layout.buildDirectory.dir("generated/rust/${variant.name}/jniLibs"))
        }
        variant.sources.jniLibs?.addGeneratedSourceDirectory(cargoBuild, CargoNdkBuild::outputDir)
        variant.sources.kotlin?.addGeneratedSourceDirectory(generateUniFfiBindings, UniFfiBindgen::outputDir)
    }
}
