// The release dry run's Gradle consumer (MOBILE-1e): resolves the signed
// mnemo-android artifact from mavenLocal() (run the library's signed
// publishToMavenLocal first) and runs a JVM smoke test against it.
//
//   ../gradlew -p release-check test -PmnemoVersion=0.12.0 \
//     -Pjna.library.path=$PWD/generated/host
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenLocal {
            content { includeGroup("io.github.toarchkumar") }
        }
        google()
        mavenCentral()
    }
}

rootProject.name = "mnemo-release-check"
