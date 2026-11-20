# UniFFI bindings call into Rust through JNA, which finds classes, fields and
# methods by reflection. R8 must not rename or remove them in release builds.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.Structure { *; }
-dontwarn java.awt.**
-dontwarn com.sun.jna.**

# Generated UniFFI Kotlin bindings (package from crates/urdevice-ffi).
-keep class com.urdevice.core.** { *; }
