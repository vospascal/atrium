plugins {
    kotlin("multiplatform") version "2.4.21"
    kotlin("plugin.compose") version "2.4.21"
    id("org.jetbrains.compose") version "1.12.2"
    id("dev.oxikt.gradle.plugin") version "1.4.0"
}

/** The Atrium repository root: scene files resolve against it. */
val atriumRoot: File = rootDir.resolve("../..").canonicalFile

cargo {
    packageDirectory = layout.projectDirectory.dir("../../crates/ffi")
}

uniffi {
    generateFromLibrary {
        packageName = "atrium.engine"
    }
}

kotlin {
    jvm()
    jvmToolchain(21)

    listOf(iosArm64(), iosSimulatorArm64()).forEach { target ->
        target.binaries.framework {
            baseName = "AtriumApp"
            isStatic = true
        }
    }

    sourceSets {
        commonMain.dependencies {
            implementation("org.jetbrains.compose.foundation:foundation:1.12.2")
            implementation("org.jetbrains.compose.ui:ui:1.12.2")
        }
        commonTest.dependencies {
            implementation(kotlin("test"))
        }
        jvmMain.dependencies {
            implementation(compose.desktop.currentOs)
        }
    }
}

compose.desktop {
    application {
        mainClass = "atrium.app.MainKt"
    }
}

// The engine opens scene files relative to the working directory, when the app runs and in tests.
tasks.withType<JavaExec>().configureEach {
    workingDir = atriumRoot
}
tasks.withType<Test>().configureEach {
    workingDir = atriumRoot
}
