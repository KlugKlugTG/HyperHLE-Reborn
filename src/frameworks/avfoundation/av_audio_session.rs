/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AVAudioSession`.

use crate::dyld::{ConstantExports, HostConstant};
use crate::Environment;
use crate::frameworks::foundation::{ns_string, NSInteger, NSUInteger};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, objc_retainBlock, release, retain,
    ClassExports, HostObject,
};

// MARK: - Category constants

pub const AVAudioSessionCategoryAmbient: &str = "AVAudioSessionCategoryAmbient";
pub const AVAudioSessionCategorySoloAmbient: &str = "AVAudioSessionCategorySoloAmbient";
pub const AVAudioSessionCategoryPlayback: &str = "AVAudioSessionCategoryPlayback";
pub const AVAudioSessionCategoryRecord: &str = "AVAudioSessionCategoryRecord";
pub const AVAudioSessionCategoryPlayAndRecord: &str = "AVAudioSessionCategoryPlayAndRecord";
pub const AVAudioSessionCategoryAudioProcessing: &str = "AVAudioSessionCategoryAudioProcessing";
pub const AVAudioSessionCategoryMultiRoute: &str = "AVAudioSessionCategoryMultiRoute";

// MARK: - Mode constants

pub const AVAudioSessionModeDefault: &str = "AVAudioSessionModeDefault";
pub const AVAudioSessionModeVoiceChat: &str = "AVAudioSessionModeVoiceChat";
pub const AVAudioSessionModeGameChat: &str = "AVAudioSessionModeGameChat";
pub const AVAudioSessionModeVideoRecording: &str = "AVAudioSessionModeVideoRecording";
pub const AVAudioSessionModeMeasurement: &str = "AVAudioSessionModeMeasurement";
pub const AVAudioSessionModeMoviePlayback: &str = "AVAudioSessionModeMoviePlayback";
pub const AVAudioSessionModeVideoChat: &str = "AVAudioSessionModeVideoChat";
pub const AVAudioSessionModeSpokenAudio: &str = "AVAudioSessionModeSpokenAudio";
// MARK: - Record permission constants (AVAudioSessionRecordPermission, iOS 8+)
// FourCC values straight from the Apple SDK header:
//   AVAudioSessionRecordPermissionUndetermined = 'undt'
//   AVAudioSessionRecordPermissionDenied       = 'deny'
//   AVAudioSessionRecordPermissionGranted      = 'grnt'

pub const AVAudioSessionRecordPermissionUndetermined: u32 = u32::from_be_bytes(*b"undt");
pub const AVAudioSessionRecordPermissionDenied: u32 = u32::from_be_bytes(*b"deny");
pub const AVAudioSessionRecordPermissionGranted: u32 = u32::from_be_bytes(*b"grnt");


// MARK: - Notification constants

pub const AVAudioSessionInterruptionNotification: &str = "AVAudioSessionInterruptionNotification";
pub const AVAudioSessionRouteChangeNotification: &str = "AVAudioSessionRouteChangeNotification";
pub const AVAudioSessionMediaServicesWereLostNotification: &str =
    "AVAudioSessionMediaServicesWereLostNotification";
pub const AVAudioSessionMediaServicesWereResetNotification: &str =
    "AVAudioSessionMediaServicesWereResetNotification";
pub const AVAudioSessionSilenceSecondaryAudioHintNotification: &str =
    "AVAudioSessionSilenceSecondaryAudioHintNotification";

// MARK: - UserInfo keys

pub const AVAudioSessionInterruptionTypeKey: &str = "AVAudioSessionInterruptionTypeKey";
pub const AVAudioSessionInterruptionOptionKey: &str = "AVAudioSessionInterruptionOptionKey";
pub const AVAudioSessionRouteChangeReasonKey: &str = "AVAudioSessionRouteChangeReasonKey";
pub const AVAudioSessionRouteChangePreviousRouteKey: &str =
    "AVAudioSessionRouteChangePreviousRouteKey";
pub const AVAudioSessionSilenceSecondaryAudioHintTypeKey: &str =
    "AVAudioSessionSilenceSecondaryAudioHintTypeKey";

// MARK: - Category option flags

type AVAudioSessionCategoryOptions = NSUInteger;
const AVAudioSessionCategoryOptionMixWithOthers: AVAudioSessionCategoryOptions = 0x1;
const AVAudioSessionCategoryOptionDuckOthers: AVAudioSessionCategoryOptions = 0x2;
const AVAudioSessionCategoryOptionAllowBluetooth: AVAudioSessionCategoryOptions = 0x4;
const AVAudioSessionCategoryOptionDefaultToSpeaker: AVAudioSessionCategoryOptions = 0x8;
const AVAudioSessionCategoryOptionInterruptSpokenAudioAndMixWithOthers:
    AVAudioSessionCategoryOptions = 0x11;
const AVAudioSessionCategoryOptionAllowBluetoothA2DP: AVAudioSessionCategoryOptions = 0x20;
const AVAudioSessionCategoryOptionAllowAirPlay: AVAudioSessionCategoryOptions = 0x40;

// MARK: - Port override

/// `AVAudioSessionPortOverride`, used by `-overrideOutputAudioPort:error:`.
type AVAudioSessionPortOverride = NSUInteger;
const AVAudioSessionPortOverrideNone: AVAudioSessionPortOverride = 0;
/// Apple spells the speaker override as the four-character code `'spkr'`.
const AVAudioSessionPortOverrideSpeaker: AVAudioSessionPortOverride = 0x7370_6b72;

// MARK: - Interruption type

type AVAudioSessionInterruptionType = NSUInteger;
const AVAudioSessionInterruptionTypeBegan: AVAudioSessionInterruptionType = 1;
const AVAudioSessionInterruptionTypeEnded: AVAudioSessionInterruptionType = 0;

// MARK: - Route change reason

type AVAudioSessionRouteChangeReason = NSUInteger;
const AVAudioSessionRouteChangeReasonUnknown: AVAudioSessionRouteChangeReason = 0;
const AVAudioSessionRouteChangeReasonNewDeviceAvailable: AVAudioSessionRouteChangeReason = 1;
const AVAudioSessionRouteChangeReasonOldDeviceUnavailable: AVAudioSessionRouteChangeReason = 2;
const AVAudioSessionRouteChangeReasonCategoryChange: AVAudioSessionRouteChangeReason = 3;
const AVAudioSessionRouteChangeReasonOverride: AVAudioSessionRouteChangeReason = 4;
const AVAudioSessionRouteChangeReasonWakeFromSleep: AVAudioSessionRouteChangeReason = 6;
const AVAudioSessionRouteChangeReasonNoSuitableRouteForCategory: AVAudioSessionRouteChangeReason =
    7;
const AVAudioSessionRouteChangeReasonRouteConfigurationChange: AVAudioSessionRouteChangeReason = 8;

// MARK: - Port types

// Per Apple's
// <https://developer.apple.com/documentation/avfaudio/avaudiosessionport>,
// each `AVAudioSessionPort*` constant is a `NSString *` whose literal value
// is the bare port-type identifier ("Speaker", "Headphones", ...). Apps use
// `[portDescription.portType isEqualToString:AVAudioSessionPortHeadphones]`,
// so the only requirement is that each constant resolves to a distinct,
// non-NULL NSString with the correct iOS-canonical spelling.
pub const AVAudioSessionPortBuiltInSpeaker: &str = "Speaker";
pub const AVAudioSessionPortBuiltInReceiver: &str = "Receiver";
pub const AVAudioSessionPortBuiltInMic: &str = "MicrophoneBuiltIn";
pub const AVAudioSessionPortHeadphones: &str = "Headphones";
pub const AVAudioSessionPortHeadsetMic: &str = "MicrophoneWired";
pub const AVAudioSessionPortLineIn: &str = "LineIn";
pub const AVAudioSessionPortLineOut: &str = "LineOut";
pub const AVAudioSessionPortBluetoothA2DP: &str = "BluetoothA2DPOutput";
pub const AVAudioSessionPortBluetoothLE: &str = "BluetoothLEOutput";
pub const AVAudioSessionPortBluetoothHFP: &str = "BluetoothHFP";
pub const AVAudioSessionPortUSBAudio: &str = "USBAudio";
pub const AVAudioSessionPortHDMI: &str = "HDMI";
pub const AVAudioSessionPortAirPlay: &str = "AirPlay";
pub const AVAudioSessionPortCarAudio: &str = "CarAudio";
pub const AVAudioSessionPortAVB: &str = "AVB";
pub const AVAudioSessionPortDisplayPort: &str = "DisplayPort";
pub const AVAudioSessionPortFireWire: &str = "FireWire";
pub const AVAudioSessionPortPCI: &str = "PCI";
pub const AVAudioSessionPortThunderbolt: &str = "Thunderbolt";
pub const AVAudioSessionPortVirtual: &str = "Virtual";
pub const AVAudioSessionPortContinuityMicrophone: &str = "ContinuityMicrophone";

pub const CONSTANTS: ConstantExports = &[
    // Categories
    (
        "_AVAudioSessionCategoryAmbient",
        HostConstant::NSString(AVAudioSessionCategoryAmbient),
    ),
    (
        "_AVAudioSessionCategorySoloAmbient",
        HostConstant::NSString(AVAudioSessionCategorySoloAmbient),
    ),
    (
        "_AVAudioSessionCategoryPlayback",
        HostConstant::NSString(AVAudioSessionCategoryPlayback),
    ),
    (
        "_AVAudioSessionCategoryRecord",
        HostConstant::NSString(AVAudioSessionCategoryRecord),
    ),
    (
        "_AVAudioSessionCategoryPlayAndRecord",
        HostConstant::NSString(AVAudioSessionCategoryPlayAndRecord),
    ),
    (
        "_AVAudioSessionCategoryAudioProcessing",
        HostConstant::NSString(AVAudioSessionCategoryAudioProcessing),
    ),
    (
        "_AVAudioSessionCategoryMultiRoute",
        HostConstant::NSString(AVAudioSessionCategoryMultiRoute),
    ),
    // Modes
    (
        "_AVAudioSessionModeDefault",
        HostConstant::NSString(AVAudioSessionModeDefault),
    ),
    (
        "_AVAudioSessionModeVoiceChat",
        HostConstant::NSString(AVAudioSessionModeVoiceChat),
    ),
    (
        "_AVAudioSessionModeGameChat",
        HostConstant::NSString(AVAudioSessionModeGameChat),
    ),
    (
        "_AVAudioSessionModeVideoRecording",
        HostConstant::NSString(AVAudioSessionModeVideoRecording),
    ),
    (
        "_AVAudioSessionModeMeasurement",
        HostConstant::NSString(AVAudioSessionModeMeasurement),
    ),
    (
        "_AVAudioSessionModeMoviePlayback",
        HostConstant::NSString(AVAudioSessionModeMoviePlayback),
    ),
    (
        "_AVAudioSessionModeVideoChat",
        HostConstant::NSString(AVAudioSessionModeVideoChat),
    ),
    (
        "_AVAudioSessionModeSpokenAudio",
        HostConstant::NSString(AVAudioSessionModeSpokenAudio),
    ),
    // Notifications
    (
        "_AVAudioSessionInterruptionNotification",
        HostConstant::NSString(AVAudioSessionInterruptionNotification),
    ),
    (
        "_AVAudioSessionRouteChangeNotification",
        HostConstant::NSString(AVAudioSessionRouteChangeNotification),
    ),
    (
        "_AVAudioSessionMediaServicesWereLostNotification",
        HostConstant::NSString(AVAudioSessionMediaServicesWereLostNotification),
    ),
    (
        "_AVAudioSessionMediaServicesWereResetNotification",
        HostConstant::NSString(AVAudioSessionMediaServicesWereResetNotification),
    ),
    (
        "_AVAudioSessionSilenceSecondaryAudioHintNotification",
        HostConstant::NSString(AVAudioSessionSilenceSecondaryAudioHintNotification),
    ),
    // UserInfo keys
    (
        "_AVAudioSessionInterruptionTypeKey",
        HostConstant::NSString(AVAudioSessionInterruptionTypeKey),
    ),
    (
        "_AVAudioSessionInterruptionOptionKey",
        HostConstant::NSString(AVAudioSessionInterruptionOptionKey),
    ),
    (
        "_AVAudioSessionRouteChangeReasonKey",
        HostConstant::NSString(AVAudioSessionRouteChangeReasonKey),
    ),
    (
        "_AVAudioSessionRouteChangePreviousRouteKey",
        HostConstant::NSString(AVAudioSessionRouteChangePreviousRouteKey),
    ),
    (
        "_AVAudioSessionSilenceSecondaryAudioHintTypeKey",
        HostConstant::NSString(AVAudioSessionSilenceSecondaryAudioHintTypeKey),
    ),
    // Port types
    (
        "_AVAudioSessionPortBuiltInSpeaker",
        HostConstant::NSString(AVAudioSessionPortBuiltInSpeaker),
    ),
    (
        "_AVAudioSessionPortBuiltInReceiver",
        HostConstant::NSString(AVAudioSessionPortBuiltInReceiver),
    ),
    (
        "_AVAudioSessionPortBuiltInMic",
        HostConstant::NSString(AVAudioSessionPortBuiltInMic),
    ),
    (
        "_AVAudioSessionPortHeadphones",
        HostConstant::NSString(AVAudioSessionPortHeadphones),
    ),
    (
        "_AVAudioSessionPortBluetoothA2DP",
        HostConstant::NSString(AVAudioSessionPortBluetoothA2DP),
    ),
    (
        "_AVAudioSessionPortBluetoothLE",
        HostConstant::NSString(AVAudioSessionPortBluetoothLE),
    ),
    (
        "_AVAudioSessionPortBluetoothHFP",
        HostConstant::NSString(AVAudioSessionPortBluetoothHFP),
    ),
    (
        "_AVAudioSessionPortHeadsetMic",
        HostConstant::NSString(AVAudioSessionPortHeadsetMic),
    ),
    (
        "_AVAudioSessionPortLineIn",
        HostConstant::NSString(AVAudioSessionPortLineIn),
    ),
    (
        "_AVAudioSessionPortLineOut",
        HostConstant::NSString(AVAudioSessionPortLineOut),
    ),
    (
        "_AVAudioSessionPortUSBAudio",
        HostConstant::NSString(AVAudioSessionPortUSBAudio),
    ),
    (
        "_AVAudioSessionPortHDMI",
        HostConstant::NSString(AVAudioSessionPortHDMI),
    ),
    (
        "_AVAudioSessionPortAirPlay",
        HostConstant::NSString(AVAudioSessionPortAirPlay),
    ),
    (
        "_AVAudioSessionPortCarAudio",
        HostConstant::NSString(AVAudioSessionPortCarAudio),
    ),
    (
        "_AVAudioSessionPortAVB",
        HostConstant::NSString(AVAudioSessionPortAVB),
    ),
    (
        "_AVAudioSessionPortDisplayPort",
        HostConstant::NSString(AVAudioSessionPortDisplayPort),
    ),
    (
        "_AVAudioSessionPortFireWire",
        HostConstant::NSString(AVAudioSessionPortFireWire),
    ),
    (
        "_AVAudioSessionPortPCI",
        HostConstant::NSString(AVAudioSessionPortPCI),
    ),
    (
        "_AVAudioSessionPortThunderbolt",
        HostConstant::NSString(AVAudioSessionPortThunderbolt),
    ),
    (
        "_AVAudioSessionPortVirtual",
        HostConstant::NSString(AVAudioSessionPortVirtual),
    ),
    (
        "_AVAudioSessionPortContinuityMicrophone",
        HostConstant::NSString(AVAudioSessionPortContinuityMicrophone),
    ),
];

#[derive(Default)]
pub struct State {
    shared_instance: Option<id>,
}

// ДОБАВЛЕНО: Полноценный объект состояния (без заглушек)
#[derive(Default)]
pub(super) struct AVAudioSessionHostObject {
    category: id,
    category_options: AVAudioSessionCategoryOptions,
    mode: id,
    active: bool,
    preferred_sample_rate: f64,
    preferred_io_buffer_duration: f64,
    delegate: id,
}
impl HostObject for AVAudioSessionHostObject {}

/// Host object backing `AVAudioSessionPortDescription` instances we create
/// ourselves (built-in mic for `availableInputs`, speaker for `currentRoute`).
pub(super) struct AVAudioSessionPortDescriptionHostObject {
    port_type: &'static str,
    name: &'static str,
    uid: &'static str,
}
impl HostObject for AVAudioSessionPortDescriptionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation AVAudioSession: NSObject

+ (id)sharedInstance {
    // ИСПРАВЛЕНО: Теперь объект действительно работает как синглтон и сохраняет
    // свое состояние
    if let Some(instance) = env.framework_state.avfoundation.av_audio_session.shared_instance {
        return instance;
    }

    let category = ns_string::get_static_str(env, AVAudioSessionCategorySoloAmbient);
    let mode = ns_string::get_static_str(env, AVAudioSessionModeDefault);

    let host_object = Box::new(AVAudioSessionHostObject {
        category,
        category_options: 0,
        mode,
        active: false,
        preferred_sample_rate: 44100.0,
        preferred_io_buffer_duration: 0.005,
        delegate: nil,
    });

    let instance = env.objc.alloc_static_object(
        this,
        host_object,
        &mut env.mem,
    );

    env.framework_state.avfoundation.av_audio_session.shared_instance = Some(instance);
    instance
}


// MARK: - Category

- (id)category {
    env.objc.borrow::<AVAudioSessionHostObject>(this).category
}

- (bool)setCategory:(id)category error:(MutPtr<id>)_error {
    let old = env.objc.borrow::<AVAudioSessionHostObject>(this).category;
    retain(env, category);
    release(env, old);
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).category = category;
    true
}

- (bool)setCategory:(id)category
        withOptions:(AVAudioSessionCategoryOptions)options
              error:(MutPtr<id>)_error {
    let old = env.objc.borrow::<AVAudioSessionHostObject>(this).category;
    retain(env, category);
    release(env, old);

    let host = env.objc.borrow_mut::<AVAudioSessionHostObject>(this);
    host.category = category;
    host.category_options = options;
    true
}

- (bool)setCategory:(id)category
               mode:(id)mode
            options:(AVAudioSessionCategoryOptions)options
              error:(MutPtr<id>)_error {
    let old_category = env.objc.borrow::<AVAudioSessionHostObject>(this).category;
    let old_mode = env.objc.borrow::<AVAudioSessionHostObject>(this).mode;

    retain(env, category);
    retain(env, mode);

    let host = env.objc.borrow_mut::<AVAudioSessionHostObject>(this);
    host.category = category;
    host.mode = mode;
    host.category_options = options;

    release(env, old_category);
    release(env, old_mode);
    true
}

- (AVAudioSessionCategoryOptions)categoryOptions {
    env.objc.borrow::<AVAudioSessionHostObject>(this).category_options
}

// MARK: - Mode

- (id)mode {
    env.objc.borrow::<AVAudioSessionHostObject>(this).mode
}

- (bool)setMode:(id)mode error:(MutPtr<id>)_error {
    let old = env.objc.borrow::<AVAudioSessionHostObject>(this).mode;
    retain(env, mode);
    release(env, old);
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).mode = mode;
    true
}

// MARK: - Activation

- (bool)setActive:(bool)active error:(MutPtr<id>)_error {
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).active = active;
    true
}

- (bool)setActive:(bool)active
      withOptions:(NSUInteger)_options
            error:(MutPtr<id>)_error {
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).active = active;
    true
}

// Apple's
// <https://developer.apple.com/documentation/avfaudio/avaudiosession/1616527-overrideoutputaudioport>:
// "Overrides the current audio route to the built-in speaker or back to the
// default route." The override only affects audio routing, which the host
// output backend chooses on its own, so accept it and report success. Apps
// commonly use this for speaker playback and treat `NO` as a fatal error.
- (bool)overrideOutputAudioPort:(AVAudioSessionPortOverride)port_override
                          error:(MutPtr<id>)_error {
    match port_override {
        AVAudioSessionPortOverrideNone => {
            log_dbg!("AVAudioSession overrideOutputAudioPort: default route")
        }
        AVAudioSessionPortOverrideSpeaker => {
            log_dbg!("AVAudioSession overrideOutputAudioPort: built-in speaker")
        }
        other => log_dbg!("AVAudioSession overrideOutputAudioPort:{:?}", other),
    }
    true
}

// MARK: - Audio properties

- (f64)sampleRate {
    env.objc.borrow::<AVAudioSessionHostObject>(this).preferred_sample_rate
}

- (bool)setPreferredSampleRate:(f64)rate error:(MutPtr<id>)_error {
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).preferred_sample_rate = rate;
    true
}

- (f64)preferredSampleRate {
    env.objc.borrow::<AVAudioSessionHostObject>(this).preferred_sample_rate
}

- (f64)currentHardwareSampleRate {
    msg![env; this sampleRate]
}

- (f64)preferredHardwareSampleRate {
    msg![env; this preferredSampleRate]
}

- (bool)setPreferredHardwareSampleRate:(f64)rate error:(MutPtr<id>)error {
    msg![env; this setPreferredSampleRate:rate error:error]
}

- (f32)currentHardwareOutputVolume {
    msg![env; this outputVolume]
}

- (f64)currentHardwareInputLatency {
    msg![env; this inputLatency]
}

- (f64)currentHardwareOutputLatency {
    msg![env; this outputLatency]
}

- (NSInteger)currentHardwareInputNumberOfChannels {
    msg![env; this maximumInputNumberOfChannels]
}

- (NSInteger)currentHardwareOutputNumberOfChannels {
    msg![env; this maximumOutputNumberOfChannels]
}

- (NSInteger)outputNumberOfChannels {
    // Current channel count on the active output route (stereo for the
    // built-in route). Geometry Dash queries this selector; without a host
    // implementation it fell through to the missing-selector soft-fail
    // (logged a warning and returned 0).
    msg![env; this currentHardwareOutputNumberOfChannels]
}

- (NSInteger)inputNumberOfChannels {
    // Mirror of outputNumberOfChannels for the input side: one built-in mic
    // channel when the host hardware has one.
    msg![env; this currentHardwareInputNumberOfChannels]
}

- (f64)outputLatency {
    0.005 // 5ms — typical for built-in speaker
}

- (f64)inputLatency {
    0.005
}

- (f64)IOBufferDuration {
    env.objc.borrow::<AVAudioSessionHostObject>(this).preferred_io_buffer_duration
}

- (bool)setPreferredIOBufferDuration:(f64)duration error:(MutPtr<id>)_error {
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).preferred_io_buffer_duration = duration;
    true
}

- (f64)preferredIOBufferDuration {
    env.objc.borrow::<AVAudioSessionHostObject>(this).preferred_io_buffer_duration
}

- (NSInteger)maximumInputNumberOfChannels {
    // One built-in mic channel when the host hardware has one.
    if crate::android_media::has_microphone() {
        1
    } else {
        0
    }
}

- (NSInteger)maximumOutputNumberOfChannels {
    2
}

- (f32)inputGain {
    1.0
}

- (bool)isInputGainSettable {
    false
}

- (bool)setInputGain:(f32)_gain error:(MutPtr<id>)_error {
    false
}

- (f32)outputVolume {
    1.0
}

// MARK: - Input / output availability

- (bool)isInputAvailable {
    // Real answer from the host microphone (Android: AudioManager /
    // AudioRecord probe; desktop: no mic bridge, so false).
    crate::android_media::has_microphone()
}

// MARK: - Record permission (iOS 8+)

- (NSUInteger)recordPermission {
    if !crate::android_media::has_microphone() {
        // No host microphone exists: report permanently denied so games
        // don't loop waiting for a grant that can never happen.
        AVAudioSessionRecordPermissionDenied
    } else {
        // The host mic exists; on Android the RECORD_AUDIO prompt is shown
        // when capture actually starts (HostMedia.startMic), so the
        // guest-visible permission is already granted.
        AVAudioSessionRecordPermissionGranted
    }
}

- (bool)requestRecordPermission:(id)response { // void (^)(BOOL granted)
    let granted = crate::android_media::has_microphone();
    if response != nil {
        let response = objc_retainBlock(env, response);
        let invoke_ptr: u32 = env.mem.read(response.cast::<u32>() + 3u32);
        if invoke_ptr != 0 {
            use crate::abi::CallFromHost;
            let invoke = crate::abi::GuestFunction::from_addr_with_thumb_bit(invoke_ptr);
            let block_arg: crate::mem::ConstVoidPtr =
                crate::mem::Ptr::from_bits(response.to_bits()).cast_const();
            let _: () = invoke.call_from_host(env, (block_arg, granted));
        }
        release(env, response);
    }
    true
}

- (bool)isOtherAudioPlaying {
    false
}


// MARK: - Route

- (id)currentRoute { // AVAudioSessionRouteDescription*
    // Return a stub route description.
    let route: id = msg_class![env; AVAudioSessionRouteDescription new];
    autorelease(env, route)
}

- (id)availableInputs { // NSArray<AVAudioSessionPortDescription*>*
    if !crate::android_media::has_microphone() {
        return msg_class![env; NSArray new];
    }
    let port = built_in_mic_port(env);
    let arr: id = msg_class![env; NSArray arrayWithObject:port];
    release(env, port);
    arr
}

- (bool)setPreferredInput:(id)_input error:(MutPtr<id>)_error {
    false
}

- (id)preferredInput {
    nil
}

// MARK: - Delegate (deprecated pre-iOS 6 API)

- (id)delegate {
    env.objc.borrow::<AVAudioSessionHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    // В Objective-C делегаты обычно хранятся как weak ссылки (без
    // retain/release)
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).delegate = delegate;
}

// MARK: - Interruption observer (deprecated pre-iOS 6 API)

- (bool)setActive:(bool)active
            flags:(NSInteger)_flags
            error:(MutPtr<id>)_error {
    env.objc.borrow_mut::<AVAudioSessionHostObject>(this).active = active;
    true
}

@end

// MARK: - AVAudioSessionRouteDescription stub

@implementation AVAudioSessionRouteDescription: NSObject

- (id)inputs {
    // Report built-in microphone as the sole input so games that check
    // `currentRoute.inputs` see that a mic exists.
    let port = built_in_mic_port(env);
    let arr: id = msg_class![env; NSArray arrayWithObject:port];
    release(env, port);
    arr
}

- (id)outputs {
    // Report built-in speaker as the sole output.
    let port: id = msg_class![env; AVAudioSessionPortDescription new];
    let arr: id  = msg_class![env; NSArray arrayWithObject:port];
    release(env, port);
    arr
}

@end

// MARK: - AVAudioSessionPortDescription
@implementation AVAudioSessionPortDescription: NSObject

- (id)portType {
    let default_type = AVAudioSessionPortBuiltInSpeaker;
    let port_type = env
        .objc
        .get_host_object(this)
        .and_then(|h| h.as_any().downcast_ref::<AVAudioSessionPortDescriptionHostObject>())
        .map(|h| h.port_type)
        .unwrap_or(default_type);
    ns_string::get_static_str(env, port_type)
}

- (id)portName {
    let default_name = "Speaker";
    let name = env
        .objc
        .get_host_object(this)
        .and_then(|h| h.as_any().downcast_ref::<AVAudioSessionPortDescriptionHostObject>())
        .map(|h| h.name)
        .unwrap_or(default_name);
    ns_string::get_static_str(env, name)
}

- (id)UID {
    let default_uid = "Built-In Speaker";
    let uid = env
        .objc
        .get_host_object(this)
        .and_then(|h| h.as_any().downcast_ref::<AVAudioSessionPortDescriptionHostObject>())
        .map(|h| h.uid)
        .unwrap_or(default_uid);
    ns_string::get_static_str(env, uid)
}

- (id)channels { // NSArray<AVAudioSessionChannelDescription*>*
    msg_class![env; NSArray new]
}

- (id)dataSources {
    nil
}

- (id)selectedDataSource {
    nil
}

- (id)preferredDataSource {
    nil
}

- (bool)setPreferredDataSource:(id)_source error:(MutPtr<id>)_error {
    false
}

@end

};

// Create a port description for the host's built-in microphone (used by
// `-[AVAudioSession availableInputs]`).
fn built_in_mic_port(env: &mut Environment) -> id {
    let class = env.objc.get_known_class("AVAudioSessionPortDescription", &mut env.mem);
    let host_object = Box::new(AVAudioSessionPortDescriptionHostObject {
        port_type: AVAudioSessionPortBuiltInMic,
        name: "iPhone Microphone",
        uid: "Built-In Microphone",
    });
    let port = env.objc.alloc_object(class, host_object, &mut env.mem);
    autorelease(env, port)
}
