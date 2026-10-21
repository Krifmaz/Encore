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
//! array / an empty string / [`FAILED`].
//!
//! Audio crosses as direct `ByteBuffer`s (native-order floats) the Kotlin side owns and reuses: `nativeProcess`
//! reads the input through `GetDirectBufferAddress` and writes the output back into the same buffer (in place)
//! or, while the engine upsamples, into a second buffer. No Java array is made or copied per call.

pub mod engine;

pub use engine::{catalog_json, eq_curve_json, Engine, Params};

use jni::objects::{JByteBuffer, JClass, JObject, JString};
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

/// `nativeProcess` / `nativeFetch` result: no engine behind the handle, a buffer that is not a usable direct
/// buffer, or a panic. Any other negative value is "the output is `-value` samples, more than the buffer holds".
pub const FAILED: jint = jint::MIN;

/// A direct buffer's memory as floats: its address and how many whole floats fit (`None` when it is not a direct
/// buffer, or not aligned for `f32`).
fn direct_floats(env: &JNIEnv, buf: &JByteBuffer) -> Option<(*mut f32, usize)> {
    if buf.is_null() {
        return None;
    }
    let ptr = env.get_direct_buffer_address(buf).ok()?;
    let bytes = env.get_direct_buffer_capacity(buf).ok()?;
    if ptr.is_null() || (ptr as usize) % std::mem::align_of::<f32>() != 0 {
        return None;
    }
    Some((ptr as *mut f32, bytes / std::mem::size_of::<f32>()))
}

/// Copies `out` to `dest` when it fits: the sample count, or `-(out.len())` when `dest_len` is too small (the
/// output stays in the engine for [`fetch_into`]).
///
/// # Safety
/// `dest` must be valid for writes of `dest_len` floats and must not overlap `out`.
unsafe fn write_out(out: &[f32], dest: *mut f32, dest_len: usize) -> jint {
    let n = out.len();
    if n > jint::MAX as usize {
        return FAILED;
    }
    if n > dest_len {
        return -(n as jint);
    }
    if n > 0 {
        std::slice::from_raw_parts_mut(dest, n).copy_from_slice(out);
    }
    n as jint
}

/// One `nativeProcess` call without the JVM: `frames` interleaved frames from the `io_len` floats at `io` go
/// through the engine, and its output is written to `dest` (which may be `io` itself: in place). Returns the
/// number of samples written, or `-(samples)` when they don't fit in `dest_len` (fetch them with [`fetch_into`]
/// into a larger buffer before the next call).
///
/// # Safety
/// `io` must be valid for reads of `io_len` floats and `dest` for writes of `dest_len` floats (they may be the
/// same memory); nothing else may use either while this runs.
pub unsafe fn process_into(e: &mut Engine, io: *const f32, io_len: usize, frames: usize, dest: *mut f32, dest_len: usize) -> jint {
    // the input is read (copied into the engine's own scratch) before anything is written: the borrow of `io`
    // ends with this call, so writing the output over the same memory is fine
    let out = {
        let input = if io_len == 0 { &[][..] } else { std::slice::from_raw_parts(io, io_len) };
        e.process(input, frames)
    };
    write_out(out, dest, dest_len)
}

/// The output of the last `process` / `flush` again, into `dest` (after a too-small buffer): the sample count,
/// or `-(samples)` when it still doesn't fit.
///
/// # Safety
/// `dest` must be valid for writes of `dest_len` floats.
pub unsafe fn fetch_into(e: &Engine, dest: *mut f32, dest_len: usize) -> jint {
    write_out(e.last_output(), dest, dest_len)
}

/// `nativeProcess(handle, io: ByteBuffer, frames, out: ByteBuffer?): Int` – `frames` interleaved frames of float
/// PCM in the direct buffer `io` go through the engine. The output is written back into `io` (in place) when
/// `out` is null, else into `out` (the second buffer the Kotlin side owns for upsampling). Returns the number of
/// samples written; `-(samples)` when they don't fit (grow the buffer, then `nativeFetch`); [`FAILED`] when the
/// handle is not live, a buffer is not direct, or the engine panicked (the chunk is lost).
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeProcess<'a>(
    env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
    io: JByteBuffer<'a>,
    frames: jint,
    out: JByteBuffer<'a>,
) -> jint {
    let Some((io_ptr, io_len)) = direct_floats(&env, &io) else {
        return FAILED;
    };
    let (dest, dest_len) = if out.is_null() {
        (io_ptr, io_len)
    } else {
        match direct_floats(&env, &out) {
            Some(d) => d,
            None => return FAILED,
        }
    };
    let frames = frames.max(0) as usize;
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: both buffers are direct buffers the Kotlin caller owns for the length of this call (the
        // processor uses them from the playback thread only); `io` and `dest` may alias, process_into allows it
        with_engine(handle, |e| unsafe { process_into(e, io_ptr, io_len, frames, dest, dest_len) })
    }))
    .ok()
    .flatten()
    .unwrap_or(FAILED)
}

/// `nativeFetch(handle, out: ByteBuffer): Int` – the last output again (after `nativeProcess` said it needs a
/// larger buffer): the number of samples written to `out`, `-(samples)` when it is still too small, [`FAILED`].
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeFetch<'a>(
    env: JNIEnv<'a>,
    _this: JObject<'a>,
    handle: jlong,
    out: JByteBuffer<'a>,
) -> jint {
    let Some((dest, dest_len)) = direct_floats(&env, &out) else {
        return FAILED;
    };
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `out` is a direct buffer the caller owns for the length of this call; the engine's output is
        // its own memory
        with_engine(handle, |e| unsafe { fetch_into(e, dest, dest_len) })
    }))
    .ok()
    .flatten()
    .unwrap_or(FAILED)
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

/// `nativeEqCurve(bandsJson, points): String` (static) – the equalizer graph's curves as JSON ([`eq_curve_json`]).
#[no_mangle]
pub extern "system" fn Java_app_encore_android_cadenza_CadenzaNative_nativeEqCurve<'a>(
    mut env: JNIEnv<'a>,
    _class: JClass<'a>,
    bands: JString<'a>,
    points: jint,
) -> jstring {
    let bands: String = env.get_string(&bands).map(Into::into).unwrap_or_default();
    let s = catch_unwind(|| eq_curve_json(&bands, points.max(0) as usize)).unwrap_or_default();
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
    fn process_writes_back_in_place() {
        // effects only: the output has the input's length and replaces it in the same buffer
        let mut e = engine();
        e.set_params(r#"{"fx_on":true,"preamp_db":-6,"bands":[]}"#);
        let g = 10f32.powf(-6.0 / 20.0);
        // a larger buffer than the chunk (the Kotlin side reuses its buffer): only `frames` frames are touched
        let mut io = vec![0.5f32; 2 * 64];
        io[2 * 32..].iter_mut().for_each(|v| *v = 0.25);
        let p = io.as_mut_ptr();
        let n = unsafe { process_into(&mut e, p, io.len(), 32, p, io.len()) };
        assert_eq!(n, 64);
        assert!(io[..64].iter().all(|v| (v - 0.5 * g).abs() < 1e-6));
        assert!(io[64..].iter().all(|v| *v == 0.25), "samples past the chunk stay as they were");
        // the same answer as the slice API
        let mut e2 = engine();
        e2.set_params(r#"{"fx_on":true,"preamp_db":-6,"bands":[]}"#);
        assert_eq!(e2.process(&vec![0.5f32; 64], 32), &io[..64]);
    }

    #[test]
    fn upsampled_output_goes_to_the_second_buffer_and_a_short_one_is_fetched() {
        // no dither, so a second engine fed the same input gives the same samples
        let mut e = Engine::new(44100, 2, "sinc-l-4k", "none", 88200, "").unwrap();
        let frames = 4410;
        let input: Vec<f32> = (0..frames).flat_map(|i| {
            let v = (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / 44100.0).sin() * 0.5;
            [v, -v]
        }).collect();
        let mut reference = Engine::new(44100, 2, "sinc-l-4k", "none", 88200, "").unwrap();
        let mut io = input.clone();
        let mut out = vec![0f32; 8];
        let mut got = Vec::new();
        let mut want = Vec::new();
        let mut fetched = 0;
        for _ in 0..10 {
            io.copy_from_slice(&input);
            want.extend_from_slice(reference.process(&input, frames));
            let p = io.as_mut_ptr();
            let mut n = unsafe { process_into(&mut e, p, io.len(), frames, out.as_mut_ptr(), out.len()) };
            if n < 0 {
                assert_ne!(n, FAILED);
                // the Kotlin side grows its buffer and fetches the same output
                out = vec![0f32; (-n) as usize];
                n = unsafe { fetch_into(&e, out.as_mut_ptr(), out.len()) };
                fetched += 1;
            }
            assert!(n >= 0);
            got.extend_from_slice(&out[..n as usize]);
            // the input buffer is not written to while upsampling
            assert_eq!(io, input);
        }
        assert!(fetched > 0, "an 8-sample buffer must have been too small");
        assert_eq!(got, want);
        assert!(got.len() > frames * 2 * 10 / 2);
        // still too small: asked again, nothing written
        let mut tiny = [7f32; 1];
        let n = unsafe { fetch_into(&e, tiny.as_mut_ptr(), tiny.len()) };
        assert_eq!(n, -(e.last_output().len() as jint));
        assert_eq!(tiny, [7.0]);
    }

    #[test]
    fn a_dead_handle_processes_nothing() {
        let mut io = vec![0.5f32; 8];
        let p = io.as_mut_ptr();
        let r = with_engine(0x7fff_dead_beef, |e| unsafe { process_into(e, p, 8, 4, p, 8) });
        assert_eq!(r, None);
        assert!(io.iter().all(|v| *v == 0.5));
        // an empty chunk is fine
        let mut e = engine();
        assert_eq!(unsafe { process_into(&mut e, std::ptr::null(), 0, 0, p, 8) }, 0);
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
            "Java_app_encore_android_cadenza_CadenzaNative_nativeFetch",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeFlush",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeSignalPath",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeCatalog",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeEqCurve",
            "Java_app_encore_android_cadenza_CadenzaNative_nativeDestroy",
        ];
        let src = include_str!("lib.rs");
        for n in names {
            assert!(src.contains(&format!("fn {n}<")), "{n} missing");
        }
    }
}
