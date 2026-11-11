plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

// The commit and version recorded in the Benchmark JSON.
val repoRoot = rootProject.layout.projectDirectory.dir("../../..").asFile
val commit = providers.exec {
    commandLine("git", "-C", repoRoot.path, "rev-parse", "--short=12", "HEAD")
    isIgnoreExitValue = true
}.standardOutput.asText.map { it.trim().ifEmpty { "unknown" } }
val mnemoVersion = File(repoRoot, "mnemo/bindings/uniffi/Cargo.toml").readLines()
    .first { it.startsWith("version = ") }.substringAfter('"').substringBefore('"')

android {
    namespace = "io.github.toarchkumar.mnemo.sample"
    compileSdk = 37

    defaultConfig {
        applicationId = "io.github.toarchkumar.mnemo.sample"
        minSdk = 24
        targetSdk = 37
        versionCode = 1
        versionName = mnemoVersion
        buildConfigField("String", "MNEMO_COMMIT", "\"${commit.get()}\"")
        buildConfigField("String", "MNEMO_VERSION", "\"$mnemoVersion\"")
        // MNemo ships arm64-v8a and x86_64. JNA's AAR brings its dispatch
        // library for five more ABIs, where the app would install and then
        // fail to load libmnemo_uniffi.so, so keep only the two.
        ndk {
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            // R8: shrinks and obfuscates the app and the library. What JNA
            // and the bindings need comes from the library's
            // consumer-rules.pro; this app adds no rules of its own.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Signed with the debug key so the release APK installs with
            // `adb install` (a sample, not a store build).
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    packaging {
        jniLibs {
            keepDebugSymbols += "**/libmnemo_uniffi.so"
        }
    }
}

dependencies {
    implementation("io.github.toarchkumar:mnemo-android:0.12.3")

    val composeBom = platform("androidx.compose:compose-bom:2026.09.00")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.11.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")

    testImplementation("junit:junit:4.13.2")
}
