/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! Host microphone capture, shared by Audio Queue input, Audio Unit RemoteIO
//! input and AVAudioRecorder.
//!
//! Exactly one host capture stream exists per process, opened at
//! [MIC_SAMPLE_RATE] Hz, mono, 16-bit PCM. It is refcounted ([start],
//! [stop]): the first guest object that starts opens it, the last one that
//! stops closes it. A background thread polls the platform backend (OpenAL
//! capture on desktop hosts, the AudioRecord JNI bridge on Android) every
//! ~15 ms and appends the new samples to a short ring buffer. If the host
//! has no microphone (or capture was refused), the thread instead advances
//! the sample counter with silence at the real-time rate, so every consumer
//! keeps seeing a well-formed stream instead of hanging.
//!
//! Consumers read with *cursors* into the monotonically increasing "total
//! samples captured" counter, so several guests can consume the same stream
//! without stealing data from each other; [take_samples] zero-fills any
//! requested range that is no longer in the ring (on real hardware, late
//! data is simply gone too).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
use std::sync::atomic::AtomicU8;
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::audio::openal;



/// Sample rate of the shared capture stream.
pub const MIC_SAMPLE_RATE: u32 = crate::android_media::MIC_SAMPLE_RATE;

/// Ring-buffer capacity in seconds of audio. Consumers that fall further
/// behind get zeros for the evicted part (a real device drops late data too).
const RING_SECONDS: f64 = 2.0;

/// How often the capture thread polls the platform backend.
const POLL_INTERVAL: Duration = Duration::from_millis(15);

/// OpenAL capture device buffer size in samples (~100 ms).
const CAPTURE_BUFFER_SAMPLES: u32 = MIC_SAMPLE_RATE / 10;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic in a callback while holding a lock must not permanently
    // poison the microphone for the rest of the session.
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct Ring {
    /// Most recent samples, newest last (mono 16-bit).
    data: Vec<i16>,
    /// Total samples appended since the stream last (re)started.
    total: u64,
    /// Fractional-sample accumulator used when synthesising silence.
    silence_frac: f64,
}

static RING: Mutex<Ring> = Mutex::new(Ring {
    data: Vec::new(),
    total: 0,
    silence_frac: 0.0,
});

/// Number of guest objects currently holding the capture stream open.
static USERS: Mutex<usize> = Mutex::new(0);

static THREAD: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static THREAD_ALIVE: AtomicBool = AtomicBool::new(false);

/// Whether the currently-open stream has a real backend behind it.
static STREAM_AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Cursor for [read_mic_chunk]: everything newer than this is returned next.
static CONSUMED: AtomicU64 = AtomicU64::new(0);

/// Cached OpenAL probe result on other hosts: 0 unknown, 1 yes, 2 no.
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
static AVAILABILITY: AtomicU8 = AtomicU8::new(0);
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
const AVAIL_YES: u8 = 1;
#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
const AVAIL_NO: u8 = 2;

struct SendDevice(*mut openal::alc_types::ALCdevice);
unsafe impl Send for SendDevice {}

#[allow(dead_code)] // the OpenAL variant is unused on Android (JNI backend)
enum Backend {
    /// Capture stream is stopped (or unavailable).
    None,
    #[cfg(target_os = "android")]
    Android,
    OpenAL(SendDevice),
}

static BACKEND: Mutex<Backend> = Mutex::new(Backend::None);

/// Whether the selected host audio input is available.
///
/// On Android this asks the Java side (which also knows whether the
/// RECORD_AUDIO permission was granted). On iOS it asks AVAudioSession, see
/// [ios_input_available]. macOS reads Core Audio device properties without
/// opening capture or checking recording permission. Other hosts probe
/// OpenAL capture once and cache the answer.
pub fn has_microphone() -> bool {
    #[cfg(target_os = "android")]
    {
        return crate::android_media::jni_has_microphone();
    }
    #[cfg(target_os = "ios")]
    {
        return ios_input_available();
    }
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn touchhle_macos_input_available() -> bool;
        }
        // The bridge reads properties only; actual capture starts in start().
        return unsafe { touchhle_macos_input_available() };
    }
    #[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
    {
        match AVAILABILITY.load(Ordering::Relaxed) {
            AVAIL_YES => true,
            AVAIL_NO => false,
            _ => {
                let yes = probe_openal_capture();
                AVAILABILITY.store(if yes { AVAIL_YES } else { AVAIL_NO }, Ordering::Relaxed);
                yes
            }
        }
    }
}

/// `[[AVAudioSession sharedInstance] isInputAvailable]`.
///
/// The OpenAL probe can't be used on iOS: opening a capture device makes the
/// system check microphone access, and it kills an app whose Info.plist has
/// no NSMicrophoneUsageDescription. Reading this property doesn't touch the
/// microphone, like the guest's own availability queries on a real device.
#[cfg(target_os = "ios")]
fn ios_input_available() -> bool {
    use std::ffi::{c_char, c_void};

    type Id = *mut c_void;
    type Sel = *mut c_void;
    extern "C" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
    }

    unsafe {
        // On arm64, objc_msgSend must be called through a pointer of the
        // method's real type.
        let send_id: unsafe extern "C" fn(Id, Sel) -> Id =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
        let send_bool: unsafe extern "C" fn(Id, Sel) -> bool =
            std::mem::transmute(objc_msgSend as unsafe extern "C" fn());

        let class = objc_getClass(b"AVAudioSession\0".as_ptr().cast());
        if class.is_null() {
            return false;
        }
        let shared_instance = sel_registerName(b"sharedInstance\0".as_ptr().cast());
        let session = send_id(class, shared_instance);
        if session.is_null() {
            return false;
        }
        let is_input_available = sel_registerName(b"isInputAvailable\0".as_ptr().cast());
        send_bool(session, is_input_available)
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios", target_os = "macos")))]
fn probe_openal_capture() -> bool {
    unsafe {
        // Opening a minimal capture device is the only reliable way to ask;
        // done once and cached by the caller.
        let dev = openal::alcCaptureOpenDevice(
            std::ptr::null(),
            MIC_SAMPLE_RATE,
            openal::AL_FORMAT_MONO16,
            1,
        );
        if dev.is_null() {
            return false;
        }
        openal::alcCaptureCloseDevice(dev);
        true
    }
}

/// Open the shared capture stream. Returns whether a real microphone is
/// behind it; with `false` the stream still advances (with silence), so
/// callers may carry on and simply record/emit silence.
pub fn start() -> bool {
    let mut users = lock(&USERS);
    *users += 1;
    if *users > 1 {
        return STREAM_AVAILABLE.load(Ordering::SeqCst);
    }

    // First consumer: (re)initialise the stream.
    {
        let mut ring = lock(&RING);
        ring.data.clear();
        ring.total = 0;
        ring.silence_frac = 0.0;
    }
    CONSUMED.store(0, Ordering::SeqCst);

    let available = init_backend();
    STREAM_AVAILABLE.store(available, Ordering::SeqCst);

    // Always run the capture thread: with no backend it keeps the sample
    // counter advancing with silence at the real-time rate.
    THREAD_ALIVE.store(true, Ordering::SeqCst);
    match std::thread::Builder::new()
        .name("HyperHLE mic capture".to_string())
        .spawn(mic_thread)
    {
        Ok(handle) => *lock(&THREAD) = Some(handle),
        Err(err) => {
            // Extremely unlikely (host thread creation failed); degrade to a
            // frozen silent stream rather than crashing the app.
            THREAD_ALIVE.store(false, Ordering::SeqCst);
            log!(
                "Warning: could not spawn the microphone capture thread ({}); \
                 microphone input will be silent.",
                err
            );
        }
    }

    if !available {
        log_once!("No host microphone available; microphone input will be silent.");
    }

    available
}

/// Release one reference on the shared capture stream; the last release
/// stops the capture thread and closes the backend.
pub fn stop() {
    let mut users = lock(&USERS);
    if *users == 0 {
        // Unmatched stop (e.g. a queue that never started); ignore.
        return;
    }
    *users -= 1;
    if *users > 0 {
        return;
    }

    THREAD_ALIVE.store(false, Ordering::SeqCst);
    if let Some(handle) = lock(&THREAD).take() {
        let _ = handle.join();
    }
    shutdown_backend();
    STREAM_AVAILABLE.store(false, Ordering::SeqCst);
    lock(&RING).data.clear();
}

fn init_backend() -> bool {
    // On Android the AudioRecord JNI bridge is the preferred backend.
    #[cfg(target_os = "android")]
    if crate::android_media::jni_start_mic() {
        *lock(&BACKEND) = Backend::Android;
        return true;
    }

    // Everywhere else (and as an Android fallback), use OpenAL capture. A
    // dedicated capture device: capture needs no ALC context, so this is
    // fully independent of the playback context Audio Toolbox keeps.
    unsafe {
        let dev = openal::alcCaptureOpenDevice(
            std::ptr::null(),
            MIC_SAMPLE_RATE,
            openal::AL_FORMAT_MONO16,
            CAPTURE_BUFFER_SAMPLES as i32,
        );
        if dev.is_null() {
            log_dbg!("alcCaptureOpenDevice() failed: no host microphone.");
            *lock(&BACKEND) = Backend::None;
            return false;
        }
        openal::alcCaptureStart(dev);
        *lock(&BACKEND) = Backend::OpenAL(SendDevice(dev));
        true
    }
}

fn shutdown_backend() {
    let old = std::mem::replace(&mut *lock(&BACKEND), Backend::None);
    match old {
        Backend::None => {}
        #[cfg(target_os = "android")]
        Backend::Android => crate::android_media::jni_stop_mic(),
        Backend::OpenAL(dev) => unsafe {
            openal::alcCaptureStop(dev.0);
            openal::alcCaptureCloseDevice(dev.0);
        },
    }
}

fn mic_thread() {
    while THREAD_ALIVE.load(Ordering::Relaxed) {
        poll_once();
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn poll_once() {
    let backend = lock(&BACKEND);
    match &*backend {
        Backend::None => append_silence_tick(),
        #[cfg(target_os = "android")]
        Backend::Android => {
            let chunk = crate::android_media::jni_read_mic_chunk();
            if chunk.is_empty() {
                // No fresh data this tick; advance with silence so the
                // stream keeps the real-time rate.
                append_silence_tick();
            } else {
                append_samples(&chunk);
            }
        }
        Backend::OpenAL(dev) => unsafe {
            let dev = dev.0;
            let mut available: openal::alc_types::ALCint = 0;
            openal::alcGetIntegerv(dev, openal::ALC_CAPTURE_SAMPLES, 1, &mut available);
            if available <= 0 {
                append_silence_tick();
            } else {
                let mut samples = vec![0i16; available as usize];
                openal::alcCaptureSamples(dev, samples.as_mut_ptr().cast(), available);
                append_samples(&samples);
            }
        },
    }
}

fn append_samples(samples: &[i16]) {
    if samples.is_empty() {
        return;
    }
    let mut ring = lock(&RING);
    ring.data.extend_from_slice(samples);
    ring.total += samples.len() as u64;
    trim_ring(&mut ring);
}

/// Advance the sample counter with ~one poll interval worth of silence.
fn append_silence_tick() {
    const TICK_SAMPLES: f64 = MIC_SAMPLE_RATE as f64 * POLL_INTERVAL.as_secs_f64();
    let mut ring = lock(&RING);
    ring.silence_frac += TICK_SAMPLES;
    let whole = ring.silence_frac as u64;
    if whole == 0 {
        return;
    }
    ring.silence_frac -= whole as f64;
    ring.data.extend(std::iter::repeat(0i16).take(whole as usize));
    ring.total += whole;
    trim_ring(&mut ring);
}

fn trim_ring(ring: &mut Ring) {
    let max_samples = (RING_SECONDS * MIC_SAMPLE_RATE as f64) as usize;
    if ring.data.len() > max_samples {
        let excess = ring.data.len() - max_samples;
        ring.data.drain(..excess);
    }
}

/// New samples since the previous call to this function (one shared cursor
/// per process, matching the historical `android_media::read_mic_chunk`
/// contract). Mono 16-bit PCM at [MIC_SAMPLE_RATE].
pub fn read_mic_chunk() -> Vec<i16> {
    let total = total_samples();
    let from = CONSUMED.swap(total, Ordering::SeqCst);
    if from >= total {
        return Vec::new();
    }
    take_samples(from, (total - from) as usize)
}

/// `count` mono samples starting at absolute sample `from` (a cursor from
/// [total_samples]). Parts of the range that were evicted from the ring are
/// zeros, like late data on a real device.
pub fn take_samples(from: u64, count: usize) -> Vec<i16> {
    if count == 0 {
        return Vec::new();
    }
    let mut out = vec![0i16; count];
    let ring = lock(&RING);
    let window_start = ring.total - ring.data.len() as u64;
    let start = from.max(window_start);
    let end = (from + count as u64).min(ring.total);
    if end > start {
        let offset = (start - window_start) as usize;
        let len = (end - start) as usize;
        let dst = (start - from) as usize;
        out[dst..dst + len].copy_from_slice(&ring.data[offset..offset + len]);
    }
    out
}

/// The absolute sample counter: how many samples have been "captured" since
/// the stream last (re)started. Monotonic within a capture session.
pub fn total_samples() -> u64 {
    lock(&RING).total
}

/// RMS level of the most recent ~150 ms of the stream, in dBFS
/// (-160.0 ..= 0.0). Mirrors `AudioQueueLevelMeterState.mAveragePower`.
pub fn average_power_db() -> f32 {
    levels_db().0
}

/// `(average, peak)` levels of the most recent ~150 ms, in dBFS, mirroring
/// `AudioQueueLevelMeterState`.
pub fn levels_db() -> (f32, f32) {
    let ring = lock(&RING);
    let window = ((MIC_SAMPLE_RATE as f64 * 0.15) as usize).min(ring.data.len());
    if window == 0 {
        return (-160.0, -160.0);
    }
    let start = ring.data.len() - window;
    let mut sum_squares = 0.0f64;
    let mut peak = 0.0f64;
    for &sample in &ring.data[start..] {
        let s = sample as f64 / 32768.0;
        sum_squares += s * s;
        peak = peak.max(s.abs());
    }
    let rms = (sum_squares / window as f64).sqrt();
    let to_db = |magnitude: f64| {
        if magnitude <= 0.0 {
            -160.0
        } else {
            (20.0 * magnitude.log10()).clamp(-160.0, 0.0) as f32
        }
    };
    (to_db(rms), to_db(peak))
}
