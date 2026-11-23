package com.urdevice.build.tasks

import java.io.File
import javax.inject.Inject
import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations

abstract class UniFfiBindgen : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations
    @get:Internal abstract val workspace: DirectoryProperty
    @get:Input abstract val cargoBin: Property<String>
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
            commandLine(cargoBin.get(), "build", "--locked", "-p", "urdevice-ffi", "--lib")
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
                cargoBin.get(), "run", "--locked", "-q", "-p", "urdevice-ffi",
                "--features", "bindgen", "--bin", "uniffi-bindgen", "--",
                "generate", "--library", library.absolutePath,
                "--language", "kotlin", "--no-format",
                "--out-dir", out.absolutePath,
            )
        }
    }
}
