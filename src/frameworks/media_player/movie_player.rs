/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 * If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMoviePlayerController` and `MPMoviePlayerViewController`.
//!
//! `MPMoviePlayerController` uses the bundled OpenH264 decoder for H.264 MP4
//! movies, then presents decoded frames through the Core Animation layer. The
//! controller also reproduces Apple's asynchronous load and playback lifecycle.
//!
//! 1. By default, `initWithContentURL:` starts the legacy asynchronous preload
//!    sequence immediately. Pass `--fix-movie-player` to defer loading
//!    until `prepareToPlay` or `play`, matching the behavior that fixes GTA III.
//! 2. Once the content is determined playable, the player posts
//!    `MPMoviePlayerLoadStateDidChangeNotification`, followed by
//!    `MPMovieNaturalSizeAvailableNotification` and
//!    `MPMovieDurationAvailableNotification`.
//! 3. In legacy mode, `readyForDisplay` also becomes true after metadata loads
//!    when there is no supported video track. With the movie-player fix enabled
//!    it remains false until a decoded video frame is presented; the movie view
//!    stays hidden until then and is not automatically presented fullscreen.
//! 4. For backwards compatibility with apps written against the
//!    `MPMoviePlayerController` introduced in iPhone OS 2 the player also
//!    posts the (now deprecated) `MPMoviePlayerContentPreloadDidFinishNotification`.
//! 5. If `shouldAutoplay` is `YES`, playback transitions to
//!    `MPMoviePlaybackStatePlaying`, posting
//!    `MPMoviePlayerNowPlayingMovieDidChangeNotification` and
//!    `MPMoviePlayerPlaybackStateDidChangeNotification`.
//! 6. Finally `MPMoviePlayerPlaybackDidFinishNotification` is posted with a
//!    `userInfo` dictionary containing
//!    `MPMoviePlayerPlaybackDidFinishReasonUserInfoKey` set to either
//!    `MPMovieFinishReasonPlaybackEnded` (file existed) or
//!    `MPMovieFinishReasonPlaybackError` (file was missing on disk).
//!
//! See:
//! * <https://developer.apple.com/documentation/mediaplayer/mpmoviefinishreason>
//! * <https://developer.apple.com/documentation/mediaplayer/mpmovieloadstate>
//! * <https://developer.apple.com/documentation/mediaplayer/mpmovieplayercontroller>

use super::movie_video::{MovieVideo, MovieVideoInfo};
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_string, ns_url, NSInteger, NSUInteger};
use crate::frameworks::uikit::ui_device::UIDeviceOrientation;
use crate::mem::MutPtr;
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;
use std::collections::{HashMap, VecDeque};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

/// Variants of `MPMoviePlayerController` notifications that we schedule on the
/// run loop. The `id` is the player instance; we keep a +1 retain for each
/// queued entry and release it after posting.
#[derive(Copy, Clone)]
enum PendingNotification {
    LoadStateChange(id),
    NaturalSizeAvailable(id),
    DurationAvailable(id),
    ReadyForDisplayChange(id),
    /// Posted for backwards-compatibility with the original
    /// `MPMoviePlayerController` API.
    ContentPreloadDidFinish(id),
    NowPlayingMovieChange(id),
    PlaybackStateChange(id),
    /// `reason` is one of the `MPMovieFinishReason*` values.
    PlaybackDidFinish {
        player: id,
        reason: NSInteger,
    },
}

impl PendingNotification {
    fn player(&self) -> id {
        match *self {
            PendingNotification::LoadStateChange(p)
            | PendingNotification::NaturalSizeAvailable(p)
            | PendingNotification::DurationAvailable(p)
            | PendingNotification::ReadyForDisplayChange(p)
            | PendingNotification::ContentPreloadDidFinish(p)
            | PendingNotification::NowPlayingMovieChange(p)
            | PendingNotification::PlaybackStateChange(p) => p,
            PendingNotification::PlaybackDidFinish { player, .. } => player,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            PendingNotification::LoadStateChange(_) => MPMoviePlayerLoadStateDidChangeNotification,
            PendingNotification::NaturalSizeAvailable(_) => MPMovieNaturalSizeAvailableNotification,
            PendingNotification::DurationAvailable(_) => MPMovieDurationAvailableNotification,
            PendingNotification::ReadyForDisplayChange(_) => {
                MPMoviePlayerReadyForDisplayDidChangeNotification
            }
            PendingNotification::ContentPreloadDidFinish(_) => {
                MPMoviePlayerContentPreloadDidFinishNotification
            }
            PendingNotification::NowPlayingMovieChange(_) => {
                MPMoviePlayerNowPlayingMovieDidChangeNotification
            }
            PendingNotification::PlaybackStateChange(_) => {
                MPMoviePlayerPlaybackStateDidChangeNotification
            }
            PendingNotification::PlaybackDidFinish { .. } => {
                MPMoviePlayerPlaybackDidFinishNotification
            }
        }
    }
}

#[derive(Default)]
pub struct State {
    /// The currently-active player. Some apps (e.g. NFSU) rely on us holding a
    /// strong reference to keep the player alive between callbacks.
    active_player: Option<id>,
    /// FIFO of scheduled notifications. The previous implementation used a
    /// `swap_remove_back` model, but that altered the relative ordering of
    /// notifications. Some apps observe the order (e.g. `LoadState` must
    /// arrive before `Duration`), so the queue is now drained strictly in
    /// FIFO order. Each entry retains the player by +1.
    pending_notifications: VecDeque<(PendingNotification, Instant)>,
    /// Video decoders for players that are currently playing (see
    /// [super::movie_video]).
    videos: HashMap<id, MovieVideo>,
    /// The `MPMoviePlayerController` owned by each live
    /// `MPMoviePlayerViewController` (its `moviePlayer` property). Each value
    /// is retained and released when the view controller is deallocated.
    /// `UIViewController`'s host object is shared with every subclass, so the
    /// link lives here rather than in a host-object field.
    view_controller_players: HashMap<id, id>,
}
impl State {
    fn get(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.media_player.movie_player
    }
}

pub(super) fn has_active_video(env: &mut Environment) -> bool {
    !State::get(env).videos.is_empty()
}

type MPMovieScalingMode = NSInteger;
const MPMovieScalingModeNone: MPMovieScalingMode = 0;
const MPMovieScalingModeAspectFit: MPMovieScalingMode = 1;
const MPMovieScalingModeAspectFill: MPMovieScalingMode = 2;
const MPMovieScalingModeFill: MPMovieScalingMode = 3;
type MPMovieControlStyle = NSInteger;
type MPMovieSourceType = NSInteger;
type MPMovieRepeatMode = NSInteger;
/// `MPMovieRepeatModeOne`: the movie loops until it is stopped.
const MPMovieRepeatModeOne: MPMovieRepeatMode = 1;

type MPMoviePlaybackState = NSInteger;
const MPMoviePlaybackStateStopped: MPMoviePlaybackState = 0;
const MPMoviePlaybackStatePlaying: MPMoviePlaybackState = 1;
const MPMoviePlaybackStatePaused: MPMoviePlaybackState = 2;

/// `MPMovieLoadState` bitmask, matching Apple's `MPMoviePlayerController.h`.
type MPMovieLoadState = NSUInteger;
const MPMovieLoadStateUnknown: MPMovieLoadState = 0;
const MPMovieLoadStatePlayable: MPMovieLoadState = 1 << 0;
const MPMovieLoadStatePlaythroughOK: MPMovieLoadState = 1 << 1;
#[allow(dead_code)]
const MPMovieLoadStateStalled: MPMovieLoadState = 1 << 2;

/// `MPMovieFinishReason` from Apple's MediaPlayer framework.
const MPMovieFinishReasonPlaybackEnded: NSInteger = 0;
const MPMovieFinishReasonPlaybackError: NSInteger = 1;
#[allow(dead_code)]
const MPMovieFinishReasonUserExited: NSInteger = 2;

// Notification names — values copy Apple's symbol names so that
// `[NSNotificationCenter addObserverForName:]` callers match the right key.
pub const MPMoviePlayerPlaybackDidFinishNotification: &str =
    "MPMoviePlayerPlaybackDidFinishNotification";
pub const MPMoviePlayerContentPreloadDidFinishNotification: &str =
    "MPMoviePlayerContentPreloadDidFinishNotification";
pub const MPMoviePlayerScalingModeDidChangeNotification: &str =
    "MPMoviePlayerScalingModeDidChangeNotification";
pub const MPMoviePlayerPlaybackStateDidChangeNotification: &str =
    "MPMoviePlayerPlaybackStateDidChangeNotification";
pub const MPMoviePlayerLoadStateDidChangeNotification: &str =
    "MPMoviePlayerLoadStateDidChangeNotification";
pub const MPMoviePlayerNowPlayingMovieDidChangeNotification: &str =
    "MPMoviePlayerNowPlayingMovieDidChangeNotification";
pub const MPMoviePlayerWillEnterFullscreenNotification: &str =
    "MPMoviePlayerWillEnterFullscreenNotification";
pub const MPMoviePlayerDidEnterFullscreenNotification: &str =
    "MPMoviePlayerDidEnterFullscreenNotification";
pub const MPMoviePlayerWillExitFullscreenNotification: &str =
    "MPMoviePlayerWillExitFullscreenNotification";
pub const MPMoviePlayerDidExitFullscreenNotification: &str =
    "MPMoviePlayerDidExitFullscreenNotification";
const MPMovieDurationAvailableNotification: &str = "MPMovieDurationAvailableNotification";
const MPMoviePlayerReadyForDisplayDidChangeNotification: &str =
    "MPMoviePlayerReadyForDisplayDidChangeNotification";
const MPMovieNaturalSizeAvailableNotification: &str = "MPMovieNaturalSizeAvailableNotification";
const MPMovieMediaTypesAvailableNotification: &str = "MPMovieMediaTypesAvailableNotification";
const MPMovieSourceTypeAvailableNotification: &str = "MPMovieSourceTypeAvailableNotification";
const MPMoviePlayerPlaybackDidFinishReasonUserInfoKey: &str =
    "MPMoviePlayerPlaybackDidFinishReasonUserInfoKey";
// `MPMediaPlayback` (a protocol adopted by `MPMoviePlayerController`)
// posts this notification when `-isPreparedToPlay` flips. Declared in
// `MPMediaPlayback.h`, iOS 3.2+. Canonical NSString value matches the
// symbol name, see
// <https://developer.apple.com/documentation/mediaplayer/mpmediaplaybackispreparedtoplaydidchangenotification>.
const MPMediaPlaybackIsPreparedToPlayDidChangeNotification: &str =
    "MPMediaPlaybackIsPreparedToPlayDidChangeNotification";
// `requestThumbnailImagesAtTimes:timeOption:` result-delivery
// notification + `userInfo` keys. Declared in `MPMoviePlayerController.h`
// (iOS 3.2+, deprecated in 9.0). See
// <https://developer.apple.com/documentation/mediaplayer/mpmovieplayerthumbnailimagerequestdidfinishnotification>.
const MPMoviePlayerThumbnailImageRequestDidFinishNotification: &str =
    "MPMoviePlayerThumbnailImageRequestDidFinishNotification";
const MPMoviePlayerThumbnailImageKey: &str = "MPMoviePlayerThumbnailImageKey";
const MPMoviePlayerThumbnailErrorKey: &str = "MPMoviePlayerThumbnailErrorKey";
const MPMoviePlayerThumbnailTimeKey: &str = "MPMoviePlayerThumbnailTimeKey";

/// `NSNotificationName` values and other constants.
pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMoviePlayerPlaybackDidFinishNotification",
        HostConstant::NSString(MPMoviePlayerPlaybackDidFinishNotification),
    ),
    (
        "_MPMoviePlayerContentPreloadDidFinishNotification",
        HostConstant::NSString(MPMoviePlayerContentPreloadDidFinishNotification),
    ),
    (
        "_MPMoviePlayerScalingModeDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerScalingModeDidChangeNotification),
    ),
    (
        "_MPMoviePlayerPlaybackStateDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerPlaybackStateDidChangeNotification),
    ),
    (
        "_MPMoviePlayerLoadStateDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerLoadStateDidChangeNotification),
    ),
    (
        "_MPMoviePlayerNowPlayingMovieDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerNowPlayingMovieDidChangeNotification),
    ),
    (
        "_MPMoviePlayerWillEnterFullscreenNotification",
        HostConstant::NSString(MPMoviePlayerWillEnterFullscreenNotification),
    ),
    (
        "_MPMoviePlayerDidEnterFullscreenNotification",
        HostConstant::NSString(MPMoviePlayerDidEnterFullscreenNotification),
    ),
    (
        "_MPMoviePlayerWillExitFullscreenNotification",
        HostConstant::NSString(MPMoviePlayerWillExitFullscreenNotification),
    ),
    (
        "_MPMoviePlayerDidExitFullscreenNotification",
        HostConstant::NSString(MPMoviePlayerDidExitFullscreenNotification),
    ),
    (
        "_MPMoviePlayerPlaybackDidFinishReasonUserInfoKey",
        HostConstant::NSString(MPMoviePlayerPlaybackDidFinishReasonUserInfoKey),
    ),
    (
        "_MPMovieDurationAvailableNotification",
        HostConstant::NSString(MPMovieDurationAvailableNotification),
    ),
    (
        "_MPMoviePlayerReadyForDisplayDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerReadyForDisplayDidChangeNotification),
    ),
    (
        "_MPMovieNaturalSizeAvailableNotification",
        HostConstant::NSString(MPMovieNaturalSizeAvailableNotification),
    ),
    (
        "_MPMovieMediaTypesAvailableNotification",
        HostConstant::NSString(MPMovieMediaTypesAvailableNotification),
    ),
    (
        "_MPMovieSourceTypeAvailableNotification",
        HostConstant::NSString(MPMovieSourceTypeAvailableNotification),
    ),
    (
        "_MPMediaPlaybackIsPreparedToPlayDidChangeNotification",
        HostConstant::NSString(MPMediaPlaybackIsPreparedToPlayDidChangeNotification),
    ),
    (
        "_MPMoviePlayerThumbnailImageRequestDidFinishNotification",
        HostConstant::NSString(MPMoviePlayerThumbnailImageRequestDidFinishNotification),
    ),
    (
        "_MPMoviePlayerThumbnailImageKey",
        HostConstant::NSString(MPMoviePlayerThumbnailImageKey),
    ),
    (
        "_MPMoviePlayerThumbnailErrorKey",
        HostConstant::NSString(MPMoviePlayerThumbnailErrorKey),
    ),
    (
        "_MPMoviePlayerThumbnailTimeKey",
        HostConstant::NSString(MPMoviePlayerThumbnailTimeKey),
    ),
];

#[derive(Default)]
struct MPMoviePlayerControllerHostObject {
    // NSURL *
    content_url: id,
    // UIView *
    view: id,
    /// The temporary fullscreen presenter used by the legacy behavior for an
    /// unparented movie view. The opt-in GTA III path keeps the view in the
    /// app's hierarchy instead.
    presentation_view_controller: id,
    background_view: id,
    audio_player: id,

    scaling_mode: MPMovieScalingMode,
    fullscreen: bool,
    fullscreen_original_frame: Option<CGRect>,
    control_style: MPMovieControlStyle,
    source_type: MPMovieSourceType,
    repeat_mode: MPMovieRepeatMode,
    should_autoplay: bool,
    /// `initialPlaybackTime`: where a play run that starts from the stopped
    /// state begins, in seconds. Negative (the default, -1) means "from the
    /// natural start of the movie".
    initial_playback_time: f64,
    /// `endPlaybackTime`: where playback ends, in seconds. Negative (the
    /// default, -1) means "at the natural end of the movie".
    end_playback_time: f64,
    playback_state: MPMoviePlaybackState,
    /// `currentPlaybackRate`: how fast the playback clock advances, where 1.0
    /// is normal speed. Only meaningful while `playback_state` is
    /// `MPMoviePlaybackStatePlaying`; Apple documents that the property reads
    /// back as 0.0 whenever the movie is paused or stopped.
    playback_rate: f64,
    load_state: MPMovieLoadState,
    /// Natural dimensions reported by the supported H.264 video track.
    video_info: Option<MovieVideoInfo>,
    /// Placeholder dimensions used when the movie has no supported video track.
    natural_size: CGSize,
    duration: f64,
    /// `true` once `duration` holds the movie's real (encoded) duration
    /// rather than [PLACEHOLDER_DURATION].
    duration_known: bool,
    /// Playback clock: position (seconds) accumulated before the current
    /// play run, and when the current run started (`None` unless playing).
    /// We can't decode the video, but apps still time their UI against
    /// `currentPlaybackTime` (e.g. fading a menu in over a background movie).
    clock_offset: f64,
    clock_started: Option<Instant>,
    ready_for_display: bool,
    /// `true` once we have scheduled the post-load notification burst, so
    /// `prepareToPlay` / `play` / `setContentURL:` don't queue it twice.
    preload_scheduled: bool,
    /// `true` once a `PlaybackDidFinish` notification has been queued for
    /// the current content URL, to prevent duplicates when both autoplay
    /// and an explicit `play` happen.
    finish_scheduled: bool,
}
impl HostObject for MPMoviePlayerControllerHostObject {}

/// Default natural size for movies we cannot decode. Matches the 480x320 frame
/// of the iPhone (4:3 letterboxed).
const PLACEHOLDER_NATURAL_SIZE: CGSize = CGSize {
    width: 480.0,
    height: 320.0,
};
/// Fallback duration when we don't know the real one. Real Apple movies report
/// the encoded duration here; we use a small non-zero value to avoid divide-by-
/// zero crashes in app code that builds a progress bar from `currentPlaybackTime
/// / duration`.
const PLACEHOLDER_DURATION: f64 = 1.0;

/// Read the duration (in seconds) of an MPEG-4/QuickTime file from the `mvhd`
/// box inside its top-level `moov` box. Returns `None` if the file can't be
/// parsed. See ISO/IEC 14496-12, "Movie Header Box".
pub(super) fn read_mp4_duration<F: Read + Seek>(file: &mut F) -> Option<f64> {
    /// Reads a box header, returning (box start, box type, box size).
    fn box_header<F: Read + Seek>(file: &mut F) -> Option<(u64, [u8; 4], u64)> {
        let start = file.stream_position().ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let mut size = u32::from_be_bytes(header[0..4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = header[4..8].try_into().unwrap();
        let mut header_len = 8;
        if size == 1 {
            let mut large = [0u8; 8];
            file.read_exact(&mut large).ok()?;
            size = u64::from_be_bytes(large);
            header_len = 16;
        } else if size == 0 {
            // Box extends to the end of the file.
            let end = file.seek(SeekFrom::End(0)).ok()?;
            file.seek(SeekFrom::Start(start + header_len)).ok()?;
            size = end - start;
        }
        if size < header_len {
            return None;
        }
        Some((start, kind, size))
    }

    file.seek(SeekFrom::Start(0)).ok()?;
    let (moov_start, moov_size) = loop {
        let (start, kind, size) = box_header(file)?;
        if &kind == b"moov" {
            break (start, size);
        }
        file.seek(SeekFrom::Start(start + size)).ok()?;
    };
    loop {
        let (start, kind, size) = box_header(file)?;
        if start >= moov_start + moov_size {
            return None;
        }
        if &kind == b"mvhd" {
            let mut version_flags = [0u8; 4];
            file.read_exact(&mut version_flags).ok()?;
            let (timescale, duration) = if version_flags[0] == 1 {
                // creation/modification time are 64-bit in version 1
                let mut b = [0u8; 28];
                file.read_exact(&mut b).ok()?;
                (
                    u32::from_be_bytes(b[16..20].try_into().unwrap()),
                    u64::from_be_bytes(b[20..28].try_into().unwrap()),
                )
            } else {
                let mut b = [0u8; 16];
                file.read_exact(&mut b).ok()?;
                (
                    u32::from_be_bytes(b[8..12].try_into().unwrap()),
                    u32::from_be_bytes(b[12..16].try_into().unwrap()) as u64,
                )
            };
            if timescale == 0 || duration == 0 {
                return None;
            }
            return Some(duration as f64 / timescale as f64);
        }
        file.seek(SeekFrom::Start(start + size)).ok()?;
    }
}

/// The explicit `endPlaybackTime`, if one is set and lies inside the movie.
///
/// Per Apple's `MPMoviePlayerController` reference, `endPlaybackTime` is "the
/// end time (measured in seconds) for playback of the movie"; the default,
/// -1, plays to the natural end. An end time at or past the movie's known
/// duration is the same as the natural end.
fn explicit_end_time(host: &MPMoviePlayerControllerHostObject) -> Option<f64> {
    let end = host.end_playback_time;
    if !end.is_finite() || end <= 0.0 {
        return None;
    }
    if host.duration_known && host.duration > 0.0 && end >= host.duration {
        return None;
    }
    Some(end)
}

/// Where the current play-through stops: the explicit `endPlaybackTime` if
/// set, otherwise the movie's duration.
fn effective_end_time(host: &MPMoviePlayerControllerHostObject) -> f64 {
    explicit_end_time(host).unwrap_or(host.duration)
}

/// Current position of the player's playback clock, in seconds. Wraps around
/// for `MPMovieRepeatModeOne`, otherwise stops at the end of the movie (or at
/// `endPlaybackTime`, if set).
fn playback_position(host: &MPMoviePlayerControllerHostObject) -> f64 {
    // The clock runs at `currentPlaybackRate`, so a 2.0 rate makes
    // `currentPlaybackTime` advance twice as fast.
    let elapsed = host
        .clock_started
        .map_or(0.0, |started| started.elapsed().as_secs_f64())
        * host.playback_rate;
    let position = host.clock_offset + elapsed;
    if host.duration <= 0.0 && explicit_end_time(host).is_none() {
        position
    } else if host.repeat_mode == MPMovieRepeatModeOne {
        // The video decoder loops over the whole file, so the clock has to
        // as well; `endPlaybackTime` only ends non-repeating playback.
        if host.duration > 0.0 {
            position % host.duration
        } else {
            position
        }
    } else {
        position.min(effective_end_time(host))
    }
}

/// Whether to use the opt-in movie-player behavior that fixes GTA III while
/// preserving the legacy lifecycle for other games by default.
fn fix_movie_player_enabled(env: &Environment) -> bool {
    env.options.fix_movie_player
}

/// Ensure the player has a valid dummy view, creating one lazily if needed.
/// Returns the view id (always non-nil after this call).
fn ensure_view(env: &mut Environment, this: id) -> id {
    let existing = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .view;
    if existing != nil {
        return existing;
    }
    let view_alloc: id = msg_class![env; UIView alloc];
    let view: id = msg![env; view_alloc init];
    if fix_movie_player_enabled(env) {
        () = msg![env; view setHidden:true];
    }
    retain(env, view);
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .view = view;
    view
}

fn ensure_background_view(env: &mut Environment, this: id) -> id {
    let existing = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .background_view;
    if existing != nil {
        return existing;
    }
    let view_alloc: id = msg_class![env; UIView alloc];
    let view: id = msg![env; view_alloc init];
    retain(env, view);
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .background_view = view;
    view
}

/// Reproduce the legacy behavior for movies whose view has not been attached
/// by the app: add it to the app window and present it fullscreen on the first
/// decoded frame. The GTA III opt-in path deliberately does not do this.
fn present_unparented_movie_view(env: &mut Environment, player: id, view: id) {
    if view == nil {
        return;
    }
    let superview: id = msg![env; view superview];
    if superview != nil {
        return;
    }
    let presentation_view_controller = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(player)
        .presentation_view_controller;
    if presentation_view_controller != nil {
        return;
    }

    let app: id = msg_class![env; UIApplication sharedApplication];
    let mut window: id = msg![env; app keyWindow];
    if window == nil {
        let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
        for candidate in windows.into_iter().rev() {
            let hidden: bool = msg![env; candidate isHidden];
            if !hidden {
                window = candidate;
                break;
            }
        }
    }
    if window == nil {
        return;
    }

    let mut presenter: id = msg![env; window rootViewController];
    if presenter == nil {
        let content_view: id = msg![env; window contentView];
        if content_view != nil {
            let candidate: id = msg![env; content_view nextResponder];
            let view_controller_class: id = msg_class![env; UIViewController class];
            if candidate != nil && msg![env; candidate isKindOfClass:view_controller_class] {
                presenter = candidate;
            }
        }
    }
    if presenter != nil {
        for _ in 0..16 {
            let presented: id = msg![env; presenter presentedViewController];
            if presented == nil {
                break;
            }
            presenter = presented;
        }
    }

    let bounds: CGRect = msg![env; window bounds];
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return;
    }
    let controller_alloc: id = msg_class![env; UIViewController alloc];
    let controller: id = msg![env; controller_alloc init];
    () = msg![env; controller setView:view];
    () = msg![env; view setFrame:bounds];
    let resize_mask: NSUInteger = (1 << 1) | (1 << 4);
    () = msg![env; view setAutoresizingMask:resize_mask];

    if presenter != nil {
        () = msg![env; presenter presentModalViewController:controller animated:false];
    }
    let mut superview: id = msg![env; view superview];
    if superview == nil {
        () = msg![env; window addSubview:view];
        superview = msg![env; view superview];
    }
    if superview == nil {
        () = msg![env; controller setView:nil];
        release(env, controller);
        return;
    }

    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(player)
        .presentation_view_controller = controller;
    log!(
        "MPMoviePlayerController {:?}: presenting unparented movie view fullscreen",
        player
    );
}

fn dismiss_unparented_movie_view(env: &mut Environment, player: id) {
    let (controller, view) = {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(player);
        (
            std::mem::replace(&mut host.presentation_view_controller, nil),
            host.view,
        )
    };
    if controller == nil {
        return;
    }

    let presenter: id = msg![env; controller presentingViewController];
    let presented: id = if presenter != nil {
        msg![env; presenter presentedViewController]
    } else {
        nil
    };
    if presenter != nil && presented == controller {
        () = msg![env; presenter dismissModalViewControllerAnimated:false];
    } else if view != nil {
        let superview: id = msg![env; view superview];
        if superview != nil {
            () = msg![env; view removeFromSuperview];
        }
    }
    () = msg![env; controller setView:nil];
    release(env, controller);
}

fn clear_movie_frame(env: &mut Environment, player: id) {
    let view = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(player)
        .view;
    if view != nil {
        let layer: id = msg![env; view layer];
        crate::frameworks::core_animation::ca_eagl_layer::clear_presented_pixels(env, layer);
    }
}

fn set_movie_view_hidden(env: &mut Environment, player: id, hidden: bool) {
    let view = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(player)
        .view;
    if view != nil {
        () = msg![env; view setHidden:hidden];
    }
}

/// Enqueue a pending notification, retaining the player so it stays alive
/// until [handle_players] dispatches it.
fn enqueue(env: &mut Environment, notification: PendingNotification, when: Instant) {
    retain(env, notification.player());
    State::get(env)
        .pending_notifications
        .push_back((notification, when));
}

fn cancel_pending_playback_finish(env: &mut Environment, player: id) {
    let removed = {
        let pending = &mut State::get(env).pending_notifications;
        let original_len = pending.len();
        pending.retain(|(notification, _)| {
            !matches!(
                *notification,
                PendingNotification::PlaybackDidFinish {
                    player: candidate,
                    ..
                } if candidate == player
            )
        });
        original_len - pending.len()
    };
    for _ in 0..removed {
        release(env, player);
    }
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(player)
        .finish_scheduled = false;
}

fn schedule_playback_finish(
    env: &mut Environment,
    player: id,
    start_at: Instant,
    use_media_duration: bool,
) {
    let finish_at = {
        let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
        let end = effective_end_time(host);
        // An explicit `endPlaybackTime` is honoured even when we can't decode
        // the video track: the app asked for playback to stop at that time.
        if use_media_duration
            && (host.video_info.is_some() || explicit_end_time(host).is_some())
            && end.is_finite()
            && end > 0.0
        {
            let remaining = (end - playback_position(host))
                .max(0.0)
                .min(31_536_000.0);
            // `remaining` is in movie seconds; the clock advances at
            // `currentPlaybackRate`, so the wall-clock delay shrinks (or
            // grows) with the rate.
            let rate = if host.playback_rate.is_finite() && host.playback_rate > 0.0 {
                host.playback_rate
            } else {
                1.0
            };
            start_at + Duration::from_secs_f64(remaining / rate)
        } else {
            start_at + Duration::from_millis(150)
        }
    };
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(player)
        .finish_scheduled = true;
    enqueue(
        env,
        PendingNotification::PlaybackDidFinish {
            player,
            reason: MPMovieFinishReasonPlaybackEnded,
        },
        finish_at,
    );
}

/// Schedule the asynchronous lifecycle of a newly-prepared movie. This is the
/// sequence Apple's `MPMoviePlayerController` posts once it has determined the
/// content is playable. We add small staggered delays so observers that watch
/// for these notifications via `addObserverForName:` (rather than KVO) can
/// distinguish them.
fn schedule_preload_sequence(env: &mut Environment, this: id) {
    {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        if host.preload_scheduled {
            return;
        }
        host.preload_scheduled = true;
    }

    // Check whether the file actually exists on disk so we can report a
    // meaningful finish reason later.
    let playback_error_reason = {
        let url = env
            .objc
            .borrow::<MPMoviePlayerControllerHostObject>(this)
            .content_url;
        if url == nil {
            Some(MPMovieFinishReasonPlaybackError)
        } else {
            let path = ns_url::to_rust_path(env, url);
            if env.fs.is_file(&path) {
                None
            } else {
                log!(
                    "MPMoviePlayerController: content URL {:?} is not a readable \
                     file in the guest filesystem; will report MPMovieFinishReasonPlaybackError.",
                    path.as_str()
                );
                Some(MPMovieFinishReasonPlaybackError)
            }
        }
    };

    if playback_error_reason.is_none() {
        let url = env
            .objc
            .borrow::<MPMoviePlayerControllerHostObject>(this)
            .content_url;
        let path = ns_url::to_rust_path(env, url);
        if let Ok(bytes) = env.fs.read(&path) {
            if let Some(duration) = read_mp4_duration(&mut Cursor::new(bytes.as_slice())) {
                log_dbg!(
                    "MPMoviePlayerController {:?}: {:?} is {:.2}s long",
                    this,
                    path.as_str(),
                    duration
                );
                let host = env
                    .objc
                    .borrow_mut::<MPMoviePlayerControllerHostObject>(this);
                host.duration = duration;
                host.duration_known = true;
            }
            if let Some(info) = MovieVideo::probe(&bytes) {
                log_dbg!(
                    "MPMoviePlayerController {:?}: H.264 video is {}x{}",
                    this,
                    info.width,
                    info.height
                );
                let host = env
                    .objc
                    .borrow_mut::<MPMoviePlayerControllerHostObject>(this);
                host.natural_size = CGSize {
                    width: info.width as f32,
                    height: info.height as f32,
                };
                host.video_info = Some(info);
            }
        }
    }

    let now = Instant::now();
    let base = now + Duration::from_millis(20);

    if playback_error_reason.is_some() {
        // For a missing file, real iOS still posts a single finish
        // notification (with reason = PlaybackError); load-state etc. are
        // never posted. Mirror that.
        env.objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
            .finish_scheduled = true;
        enqueue(
            env,
            PendingNotification::PlaybackDidFinish {
                player: this,
                reason: MPMovieFinishReasonPlaybackError,
            },
            base,
        );
        return;
    }

    // Promote loadState to "Playable | PlaythroughOK".
    {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        host.load_state = MPMovieLoadStatePlayable | MPMovieLoadStatePlaythroughOK;
    }
    enqueue(env, PendingNotification::LoadStateChange(this), base);

    let video_supported = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .video_info
        .is_some();
    let metadata_at = base + Duration::from_millis(10);
    enqueue(
        env,
        PendingNotification::NaturalSizeAvailable(this),
        metadata_at,
    );
    enqueue(
        env,
        PendingNotification::DurationAvailable(this),
        metadata_at,
    );
    if !video_supported && !fix_movie_player_enabled(env) {
        env.objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
            .ready_for_display = true;
        enqueue(
            env,
            PendingNotification::ReadyForDisplayChange(this),
            metadata_at,
        );
    }

    // Legacy `ContentPreloadDidFinish` (iPhone OS 2 API).
    enqueue(
        env,
        PendingNotification::ContentPreloadDidFinish(this),
        metadata_at,
    );

    // If shouldAutoplay is the default (true) and the app didn't disable it,
    // transition to "playing" automatically.
    let should_autoplay = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .should_autoplay;
    if should_autoplay {
        schedule_playback(env, this, metadata_at + Duration::from_millis(10));
    } else if !video_supported {
        schedule_playback_finish(env, this, metadata_at, false);
    }
}

fn schedule_playback(env: &mut Environment, this: id, start_at: Instant) {
    cancel_pending_playback_finish(env, this);
    {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        let starting_from_stop = host.playback_state == MPMoviePlaybackStateStopped
            && host.clock_started.is_none();
        host.playback_state = MPMoviePlaybackStatePlaying;
        // `play` starts at normal speed unless the app already asked for a
        // different `currentPlaybackRate`. The property reads back as 0.0
        // outside the playing state, so a stale 0.0 must not stall the clock.
        if !host.playback_rate.is_finite() || host.playback_rate <= 0.0 {
            host.playback_rate = 1.0;
        }
        if starting_from_stop
            && host.initial_playback_time.is_finite()
            && host.initial_playback_time > 0.0
        {
            // `initialPlaybackTime`: "The time, specified in seconds within
            // the video timeline, when playback should start."
            host.clock_offset = host.initial_playback_time;
        }
        if host.clock_started.is_none() {
            host.clock_started = Some(start_at);
        }
    }

    enqueue(
        env,
        PendingNotification::NowPlayingMovieChange(this),
        start_at,
    );
    enqueue(
        env,
        PendingNotification::PlaybackStateChange(this),
        start_at,
    );
    schedule_playback_finish(env, this, start_at, true);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMoviePlayerController: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(MPMoviePlayerControllerHostObject {
        content_url: nil,
        view: nil,
        presentation_view_controller: nil,
        background_view: nil,
        audio_player: nil,

        scaling_mode: 0,
        fullscreen: false,
        fullscreen_original_frame: None,
        control_style: 0,
        source_type: 0,
        repeat_mode: 0,
        should_autoplay: true,

        initial_playback_time: -1.0,
        end_playback_time: -1.0,
        playback_state: MPMoviePlaybackStateStopped,
        playback_rate: 1.0,
        load_state: MPMovieLoadStateUnknown,
        video_info: None,
        natural_size: PLACEHOLDER_NATURAL_SIZE,
        duration: PLACEHOLDER_DURATION,
        duration_known: false,
        clock_offset: 0.0,
        clock_started: None,
        ready_for_display: false,
        preload_scheduled: false,
        finish_scheduled: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithContentURL:(id)url { // NSURL*
    log_dbg!(
        "[(MPMoviePlayerController*){:?} initWithContentURL:{:?} ({:?})]",
        this,
        url,
        ns_url::to_rust_path(env, url),
    );

    let this: id = msg![env; this init];
    retain(env, url);

    {
        let host = env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        host.content_url = url;
    }

    ensure_view(env, this);
    ensure_background_view(env, this);

    if !fix_movie_player_enabled(env) {
        // Legacy behavior: loading starts as soon as the controller is created.
        schedule_preload_sequence(env, this);
    }

    this
}

- (())setContentURL:(id)url { // NSURL*
    log_dbg!(
        "[(MPMoviePlayerController*){:?} setContentURL:{:?} ({:?})]",
        this,
        url,
        ns_url::to_rust_path(env, url),
    );

    cancel_pending_playback_finish(env, this);
    State::get(env).videos.remove(&this);
    let use_movie_player_fix = fix_movie_player_enabled(env);
    if use_movie_player_fix {
        clear_movie_frame(env, this);
        set_movie_view_hidden(env, this, true);
    }
    stop_movie_audio(env, this);
    if !use_movie_player_fix {
        dismiss_unparented_movie_view(env, this);
    }

    let (old_url, was_preloaded) = {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        let old = host.content_url;
        host.content_url = url;
        host.video_info = None;
        host.natural_size = PLACEHOLDER_NATURAL_SIZE;
        host.duration = PLACEHOLDER_DURATION;
        host.duration_known = false;
        host.clock_offset = 0.0;
        host.clock_started = None;
        host.preload_scheduled = false;
        host.finish_scheduled = false;
        host.load_state = MPMovieLoadStateUnknown;
        host.ready_for_display = false;
        let was = host.playback_state == MPMoviePlaybackStatePlaying;
        host.playback_state = MPMoviePlaybackStateStopped;
        host.playback_rate = 1.0;
        (old, was)
    };
    retain(env, url);
    if old_url != nil {
        release(env, old_url);
    }

    if was_preloaded {
        // A state change is observable when we tear down the previous
        // playback session.
        enqueue(env, PendingNotification::PlaybackStateChange(this), Instant::now());
    }

    schedule_preload_sequence(env, this);
}

- (())dealloc {
    State::get(env).videos.remove(&this);
    let use_movie_player_fix = fix_movie_player_enabled(env);
    if use_movie_player_fix {
        clear_movie_frame(env, this);
        set_movie_view_hidden(env, this, true);
    }
    stop_movie_audio(env, this);
    if !use_movie_player_fix {
        dismiss_unparented_movie_view(env, this);
    }

    // No need to drain pending notifications: each pending entry holds a
    // +1 retain on the player, so dealloc can only run once all queued
    // notifications have been delivered and released.
    let url = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .content_url;
    release(env, url);

    let view = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .view;
    release(env, view);

    let bg_view = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .background_view;
    release(env, bg_view);

    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)contentURL {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .content_url
}

- (id)backgroundColor {
    msg_class![env; UIColor blackColor]
}
- (())setBackgroundColor:(id)_color { // UIColor*
    // Background color is a visual property; we have no movie view to tint.
}

// --- Scaling mode ---

- (MPMovieScalingMode)scalingMode {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .scaling_mode
}
- (())setScalingMode:(MPMovieScalingMode)mode {
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .scaling_mode = mode;
}

// --- Control style ---

- (MPMovieControlStyle)controlStyle {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .control_style
}
- (())setControlStyle:(MPMovieControlStyle)style {
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .control_style = style;
}

// --- Source type ---

- (MPMovieSourceType)movieSourceType {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .source_type
}
- (())setMovieSourceType:(MPMovieSourceType)source_type {
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .source_type = source_type;
}

// --- Repeat mode ---

- (MPMovieRepeatMode)repeatMode {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .repeat_mode
}
- (())setRepeatMode:(MPMovieRepeatMode)mode {
    log_dbg!("[(MPMoviePlayerController*){:?} setRepeatMode:{}]", this, mode);
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .repeat_mode = mode;
}

// --- Autoplay ---

- (bool)shouldAutoplay {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .should_autoplay
}
- (())setShouldAutoplay:(bool)autoplay {
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .should_autoplay = autoplay;
}

// --- Misc setters ---

- (())setUseApplicationAudioSession:(bool)_use_session {
    // No audio session integration needed in the emulator.
}

- (bool)isFullscreen {
    if fix_movie_player_enabled(env) {
        env.objc
            .borrow::<MPMoviePlayerControllerHostObject>(this)
            .fullscreen
    } else {
        // The legacy implementation had no explicit fullscreen state.
        false
    }
}

- (())setFullscreen:(bool)fullscreen {
    if fix_movie_player_enabled(env) {
        set_movie_fullscreen(env, this, fullscreen);
    }
}

- (())setFullscreen:(bool)fullscreen animated:(bool)_animated {
    if fix_movie_player_enabled(env) {
        set_movie_fullscreen(env, this, fullscreen);
    }
}

// --- View ---

// Returns the player's backing view. Created lazily if initWithContentURL:
// somehow failed to allocate it, so this always returns a non-nil UIView.
- (id)view {
    ensure_view(env, this)
}

- (id)backgroundView {
    ensure_background_view(env, this)
}
- (())setBackgroundView:(id)view {
    let old = env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).background_view;
    if old != nil {
        release(env, old);
    }
    retain(env, view);
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).background_view = view;
}

// --- Playback state / time ---

- (MPMoviePlaybackState)playbackState {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .playback_state
}

- (MPMovieLoadState)loadState {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .load_state
}

// Apple's
// <https://developer.apple.com/documentation/mediaplayer/mpmovieplayercontroller/1620773-currentplaybackrate>:
// "The current playback rate for the movie. ... A value of 0.0 means the
// movie is paused; 1.0 is normal playback. ... This property is set to 0.0
// when the movie is paused or stopped."
- (f32)currentPlaybackRate {
    let host = env
        .objc
        .borrow::<MPMoviePlayerControllerHostObject>(this);
    if host.playback_state == MPMoviePlaybackStatePlaying {
        host.playback_rate as f32
    } else {
        0.0
    }
}
- (())setCurrentPlaybackRate:(f32)rate {
    // Negative and non-finite rates are meaningless for playback; iOS
    // clamps to 0.0 (paused) rather than storing garbage in the clock.
    let rate = if rate.is_finite() {
        rate.max(0.0) as f64
    } else {
        0.0
    };
    let (previous_state, was_finish_scheduled) = {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        if host.playback_state == MPMoviePlaybackStatePlaying {
            // Rebase the clock so that the new rate only affects time
            // from now on — time already played keeps its value.
            host.clock_offset = playback_position(host);
            host.clock_started = Some(Instant::now());
        }
        host.playback_rate = rate;
        (host.playback_state, host.finish_scheduled)
    };
    let was_playing = previous_state == MPMoviePlaybackStatePlaying;
    if rate == 0.0 {
        if was_playing {
            () = msg![env; this pause];
        }
    } else if previous_state == MPMoviePlaybackStatePaused {
        // Setting a non-zero rate while paused resumes playback.
        () = msg![env; this play];
    } else if was_playing && was_finish_scheduled {
        // The pending end of playback was computed for the old rate.
        cancel_pending_playback_finish(env, this);
        schedule_playback_finish(env, this, Instant::now(), true);
    }
}

- (CGSize)naturalSize {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .natural_size
}

- (f64)currentPlaybackTime {
    playback_position(env.objc.borrow::<MPMoviePlayerControllerHostObject>(this))
}
- (())setCurrentPlaybackTime:(f64)time {
    let (audio_player, playing) = {
        let host = env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        host.clock_offset = time.max(0.0);
        if host.clock_started.is_some() {
            host.clock_started = Some(Instant::now());
        }
        (host.audio_player, host.playback_state == MPMoviePlaybackStatePlaying)
    };
    if audio_player != nil {
        let current_time = env
            .objc
            .borrow::<MPMoviePlayerControllerHostObject>(this)
            .clock_offset;
        () = msg![env; audio_player setCurrentTime:current_time];
    }
    if playing {
        let start_at = Instant::now();
        cancel_pending_playback_finish(env, this);
        schedule_playback_finish(env, this, start_at, true);
    }
}

- (f64)initialPlaybackTime {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .initial_playback_time
}
- (())setInitialPlaybackTime:(f64)time {
    env.objc
        .borrow_mut::<MPMoviePlayerControllerHostObject>(this)
        .initial_playback_time = time;
}

- (f64)endPlaybackTime {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .end_playback_time
}
- (())setEndPlaybackTime:(f64)time {
    let playing = {
        let host = env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        host.end_playback_time = time;
        host.playback_state == MPMoviePlaybackStatePlaying
    };
    // If the movie is already playing, move the pending end-of-playback to
    // the new end time (or back to the natural end for a negative value).
    if playing {
        let start_at = Instant::now();
        cancel_pending_playback_finish(env, this);
        schedule_playback_finish(env, this, start_at, true);
    }
}

- (f64)duration {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .duration
}
- (f64)playableDuration {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .duration
}
- (bool)isPreparedToPlay {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .load_state
        & (MPMovieLoadStatePlayable | MPMovieLoadStatePlaythroughOK)
        != 0
}
- (bool)readyForDisplay {
    env.objc
        .borrow::<MPMoviePlayerControllerHostObject>(this)
        .ready_for_display
}

- (())prepareToPlay {
    // Per Apple docs `prepareToPlay` is the asynchronous load entry point.
    // Make sure the preload sequence has been scheduled.
    schedule_preload_sequence(env, this);
}

// Apparently an undocumented, private API, but Spore Origins uses it.
- (())setMovieControlMode:(NSInteger)_mode {
    // As this is undocumented and we don't have real video playback yet, let's
    // ignore it.
}

// Another undocumented one! But some apps may still use it :/
// https://stackoverflow.com/a/1390079/2241008
- (())setOrientation:(UIDeviceOrientation)_orientation animated:(bool)_animated {
}

// MPMediaPlayback implementation
- (())play {
    let already_playing = {
        let host = env
            .objc
            .borrow::<MPMoviePlayerControllerHostObject>(this);
        host.playback_state == MPMoviePlaybackStatePlaying
    };

    // If preload hasn't been scheduled yet (e.g. content was provided via
    // `setContentURL:` without ever calling `prepareToPlay`), run the
    // full sequence now.
    schedule_preload_sequence(env, this);

    if !already_playing {
        let when = Instant::now() + Duration::from_millis(20);
        schedule_playback(env, this, when);
    }
}

- (())pause {
    cancel_pending_playback_finish(env, this);
    {
        let host = env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        host.clock_offset = playback_position(host);
        host.clock_started = None;
        host.playback_state = MPMoviePlaybackStatePaused;
    }
    enqueue(env, PendingNotification::PlaybackStateChange(this), Instant::now());
}

- (())stop {
    cancel_pending_playback_finish(env, this);
    {
        let host = env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this);
        host.clock_offset = 0.0;
        host.clock_started = None;
        host.playback_state = MPMoviePlaybackStateStopped;
        host.playback_rate = 1.0;
    }
    State::get(env).videos.remove(&this);
    let use_movie_player_fix = fix_movie_player_enabled(env);
    if use_movie_player_fix {
        clear_movie_frame(env, this);
        set_movie_view_hidden(env, this, true);
    }
    enqueue(env, PendingNotification::PlaybackStateChange(this), Instant::now());
    if !use_movie_player_fix {
        dismiss_unparented_movie_view(env, this);
    }
    if env
        .framework_state
        .media_player
        .movie_player
        .active_player == Some(this)
    {
        env.framework_state.media_player.movie_player.active_player = None;
        release(env, this);
    }
}

@end

@implementation MPMoviePlayerViewController: UIViewController

- (id)initWithContentURL:(id)url {
    log_dbg!(
        "[(MPMoviePlayerViewController*){:?} initWithContentURL:{:?} ({:?})]",
        this,
        url,
        ns_url::to_rust_path(env, url),
    );
    // Call designated initializer of UIViewController superclass.
    let this: id = msg![env; this init];

    // Per Apple docs, MPMoviePlayerViewController creates and manages its
    // own MPMoviePlayerController. Create one and store it so the
    // `moviePlayer` property can return it.
    // https://developer.apple.com/documentation/mediaplayer/mpmovieplayerviewcontroller
    let player: id = msg_class![env; MPMoviePlayerController alloc];
    let player: id = msg![env; player initWithContentURL:url];
    // The +1 reference from alloc/init is owned by the view controller and
    // given up in -dealloc. This deliberately isn't stored through KVC: an
    // app subclass has no setter for a private key, so KVC would fall back
    // to its never-freed dynamic storage (and warn about it).
    if this != nil {
        let old = State::get(env).view_controller_players.insert(this, player);
        if let Some(old) = old {
            release(env, old);
        }
    } else {
        release(env, player);
    }

    this
}

- (())dealloc {
    if let Some(player) = State::get(env).view_controller_players.remove(&this) {
        release(env, player);
    }
    msg_super![env; this dealloc]
}

// Apple docs: "The movie player controller object used to present the movie."
// @property(nonatomic, readonly) MPMoviePlayerController *moviePlayer
// https://developer.apple.com/documentation/mediaplayer/mpmovieplayerviewcontroller/1619165-movieplayer
- (id)moviePlayer {
    State::get(env)
        .view_controller_players
        .get(&this)
        .copied()
        .unwrap_or(nil)
}

- (())viewDidLoad {
    let parent_view: id = msg![env; this view];
    let movie_player: id = msg![env; this moviePlayer];
    if parent_view == nil || movie_player == nil {
        return;
    }

    let movie_view: id = msg![env; movie_player view];
    if movie_view == nil {
        return;
    }
    let bounds: CGRect = msg![env; parent_view bounds];
    let resize_mask: NSUInteger = (1 << 1) | (1 << 4);
    () = msg![env; movie_view setFrame:bounds];
    () = msg![env; movie_view setAutoresizingMask:resize_mask];
    () = msg![env; parent_view addSubview:movie_view];
}

@end

};

/// For use by `NSRunLoop` via [super::handle_players]: check movie players'
/// status, send notifications if necessary.
/// Start decoding the player's movie for display, if it is playing and has
/// no decoder yet.
fn stop_movie_audio(env: &mut Environment, player: id) {
    let audio_player = {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(player);
        let audio_player = host.audio_player;
        host.audio_player = nil;
        audio_player
    };
    if audio_player != nil {
        () = msg![env; audio_player stop];
        release(env, audio_player);
    }
}

fn sync_movie_audio(env: &mut Environment, player: id) {
    let (playback_state, mut audio_player, url, looping, playback_time) = {
        let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
        (
            host.playback_state,
            host.audio_player,
            host.content_url,
            host.repeat_mode == MPMovieRepeatModeOne,
            playback_position(host),
        )
    };

    match playback_state {
        MPMoviePlaybackStatePaused => {
            if audio_player != nil {
                () = msg![env; audio_player pause];
            }
        }
        MPMoviePlaybackStateStopped => stop_movie_audio(env, player),
        MPMoviePlaybackStatePlaying => {
            if audio_player == nil && url != nil {
                let path = ns_url::to_rust_path(env, url);
                let Ok(bytes) = env.fs.read(&path) else {
                    return;
                };
                let Ok(length) = NSUInteger::try_from(bytes.len()) else {
                    log!("MPMoviePlayerController: embedded audio file is too large to load");
                    return;
                };
                let data_buffer = env.mem.alloc(length);
                env.mem
                    .bytes_at_mut(data_buffer.cast(), length as crate::mem::GuestUSize)
                    .copy_from_slice(&bytes);
                let bytes_ptr = data_buffer.cast_const().cast_void();
                let data: id = msg_class![env; NSData dataWithBytes:bytes_ptr length:length];
                env.mem.free(data_buffer.cast());
                if data == nil {
                    return;
                }
                audio_player = msg_class![env; AVAudioPlayer alloc];
                audio_player =
                    msg![env; audio_player initWithData:data error:(MutPtr::<id>::null())];
                if audio_player == nil {
                    log_dbg!("MPMoviePlayerController: movie has no decodable audio track");
                    return;
                }
                env.objc
                    .borrow_mut::<MPMoviePlayerControllerHostObject>(player)
                    .audio_player = audio_player;
            }

            if audio_player != nil {
                let loops: NSInteger = if looping { -1 } else { 0 };
                () = msg![env; audio_player setNumberOfLoops:loops];
                () = msg![env; audio_player prepareToPlay];
                () = msg![env; audio_player setCurrentTime:playback_time];
                let started: bool = msg![env; audio_player play];
                if !started {
                    log_dbg!("MPMoviePlayerController: embedded audio output could not start");
                }
            }
        }
        _ => {}
    }
}

fn start_video_if_playing(env: &mut Environment, player: id) {
    let (playing, looping, supported, url) = {
        let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
        (
            host.playback_state == MPMoviePlaybackStatePlaying,
            host.repeat_mode == MPMovieRepeatModeOne,
            host.video_info.is_some(),
            host.content_url,
        )
    };
    if !playing || !supported || url == nil || State::get(env).videos.contains_key(&player) {
        return;
    }
    let path = ns_url::to_rust_path(env, url);
    let Ok(bytes) = env.fs.read(&path) else {
        return;
    };
    if let Some(video) = MovieVideo::start(&bytes, looping) {
        State::get(env).videos.insert(player, video);
    }
}

fn set_movie_fullscreen(env: &mut Environment, player: id, fullscreen: bool) {
    let (was_fullscreen, mut view) = {
        let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
        (host.fullscreen, host.view)
    };
    if fullscreen && view == nil {
        view = ensure_view(env, player);
    }
    if was_fullscreen == fullscreen {
        if fullscreen && view != nil {
            update_movie_view_frame(env, player, view);
        }
        return;
    }

    let original_frame = if fullscreen && view != nil {
        let parent: id = msg![env; view superview];
        if parent != nil {
            Some(msg![env; view frame])
        } else {
            None
        }
    } else {
        None
    };
    let frame_to_restore = {
        let host = env
            .objc
            .borrow_mut::<MPMoviePlayerControllerHostObject>(player);
        host.fullscreen = fullscreen;
        if fullscreen {
            host.fullscreen_original_frame = original_frame;
            None
        } else {
            host.fullscreen_original_frame.take()
        }
    };

    if fullscreen {
        if view != nil {
            update_movie_view_frame(env, player, view);
        }
    } else if view != nil {
        if let Some(frame) = frame_to_restore {
            () = msg![env; view setFrame:frame];
        }
    }
}

fn set_view_frame_if_needed(env: &mut Environment, view: id, frame: CGRect) {
    let old_frame: CGRect = msg![env; view frame];
    if (old_frame.origin.x - frame.origin.x).abs() > 0.01
        || (old_frame.origin.y - frame.origin.y).abs() > 0.01
        || (old_frame.size.width - frame.size.width).abs() > 0.01
        || (old_frame.size.height - frame.size.height).abs() > 0.01
    {
        () = msg![env; view setFrame:frame];
    }
}

fn update_movie_view_frame(env: &mut Environment, player: id, view: id) {
    let use_movie_player_fix = fix_movie_player_enabled(env);
    let (video_info, scaling_mode, fullscreen, has_original_frame) = {
        let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
        (
            host.video_info,
            host.scaling_mode,
            if use_movie_player_fix {
                host.fullscreen
            } else {
                host.presentation_view_controller != nil
            },
            use_movie_player_fix && host.fullscreen_original_frame.is_some(),
        )
    };
    if !use_movie_player_fix && video_info.is_none() {
        return;
    }
    let parent: id = msg![env; view superview];
    if parent == nil {
        return;
    }
    let bounds: CGRect = msg![env; parent bounds];
    if fullscreen {
        if use_movie_player_fix && !has_original_frame {
            let original_frame: CGRect = msg![env; view frame];
            env.objc
                .borrow_mut::<MPMoviePlayerControllerHostObject>(player)
                .fullscreen_original_frame = Some(original_frame);
        }
        set_view_frame_if_needed(env, view, bounds);
        return;
    }
    let Some(video_info) = video_info else {
        return;
    };
    let video_width = video_info.width as f32;
    let video_height = video_info.height as f32;
    let bounds_width = bounds.size.width;
    let bounds_height = bounds.size.height;
    if video_width <= 0.0 || video_height <= 0.0 || bounds_width <= 0.0 || bounds_height <= 0.0 {
        return;
    }

    let scale_x = bounds_width / video_width;
    let scale_y = bounds_height / video_height;
    let scale = match scaling_mode {
        MPMovieScalingModeNone => scale_x.min(scale_y).min(1.0),
        MPMovieScalingModeAspectFit => scale_x.min(scale_y),
        MPMovieScalingModeAspectFill => scale_x.max(scale_y),
        MPMovieScalingModeFill => {
            let frame = bounds;
            set_view_frame_if_needed(env, view, frame);
            () = msg![env; parent setClipsToBounds:false];
            return;
        }
        _ => scale_x.min(scale_y),
    };
    let width = video_width * scale;
    let height = video_height * scale;
    let frame = CGRect {
        origin: CGPoint {
            x: bounds.origin.x + (bounds_width - width) * 0.5,
            y: bounds.origin.y + (bounds_height - height) * 0.5,
        },
        size: CGSize { width, height },
    };
    set_view_frame_if_needed(env, view, frame);
    let clips_to_bounds = scaling_mode == MPMovieScalingModeAspectFill;
    () = msg![env; parent setClipsToBounds:clips_to_bounds];
}

/// Show the latest decoded frame of each playing movie in its player's view.
fn present_video_frames(env: &mut Environment) {
    let use_movie_player_fix = fix_movie_player_enabled(env);
    let players: Vec<id> = State::get(env).videos.keys().copied().collect();
    let mut frames = Vec::new();
    for player in players {
        let playback_time = {
            let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
            playback_position(host)
        };
        let frame = State::get(env)
            .videos
            .get_mut(&player)
            .and_then(|video| video.take_frame(playback_time));
        let Some(frame) = frame else {
            continue;
        };
        frames.push((player, frame));
    }

    for (player, frame) in frames {
        let view = env
            .objc
            .borrow::<MPMoviePlayerControllerHostObject>(player)
            .view;
        if view == nil {
            continue;
        }
        if !use_movie_player_fix {
            present_unparented_movie_view(env, player, view);
            let superview: id = msg![env; view superview];
            if superview == nil {
                continue;
            }
        }
        update_movie_view_frame(env, player, view);
        let layer: id = msg![env; view layer];
        let pts = frame.pts;
        crate::frameworks::core_animation::ca_eagl_layer::present_pixels(
            env,
            layer,
            frame.rgba,
            frame.width,
            frame.height,
        );
        let became_ready = {
            let host = env
                .objc
                .borrow_mut::<MPMoviePlayerControllerHostObject>(player);
            if host.ready_for_display {
                false
            } else {
                host.ready_for_display = true;
                true
            }
        };
        if use_movie_player_fix {
            set_movie_view_hidden(env, player, false);
        }
        if became_ready {
            enqueue(
                env,
                PendingNotification::ReadyForDisplayChange(player),
                Instant::now(),
            );
            if !use_movie_player_fix {
                log!(
                    "MPMoviePlayerController {:?}: first video frame presented at {:.3}s",
                    player,
                    pts
                );
            }
        }
    }
}

pub(super) fn handle_players(env: &mut Environment) {
    present_video_frames(env);

    // Pop all notifications whose time has come, preserving FIFO order.
    let mut ready: Vec<PendingNotification> = Vec::new();
    {
        let pending = &mut State::get(env).pending_notifications;
        let now = Instant::now();
        while let Some(&(notif, when)) = pending.front() {
            if when <= now {
                ready.push(notif);
                pending.pop_front();
            } else {
                break;
            }
        }
    }

    for notif in ready {
        let player = notif.player();
        let name_str = notif.name();

        // A looping movie (`MPMovieRepeatModeOne`) never reaches its end, so
        // Apple's player never posts `PlaybackDidFinish` for it on its own;
        // it keeps playing until the app calls `stop`. Apps that play a
        // looping background movie behind their UI (e.g. BioShock's main
        // menu) treat an early "playback ended" as the movie being dismissed
        // and tear down that screen. The repeat mode is usually set after
        // `initWithContentURL:` has already queued the finish, so check it
        // here, at dispatch time.
        if let PendingNotification::PlaybackDidFinish {
            reason: MPMovieFinishReasonPlaybackEnded,
            ..
        } = notif
        {
            let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
            if host.repeat_mode == MPMovieRepeatModeOne
                && host.playback_state == MPMoviePlaybackStatePlaying
            {
                log_dbg!(
                    "MPMoviePlayerController {:?}: looping movie, not posting PlaybackDidFinish",
                    player
                );
                env.objc
                    .borrow_mut::<MPMoviePlayerControllerHostObject>(player)
                    .finish_scheduled = false;
                release(env, player);
                continue;
            }
        }

        // For PlaybackDidFinish we must update playbackState BEFORE posting,
        // so observers that read [player playbackState] see Stopped.
        if matches!(notif, PendingNotification::PlaybackDidFinish { .. }) {
            let use_movie_player_fix = fix_movie_player_enabled(env);
            let host = env
                .objc
                .borrow_mut::<MPMoviePlayerControllerHostObject>(player);
            host.playback_state = MPMoviePlaybackStateStopped;
            host.clock_offset = 0.0;
            host.clock_started = None;
            host.playback_rate = 1.0;
            host.finish_scheduled = false;
            State::get(env).videos.remove(&player);
            if use_movie_player_fix {
                clear_movie_frame(env, player);
                set_movie_view_hidden(env, player, true);
            }
            stop_movie_audio(env, player);
            if !use_movie_player_fix {
                dismiss_unparented_movie_view(env, player);
            }
        }

        if matches!(notif, PendingNotification::PlaybackStateChange(_)) {
            start_video_if_playing(env, player);
            sync_movie_audio(env, player);
        }

        let name = ns_string::get_static_str(env, name_str);
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];

        if let PendingNotification::PlaybackDidFinish { reason, .. } = notif {
            // userInfo[MPMoviePlayerPlaybackDidFinishReasonUserInfoKey] = reason
            let reason_num: id = msg_class![env; NSNumber numberWithInt:(reason)];
            let reason_key =
                ns_string::get_static_str(env, MPMoviePlayerPlaybackDidFinishReasonUserInfoKey);
            let user_info: id = msg_class![env; NSDictionary
                dictionaryWithObject:reason_num
                forKey:reason_key];
            let _: () = msg![env; center postNotificationName:name
                                                       object:player
                                                     userInfo:user_info];
        } else {
            let _: () = msg![env; center postNotificationName:name
                                                       object:player];
        }

        // Release the retain we took when queuing this notification.
        release(env, player);
    }
}
