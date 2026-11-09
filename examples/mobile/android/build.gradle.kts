// Every version here is exact. AGP 9 compiles Kotlin itself (built-in
// Kotlin); the Kotlin Gradle plugin and the Compose compiler plugin are on
// the classpath at the version the library uses.
buildscript {
    repositories {
        google()
        mavenCentral()
    }
    dependencies {
        classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:2.4.21")
        classpath("org.jetbrains.kotlin:compose-compiler-gradle-plugin:2.4.21")
    }
}

plugins {
    id("com.android.application") version "9.5.1" apply false
}
