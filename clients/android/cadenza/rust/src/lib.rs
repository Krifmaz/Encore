//! `libencore_cadenza.so`: cadenza-dsp on the phone, behind the JNI methods of
//! `app.encore.android.cadenza.CadenzaNative` (clients/android/cadenza/src/main/java/…/CadenzaNative.kt).
//!
//! The engine itself ([`engine::Engine`]) is plain Rust and is what `cargo test` covers on the host; the JNI
//! functions below only move data across. Every native method is an *instance* method of `CadenzaNative`, so the
//! second argument is the Kotlin object (unused). A handle is a `Box<Mutex<Engine>>` pointer as a `jlong`; 0 is
//! "no engine" and every function tolerates it. Nothing here unwinds into the JVM: panics turn into 0 / an empty
//! array / an empty string.

pub mod engine;

pub use engine::{catalog_json, Engine, Params};

use jni::objects::{JClass, JFloatArray, JObject, JString};
use jni::sys::{jfloatArray, jint, jlong, jstring};
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Mutex;

type Handle = Mutex<Engine>;

fn with_engine<R>(handle: jlong, f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
    if handle == 0 {
        return None;
    }
    // SAFETY: the handle came from nativeCreate (Box::into_raw) and is only freed by nativeDestroy, after which
    // the Kotlin side sets it to 0 and never uses it again.
    let m: &Handle = unsafe { &*(handle as *const Handle) };
    let mut guard = match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    Some(f(&mut guard))
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    if s.is_null() {
        return String::new();
    }
    match env.get_string(s) {
        Ok(js) => js.into(),
        Err(_) => String::new(),
    }
}

fn float_array<'a>(env: &mut JNIEnv<'a>, data: &[f32]) -> jfloatArray {
    let arr = match env.new_float_array(data.len() as i32) {
        Ok(a) => a,
        Err(_) => return std::ptr::null_mut(),
    };
    if !data.is_empty() && env.set_float_array_region(&arr, 0, data).is_err() {
        return std::ptr::null_mut();
    }
    arr.into_raw()
}

fn string<'a>(env: &mut JNIEnv<'a>, s: &str) -> jstring {
    match env.new_string(s) {
        Ok(js) => js.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// `nativeCreate(sampleRate, channels, filterId, ditherId, outRate, cacheDir): Long` → handle (0 on failure).
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeCreate<'a>(
    mut env: JNIEnv<'a>,
    _this: JObject<'a>,
    sample_rate: jint,
    channels: jint,
    filter_id: JString<'a>,
    dither_id: JString<'a>,
    out_rate: jint,
    cache_dir: JString<'a>,
) -> jlong {
    let filter = jstr(&mut env, &filter_id);
    let dither = jstr(&mut env, &dither_id);
    let cache = jstr(&mut env, &cache_dir);
    let r = catch_unwind(AssertUnwindSafe(|| {
        Engine::new(
            sample_rate.max(0) as u32,
            channels.max(0) as u32,
            &filter,
            &dither,
            out_rate.max(0) as u32,
            &cache,
        )
    }));
    match r {
        Ok(Ok(engine)) => Box::into_raw(Box::new(Mutex::new(engine))) as jlong,
        _ => 0,
    }
}

/// `nativeOutputRate(handle): Int` – the rate the engine produces (0 for a dead handle).
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeOutputRate<'a>(
    _env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
) -> jint {
    with_engine(handle, |e| e.output_rate() as jint).unwrap_or(0)
}

/// `nativeSetParams(handle, json)`.
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeSetParams<'a>(
    mut env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
    json: JString<'a>,
) {
    let json = jstr(&mut env, &json);
    let _ = catch_unwind(AssertUnwindSafe(|| {
        with_engine(handle, |e| e.set_params(&json));
    }));
}

/// `nativeProcess(handle, input: FloatArray, frames): FloatArray` – interleaved float PCM in and out.
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeProcess<'a>(
    mut env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
    input: JFloatArray<'a>,
    frames: jint,
) -> jfloatArray {
    if input.is_null() {
        return float_array(&mut env, &[]);
    }
    let len = match env.get_array_length(&input) {
        Ok(n) if n >= 0 => n as usize,
        _ => return float_array(&mut env, &[]),
    };
    let mut buf = vec![0f32; len];
    if len > 0 && env.get_float_array_region(&input, 0, &mut buf).is_err() {
        return float_array(&mut env, &[]);
    }
    let out: Vec<f32> = catch_unwind(AssertUnwindSafe(|| {
        with_engine(handle, |e| e.process(&buf, frames.max(0) as usize).to_vec())
    }))
    .ok()
    .flatten()
    .unwrap_or_else(|| buf.clone());
    float_array(&mut env, &out)
}

/// `nativeFlush(handle): FloatArray` – the tail at the end of a stream; resets the filters.
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeFlush<'a>(
    mut env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
) -> jfloatArray {
    let out: Vec<f32> = catch_unwind(AssertUnwindSafe(|| with_engine(handle, |e| e.flush().to_vec())))
        .ok()
        .flatten()
        .unwrap_or_default();
    float_array(&mut env, &out)
}

/// `nativeSignalPath(handle): String` – JSON (`engine::Engine::signal_path_json`); "" for a dead handle.
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeSignalPath<'a>(
    mut env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
) -> jstring {
    let s: String = catch_unwind(AssertUnwindSafe(|| with_engine(handle, |e| e.signal_path_json())))
        .ok()
        .flatten()
        .unwrap_or_default();
    string(&mut env, &s)
}

/// `nativeCatalog(): String` (static) – the filters and dithers on offer as JSON.
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeCatalog<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass<'a>,
) -> jstring {
    let s = catch_unwind(catalog_json).unwrap_or_default();
    string(&mut env, &s)
}

/// `nativeDestroy(handle)`.
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeDestroy<'a>(
    _env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: see with_engine; this is the one place the box is freed.
    let b: Box<Handle> = unsafe { Box::from_raw(handle as *mut Handle) };
    drop(b);
}

#[cfg(test)]
mod tests {
    #[test]
    fn jni_symbols_match_the_kotlin_class() {
        // The Kotlin class is app.encore.android.cadenza.CadenzaNative; a renamed class or package breaks the
        // link at run time only, so pin the names here.
        let names = [
            "Java_app_encore_android_cadenza_CadenzaNative_nativeCreate",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeOutputRate",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeSetParams",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeProcess",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeFlush",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeSignalPath",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeCatalog",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeDestroy",
        ];
        let src = include_str!("lib.rs");
        for n in names {
            assert!(src.contains(&format!("fn {n}<")), "{n} missing");
        }
    }
}
