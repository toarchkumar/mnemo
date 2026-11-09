// The MNemo Android library (MOBILE-1c). Every version here is exact.
//
// Before building, run scripts/build-native.sh. It writes three things
// under generated/ (all gitignored):
//   generated/kotlin/   the UniFFI Kotlin bindings
//   generated/jniLibs/  libmnemo_uniffi.so for arm64-v8a and x86_64 (16 KB pages)
//   generated/host/     the host library the JVM tests load through JNA

import com.vanniktech.maven.publish.AndroidSingleVariantLibrary
import com.vanniktech.maven.publish.JavadocJar
import com.vanniktech.maven.publish.SourcesJar

buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        // AGP 9 compiles Kotlin itself (built-in Kotlin) with the Kotlin
        // Gradle plugin on the classpath; this pins a current one instead
        // of AGP's minimum (2.2.10), so libraries built with newer Kotlin
        // (coroutines, lifecycle) are readable.
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.21")
    }
}

plugins {
    id("com.android.library") version "9.5.1"
    id("com.vanniktech.maven.publish") version "0.37.0"
}

val jnaVersion = "5.19.1"
val coroutinesVersion = "1.11.0"
val generated = layout.projectDirectory.dir("generated")

android {
    namespace = "io.github.toarchkumar.mnemo"
    compileSdk = 37

    defaultConfig {
        minSdk = 24
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        consumerProguardFiles("consumer-rules.pro")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        named("main") {
            kotlin.directories += "generated/kotlin"
            jniLibs.directories += "generated/jniLibs"
        }
        // Test doubles (an in-memory key store, embedders) shared by the JVM
        // and the instrumented tests.
        named("test") {
            kotlin.directories += "src/sharedTest/kotlin"
        }
        named("androidTest") {
            kotlin.directories += "src/sharedTest/kotlin"
        }
    }

    packaging {
        jniLibs {
            // scripts/build-native.sh ships them stripped already.
            keepDebugSymbols += "**/libmnemo_uniffi.so"
        }
    }

    testOptions {
        targetSdk = 37
    }

    lint {
        targetSdk = 37
    }
}

dependencies {
    // The generated bindings expose JNA types (Pointer, Structure) publicly.
    // The @aar artifact carries JNA's Android native dispatch library.
    api("net.java.dev.jna:jna:$jnaVersion@aar")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:$coroutinesVersion")
    implementation("androidx.lifecycle:lifecycle-process:2.11.0")

    // JVM tests: the plain JNA jar, which carries the desktop dispatch
    // libraries, loads the host libmnemo_uniffi from generated/host.
    testImplementation("net.java.dev.jna:jna:$jnaVersion")
    testImplementation("junit:junit:4.13.2")

    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test:core:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("junit:junit:4.13.2")
}

// A clear error instead of a wall of unresolved references.
val checkNativeOutputs by tasks.registering {
    val bindings = generated.dir("kotlin").asFile
    doLast {
        check(bindings.walkTopDown().any { it.extension == "kt" }) {
            "No generated bindings in $bindings: run scripts/build-native.sh first."
        }
    }
}
tasks.named("preBuild") { dependsOn(checkNativeOutputs) }

tasks.withType<Test>().configureEach {
    val host = generated.dir("host").asFile
    systemProperty("jna.library.path", host.absolutePath)
    doFirst {
        check(host.isDirectory) { "No host library in $host: run scripts/build-native.sh first." }
    }
    testLogging {
        events("passed", "skipped", "failed")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}

// Maven Central (the upload itself is MOBILE-1e). The CI `kotlin` job runs
// publishToMavenLocal, unsigned, and checks the POM. Signing turns on when
// the in-memory key is present; the release workflow maps the repository
// secrets to the plugin's properties:
//   MAVEN_CENTRAL_USERNAME     -> ORG_GRADLE_PROJECT_mavenCentralUsername
//   MAVEN_CENTRAL_PASSWORD     -> ORG_GRADLE_PROJECT_mavenCentralPassword
//   MAVEN_SIGNING_KEY          -> ORG_GRADLE_PROJECT_signingInMemoryKey
//   MAVEN_SIGNING_KEY_PASSWORD -> ORG_GRADLE_PROJECT_signingInMemoryKeyPassword
mavenPublishing {
    configure(
        AndroidSingleVariantLibrary(
            javadocJar = JavadocJar.Empty(),
            sourcesJar = SourcesJar.Sources(),
            variant = "release",
        ),
    )
    coordinates(
        property("GROUP").toString(),
        "mnemo-android",
        property("VERSION_NAME").toString(),
    )
    publishToMavenCentral()
    if (providers.gradleProperty("signingInMemoryKey").isPresent) {
        signAllPublications()
    }
    pom {
        name = "MNemo for Android"
        description = "An encrypted, single-file agent memory for Android apps: " +
            "vectors, text, metadata and conversation threads in one portable .mnemo file."
        inceptionYear = "2026"
        url = "https://github.com/toarchkumar/mnemo"
        licenses {
            license {
                name = "The Apache License, Version 2.0"
                url = "https://www.apache.org/licenses/LICENSE-2.0.txt"
                distribution = "repo"
            }
        }
        developers {
            developer {
                id = "toarchkumar"
                name = "Arch Kumar"
                url = "https://github.com/toarchkumar"
            }
        }
        scm {
            url = "https://github.com/toarchkumar/mnemo"
            connection = "scm:git:https://github.com/toarchkumar/mnemo.git"
            developerConnection = "scm:git:ssh://git@github.com/toarchkumar/mnemo.git"
        }
    }
}
