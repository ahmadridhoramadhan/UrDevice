# JNA and the UniFFI bindings are called through reflection and native code.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keep class com.urdevice.core.** { *; }
-dontwarn java.awt.**
