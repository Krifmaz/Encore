//! `libencore_cadenza.so`: cadenza-dsp on the phone, behind the JNI methods of
//! `app.encore.android.cadenza.CadenzaNative` (clients/android/cadenza/src/main/java/…/CadenzaNative.kt).
//!
//! The engine itself ([`engine::Engine`]) is plain Rust and is what `cargo test` covers on the host; the JNI
//! functions below only move data across. Every native method is an *instance* method of `CadenzaNative`, so the
//! second argument is the Kotlin object (unused). A handle is a number naming an engine in a registry (never a
//! pointer, never reused); 0 is "no engine" and every function tolerates it. A call holds its own reference to
//! the engine, so freeing it while the audio thread is still inside `process` (the player released from the main
//! thread, the Signal path read during a configure) only drops the registry's reference: the engine goes when the
//! last call returns. A stale or freed handle finds nothing (no use-after-free, no double free; GrapheneOS's
//! hardened malloc turns those into crashes). Nothing here unwinds into the JVM: panics turn into 0 / an empty
//! array / an empty string.

pub mod engine;

pub use engine::{catalog_json, Engine, Params};

use jni::objects::{JClass, JFloatArray, JObject, JString};
use jni::sys::{jfloatArray, jint, jlong, jstring};
use jni::JNIEnv;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

type Shared = Arc<Mutex<Engine>>;

/// Live engines by handle. Handles count up from 1 and are never reused.
fn registry() -> MutexGuard<'static, HashMap<jlong, Shared>> {
    static ENGINES: OnceLock<Mutex<HashMap<jlong, Shared>>> = OnceLock::new();
    let m = ENGINES.get_or_init(|| Mutex::new(HashMap::new()));
    m.lock().unwrap_or_else(|p| p.into_inner())
}

static NEXT: AtomicI64 = AtomicI64::new(1);

/// Keeps `engine` and returns its handle (never 0).
pub fn register(engine: Engine) -> jlong {
    let h = NEXT.fetch_add(1, Ordering::Relaxed);
    registry().insert(h, Arc::new(Mutex::new(engine)));
    h
}

/// Forgets the handle; `false` when it was not live (already freed, never made). An engine still in use by
/// another thread is dropped when that call returns.
pub fn unregister(handle: jlong) -> bool {
    if handle == 0 {
        return false;
    }
    let removed = registry().remove(&handle);
    removed.is_some()
}

/// Runs `f` on the engine behind `handle`; `None` for 0 or a handle that is no longer live.
pub fn with_engine<R>(handle: jlong, f: impl FnOnce(&mut Engine) -> R) -> Option<R> {
    if handle == 0 {
        return None;
    }
    // the registry lock is only held to take a reference, never while the engine works
    let e: Shared = registry().get(&handle)?.clone();
    let mut guard = e.lock().unwrap_or_else(|p| p.into_inner());
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
        Ok(Ok(engine)) => register(engine),
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

/// `nativeProcess(handle, input: FloatArray, frames): FloatArray?` – interleaved float PCM in and out; null when
/// the handle is not live (or the engine panicked).
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
    let out: Option<Vec<f32>> = catch_unwind(AssertUnwindSafe(|| {
        with_engine(handle, |e| e.process(&buf, frames.max(0) as usize).to_vec())
    }))
    .ok()
    .flatten();
    // no engine (freed by another thread) or a panic: null, and the Kotlin side passes its input through. Not the
    // whole array: the caller reuses a larger buffer, only `frames` of it are this chunk
    match out {
        Some(out) => float_array(&mut env, &out),
        None => std::ptr::null_mut(),
    }
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
    // a panic in an engine's Drop must not unwind into the JVM
    let _ = catch_unwind(|| unregister(handle));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(48000, 2, "", "", 0, "").unwrap()
    }

    #[test]
    fn handles_are_checked_and_freed_once() {
        let h = register(engine());
        assert_ne!(h, 0);
        assert_eq!(with_engine(h, |e| e.output_rate()), Some(48000));
        assert!(unregister(h));
        // a stale handle finds nothing, a second free is a no-op (no double free)
        assert_eq!(with_engine(h, |e| e.output_rate()), None);
        assert!(!unregister(h));
        assert!(!unregister(0));
        assert_eq!(with_engine(0, |e| e.output_rate()), None);
        // a pointer-looking number is not dereferenced
        assert_eq!(with_engine(0x7fff_dead_beef, |e| e.output_rate()), None);
        // handles are not reused
        let h2 = register(engine());
        assert_ne!(h2, h);
        assert!(unregister(h2));
    }

    #[test]
    fn freeing_while_processing_on_another_thread_is_safe() {
        // the audio thread keeps processing while another thread frees and rebuilds the engine (player release,
        // configure on a new song, the Signal path read from the UI)
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let current = Arc::new(AtomicI64::new(register(engine())));
        // buffers the audio thread got through; the swaps below only start once it is running, so the test
        // can't finish before the thread was ever scheduled (it did on a fast CI runner)
        let done = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let audio = {
            let (stop, current, done) = (stop.clone(), current.clone(), done.clone());
            std::thread::spawn(move || {
                let input = vec![0.25f32; 2 * 512];
                while !stop.load(Ordering::Relaxed) {
                    let h = current.load(Ordering::Relaxed);
                    if let Some(n) = with_engine(h, |e| {
                        e.set_params(r#"{"fx_on":true,"preamp_db":-3,"bands":[{"type":"peaking","freq":1000,"gain":3,"q":1}]}"#);
                        e.process(&input, 512).len()
                    }) {
                        assert_eq!(n, input.len());
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                }
            })
        };
        while done.load(Ordering::Relaxed) == 0 {
            std::thread::yield_now();
        }
        for _ in 0..300 {
            let old = current.swap(register(engine()), Ordering::Relaxed);
            assert!(unregister(old));
            let _ = with_engine(old, |e| e.signal_path_json());
        }
        stop.store(true, Ordering::Relaxed);
        audio.join().unwrap();
        assert!(done.load(Ordering::Relaxed) > 0);
        assert!(unregister(current.load(Ordering::Relaxed)));
    }

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
