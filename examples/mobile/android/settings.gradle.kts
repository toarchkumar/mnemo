// The MNemo Android sample (MOBILE-1d). It builds against the local library
// in mnemo/bindings/kotlin as an included build, so run that project's
// scripts/build-native.sh first (see examples/mobile/README.md).
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

includeBuild("../../../mnemo/bindings/kotlin") {
    dependencySubstitution {
        substitute(module("io.github.toarchkumar:mnemo-android")).using(project(":"))
    }
}

rootProject.name = "mnemo-sample"
include(":app")
