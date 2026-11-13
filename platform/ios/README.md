# HyperHLE-Reborn-Fork-V2 for iOS

The iOS app is displayed as **HyperHLE-Reborn-Fork-V2** and runs the HyperHLE
core on modern iPhones and iPads. It plays supported 32-bit iPhone OS games
through high-level emulation and Dynarmic JIT; no games or Apple software are
included.

The iOS app, its build scripts and its packaging come from
[Applesauce](https://github.com/johnny901901901/Applesauce), an unaffiliated
iOS port of touchHLE and HyperHLE. This directory is that host app, rebranded
and built against this repository's core. Applesauce is not connected to this
fork, so report problems here rather than there.

The home-screen label is **HyperHLE-Reborn-Fork-V2**. The `.app` bundle path and
release filenames retain their historical `HyperHLE` names for build tooling.

## Downloads

Every run of the [HyperHLE CI workflow](https://github.com/KlugKlugTG/HyperHLE-Reborn/actions/workflows/HyperHLE_release.yml)
uploads the three IPAs below as separate artifacts, and
[releases](https://github.com/KlugKlugTG/HyperHLE-Reborn/releases) attach them
too:

| Your iOS | IPA | How JIT gets enabled |
| --- | --- | --- |
| 17.4 or newer | `HyperHLE-iOS-unsigned.ipa` | Sideload (AltStore, SideStore, Sideloadly, Xcode), then StikDebug's bolt button in the app |
| 15.0–17.0, TrollStore installed | `HyperHLE-iOS-trollstore.ipa` | TrollStore's **Enable JIT**, before each session |
| 15.0–17.0, TrollStore, **A11 or older** | `HyperHLE-iOS-trollstore-permanent-jit.ipa` | Already on, and stays on |
| 17.0.1–17.3 | — | Only AltJIT with a computer attached |

The app's version follows the repository's release tags: a release build
carries that release's version (e.g. `1.1.0`), and other builds carry the latest
tag's. The build number is the commit count.

- The plain IPA is fully unsigned. Your install tool signs it with your own
  Apple account.
- The TrollStore IPAs are fakesigned with `ldid` so they can carry
  `get-task-allow` and the extended-virtual-addressing and
  increased-memory-limit entitlements. A free Apple account cannot sign those.
- **The permanent-JIT build crashes on launch on A12 and newer** (iPhone XS/XR
  onwards), because iOS bans `dynamic-codesigning` there. Use the ordinary
  TrollStore build on those devices.

## JIT

Dynarmic needs executable memory, so JIT has to be enabled whenever the app
starts as a new process:

- **StikDebug (iOS 17.4+):** pair StikDebug with your device, connect its
  loopback VPN, then tap the bolt button in the HyperHLE-Reborn-Fork-V2 library.
- **TrollStore (iOS 15.0–17.0):** use TrollStore's **Enable JIT** on HyperHLE-Reborn-Fork-V2
  before starting a game.
- **AltJIT:** in AltStore, long-press HyperHLE-Reborn-Fork-V2 under **My Apps** and choose
  **Enable JIT**.

## Importing games

Tap **Import Game** and pick a decrypted 32-bit `.ipa` from Files. The app reads
its name and icon and adds it to the library. Saves and imported games live in
the app's Documents folder, which the Files app shows under **On My iPhone**.

Per-game options from `touchHLE_default_options.txt` apply as on other
platforms. On iOS, `--ios-es2-direct-present` additionally presents an OpenGL
ES 2.0 drawable directly even when it is not the fullscreen layer (The Sims
Medieval needs it).

## Building

Requirements: macOS with a recent Xcode, Rust via rustup, CMake, Ninja,
Python 3, Boost headers in `vendor/boost` (or `TOUCHHLE_BOOST_ROOT`), and
`ldid` for the TrollStore IPAs.

```sh
git submodule update --init
brew install cmake ninja ldid
rustup target add aarch64-apple-ios

sh platform/ios/scripts/build-sdl-shared.sh iphoneos
sh platform/ios/scripts/build-host.sh iphoneos Release

sh platform/ios/scripts/package-ipa.sh
sh platform/ios/scripts/package-ipa.sh --trollstore
sh platform/ios/scripts/package-ipa.sh --trollstore-permanent-jit
```

The IPAs land in `dist/`. Check them with `platform/ios/scripts/verify-ipa.sh`
(pass `--trollstore` or `--trollstore-permanent-jit` for those builds).

How the pieces fit together:

- The core is built from `platform/ios/rust-entry` as `libhyperhle_core.dylib`
  with the `dynamic-sdl` feature, and the app loads it with `dlopen`.
- SDL2 is built as a shared library both the app and the core link against.
  SDL's UIKit backend needs a few changes for this host
  (`patches/sdl-ios-host.patch`), applied to a copy of `vendor/SDL` in
  `build/sdl-ios-src`.
- Dynarmic needs the iOS JIT changes in `patches/dynarmic-ios-jit.patch`
  (MAP_JIT memory with separate writable and executable views). They are
  applied to a copy of `vendor/dynarmic` in `build/dynarmic-ios-src`, which the
  build points at through `TOUCHHLE_DYNARMIC_DIR`. The submodules themselves
  are never modified.

To run on your own device from Xcode, run `build-sdl-shared.sh iphoneos` and
`build-rust.sh aarch64-apple-ios Debug`, open `TouchHLEHost.xcodeproj`, choose
your team under **Signing & Capabilities** (and change the bundle identifier in
`Config/Build.xcconfig` if needed), then press **Run**.

The host can also ship upstream touchHLE as a second core. Point
`TOUCHHLE_CORE_REPO` at a touchHLE checkout that has an iOS entry crate (such as
the `ios-core-dylib` branch of johnny901901901/touchHLE) when running
`build-host.sh`, and the in-app core picker appears.

## License

MPL-2.0, like the rest of HyperHLE, subject to the third-party licences of the
bundled components.
