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
        versionCode = 2
        versionName = "1.0.0-beta.2"
    }

    // Two interfaces over the same engine, to compare: Material 3 (Google's components, AndroidX)
    // and custom (platform views only, as Flax Reader does). They install side by side.
    flavorDimensions += "ui"
    productFlavors {
        create("material") {
            dimension = "ui"
            applicationIdSuffix = ".material"
            resValue("string", "app_name", "Flores Material")
        }
        create("custom") {
            dimension = "ui"
            applicationIdSuffix = ".custom"
            resValue("string", "app_name", "Flores Custom")
        }
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

// The material interface: Material 3, which brings AppCompat and the rest of AndroidX it needs. The
// custom one uses none.
dependencies {
    "materialImplementation"("com.google.android.material:material:1.12.0")
    "materialImplementation"("androidx.appcompat:appcompat:1.7.0")
    "materialImplementation"("androidx.activity:activity:1.9.3")
    "materialImplementation"("androidx.recyclerview:recyclerview:1.3.2")
}
