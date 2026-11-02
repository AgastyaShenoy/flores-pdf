import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "io.github.agastyashenoy.flores"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.agastyashenoy.flores"
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "1.0.0-beta.1"
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Signed with the debug key so it installs for testing; a store build needs a real key.
            signingConfig = signingConfigs.getByName("debug")
            // Phones and tablets run ARM; the emulator's x86-64 engine only goes in with -Pemulator.
            ndk {
                abiFilters += if (project.hasProperty("emulator")) listOf("arm64-v8a", "x86_64") else listOf("arm64-v8a")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // libflores.so, the engine, is built by ../build-rust.ps1 into src/main/jniLibs.
    packaging {
        jniLibs.useLegacyPackaging = false
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

// No AndroidX or other libraries: only the platform's own classes, to stay small.
dependencies {
}
