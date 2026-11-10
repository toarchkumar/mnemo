buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.21")
    }
}

plugins {
    id("com.android.library") version "9.5.1"
}

val mnemoVersion = providers.gradleProperty("mnemoVersion").get()
val hostLibrary = providers.gradleProperty("jna.library.path").get()

android {
    namespace = "io.github.toarchkumar.mnemo.check"
    compileSdk = 37
    defaultConfig { minSdk = 24 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    testOptions { targetSdk = 37 }
    lint { targetSdk = 37 }
}

dependencies {
    // Resolved from mavenLocal(): the published AAR and its POM, which
    // brings JNA (the @aar) and coroutines.
    implementation("io.github.toarchkumar:mnemo-android:$mnemoVersion")
    // JVM tests load the host library through the desktop JNA jar.
    testImplementation("net.java.dev.jna:jna:5.19.1")
    testImplementation("junit:junit:4.13.2")
}

tasks.withType<Test>().configureEach {
    systemProperty("jna.library.path", hostLibrary)
    testLogging { events("passed", "failed") }
}

// Proof of where the artifact came from.
tasks.register("printMnemoArtifact") {
    val files = configurations.named("debugCompileClasspath")
    doLast {
        files.get().incoming.artifactView { lenient(true) }.artifacts.artifacts
            .filter { it.id.componentIdentifier.displayName.startsWith("io.github.toarchkumar:mnemo-android") }
            .forEach { println("resolved ${it.id.componentIdentifier.displayName}") }
    }
}
