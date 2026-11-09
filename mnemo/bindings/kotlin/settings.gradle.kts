// The MNemo Android library (MOBILE-1c): one Gradle project, published as
// io.github.toarchkumar:mnemo-android. Build the native parts first with
// scripts/build-native.sh (see README.md).
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "mnemo-android"
