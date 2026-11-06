/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Stub for `VideoToolbox.framework/VideoToolbox`.
//!
//! On iOS, VideoToolbox provides hardware H.264 decoding
//! (`VTDecompressionSessionCreate` and friends) and the `VTFrameSilo`
//! helpers for image sequences. Apps that link it usually use it for video
//! playback or for encoding captured frames; neither works in this emulator,
//! which decodes H.264 in software through the bundled OpenH264 decoder.
//!
//! Registering the dylib with no exports suppresses the
//! `app binary depends on unimplemented or missing dylib` warning at startup.
//! Weakly-linked symbol lookups then return `NULL`, which is what a guest
//! checking for video support expects, and any direct call into the framework
//! is reported once by the unimplemented-function path.

use crate::dyld::HostDylib;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/VideoToolbox.framework/VideoToolbox",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[],
};