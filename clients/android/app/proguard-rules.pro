# R8 rules for Encore. Libraries ship their own consumer rules (OkHttp, Coil, Media3, kotlinx.serialization);
# these are the belt-and-braces ones for what we reflect on ourselves.

# ---- kotlinx.serialization (from the library's README) ----
-keepattributes RuntimeVisibleAnnotations,AnnotationDefault,InnerClasses,Signature

# Keep `Companion` object fields of serializable classes.
-if @kotlinx.serialization.Serializable class **
-keepclassmembers class <1> {
    static <1>$Companion Companion;
}

# Keep `serializer()` on companion objects (both default and named) of serializable classes.
-if @kotlinx.serialization.Serializable class ** {
    static **$* *;
}
-keepclassmembers class <2>$<3> {
    kotlinx.serialization.KSerializer serializer(...);
}

# Keep `INSTANCE.serializer()` of serializable objects.
-if @kotlinx.serialization.Serializable class ** {
    public static ** INSTANCE;
}
-keepclassmembers class <1> {
    public static <1> INSTANCE;
    kotlinx.serialization.KSerializer serializer(...);
}

# Our own @Serializable request/response classes.
-keep,includedescriptorclasses class app.encore.android.**$$serializer { *; }
-keepclassmembers @kotlinx.serialization.Serializable class app.encore.android.** {
    *** Companion;
    kotlinx.serialization.KSerializer serializer(...);
    <fields>;
}

# ---- OkHttp optional platform integrations ----
-dontwarn okhttp3.internal.platform.**
-dontwarn org.conscrypt.**
-dontwarn org.bouncycastle.**
-dontwarn org.openjsse.**

# ---- JNI (Cadenza, from M2 on) ----
-keepclasseswithmembernames class * {
    native <methods>;
}
