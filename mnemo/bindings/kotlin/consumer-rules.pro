# Applied to apps that depend on mnemo-android when they minify with R8.

# JNA binds native methods, callbacks and Structure fields by name and by
# reflection (rules from JNA's documentation).
-dontwarn java.awt.**
-keep class com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }

# The UniFFI bindings: JNA reads their Structure fields and callback
# interfaces reflectively.
-keep class io.github.toarchkumar.mnemo.** { *; }
