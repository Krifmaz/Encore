# JNI: the Rust library links to these names, R8 must keep them (and the class) as they are.
-keep class app.encore.android.cadenza.CadenzaNative { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
