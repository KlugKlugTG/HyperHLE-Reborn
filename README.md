# HyperHLE-Reborn

<p align="center"><img src="./Picsart_26-10-20_10-43-04-335.jpg" alt="Senses" width="700"></p>
**HyperHLE-Reborn** is a community-maintained fork of [HyperHLE](https://github.com/HyperHLE/HyperHLE), itself based on [touchHLE](https://github.com/touchHLE/touchHLE). The user-facing name of this project is **HyperHLE**.

HyperHLE is a high-level emulator for early iPhone OS apps. It reimplements selected iOS frameworks on the host, allowing compatible apps to run without booting iOS. This fork focuses on Android/mobile usability, fullscreen and orientation handling, graphics compatibility, and app-specific fixes. It is not an official Apple product.

The repository does not distribute commercial apps or iOS firmware. Put an app bundle (`.app`) or IPA (`.ipa`) that you are entitled to use in `touchHLE_apps/`; it will appear in the app picker.

## Platforms and features

- Builds for Android, iOS, Windows, macOS, and Linux. The iOS app comes from [Applesauce](https://github.com/johnny901901901/Applesauce); see [`platform/ios`](platform/ios/README.md).
- Virtual iPhone and iPad device profiles.
- Android touch input and fullscreen/orientation handling.
- SharpEmu-style performance display: full counters with a 128-frame graph, minimal panel, or title-bar summary. **F1** toggles it; **F2** cycles panel corners. Mobile builds expose Full/Minimal modes in the app picker; `titlebar` falls back to Minimal where the OS has no title bar.
- The GLES backend reports `Q DIRECT` and tracked `GLALLOC` storage instead of inventing Vulkan queue/allocation counts. Android GPU load uses readable vendor counters; kernels that hide them can still show `GPU N/A`. `GC 0/0/0` is expected because HyperHLE is written in Rust and has no garbage collector.
- Game Center is emulated locally: with `--game-center-authenticated` (or "Local Game Center sign-in" in Settings) games can sign in, report achievements and leaderboard scores, and read them back. Progress is stored in `touchHLE_gamecenter.txt` and the app picker's **Game Center** tile (in the dock) shows every recorded achievement with a progress bar and every best score, per game. Signed out (the default), reporting fails like on a device without sign-in, so nothing is recorded. Nothing is sent to Apple.
- Ongoing GLES, EAGL, Core Animation, audio, and iOS framework compatibility work.
- Download build artifacts from the [HyperHLE GitHub Actions workflow](https://github.com/KlugKlugTG/HyperHLE-Reborn/actions/workflows/HyperHLE_release.yml).

## Newly tested and working games

The titles below have been tested with HyperHLE-Reborn and are known to run. Compatibility may still vary with the app version, virtual device profile, host GPU/driver, and settings; this list does not guarantee that every feature or a full playthrough works. Also any update can break these games in list, report if it doesn't work

- N.O.V.A. 3 (Playable)
- Gangstar Vegas (Unplayable on adreno)
- Geometry Dash (Playable)
- Modern Combat 3 (Playable)
- Asphalt 7 (Playable)
- Turbo Dismount (Playable)
- Scarface (Playable)
- Zombie Safari (Playable)
- Real Racing 1 (Playable)
- Real Racing 3 (Playable)
- Silent Ops (Playable)
- Need For Speed: Most Wanted (Playable)
- Minecraft 0.16.2 (Playable)
- Oceanhorn (Playable)
- Asphalt 8 (Playable)
- GTA 3 (Ingame)
- Granny (Ingame)
- Talking Tom Hero Dash (Playable)
- My Talking Hank (Playable)
- Subway Surfers 1.106.0 (Playable)
- And more more games

## Build and documentation

Build and run a desktop version with Cargo:

```sh
cargo run --release -- path/to/app.app
```

For iOS build steps and the unsigned IPAs, see [`platform/ios`](platform/ios/README.md). For Android build steps, see the `android/` directory and the [CI workflow](https://github.com/KlugKlugTG/HyperHLE-Reborn/actions/workflows/HyperHLE_release.yml). Command-line options are documented in [`OPTIONS_HELP.txt`](OPTIONS_HELP.txt); notable compatibility changes are tracked in [`CHANGELOG.md`](CHANGELOG.md).

## Community

Join the [HyperHLE Discord server](https://discord.gg/ZpEkAV47H9) to discuss the project and contribute.

## Credits and license

HyperHLE-Reborn carries forward work from HyperHLE and touchHLE contributors and uses open-source libraries. It is licensed under the [Mozilla Public License 2.0](LICENSE); see the individual projects for their respective notices and licenses.
