# R8 rules for the release build.

# JNA finds classes, fields and methods by reflection and from native code.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
# JNA's desktop parts reference AWT, which Android doesn't have.
-dontwarn java.awt.**

# UniFFI's generated bindings: JNA maps their structures and callbacks by
# name, and Rust calls the callback interfaces through them.
-keep class io.github.deathedit.tether.core.** { *; }

# JNI functions in libtether_ffi.so are found by class and method name
# (NativeSurface).
-keepclasseswithmembernames,includedescriptorclasses class * {
    native <methods>;
}
