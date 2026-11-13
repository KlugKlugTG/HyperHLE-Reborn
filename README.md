# HyperHLE-Reborn-Fork-V2

<p align="center"><img src="./Picsart_26-10-20_10-43-04-335.jpg" alt="The official portrait of an unofficially named emulator" width="700"></p>

> **A high-level emulator. A low-level naming incident. Several extremely old pixels.**
>
> At an unspecified hour, a 32-bit iPhone app arrived at the Department of Frameworks with a form marked “Please run me on a computer that did not exist when I was young.” HyperHLE-Reborn-Fork-V2 accepted the form, misplaced the carbon copy, and began reimplementing selected iOS frameworks. This is the resulting situation.

## Executive summary, recovered from a filing cabinet

HyperHLE-Reborn-Fork-V2 is the user-facing name of this community-maintained fork of [HyperHLE](https://github.com/HyperHLE/HyperHLE), itself based on [touchHLE](https://github.com/touchHLE/touchHLE). It is a **high-level emulator for early iPhone OS apps**: it reimplements selected iOS frameworks on the host so compatible apps can run without booting iOS.

The name is not a versioning system, a prophecy, or a claim that the emulator has been forked exactly twice. The naming lineage is roughly:

```text
HyperHLE
  └── HyperHLE-Reborn
        └── HyperHLE-Reborn-Fork
              └── HyperHLE-Reborn-Fork-V2
                    └── [a committee is being formed]
```

Each generation adds one more hyphen and one less opportunity to fit the name beneath an app icon. The displayed app name is intentionally **HyperHLE-Reborn-Fork-V2**. Internal package IDs, executable names, folders, and some release filenames retain their historical HyperHLE/touchHLE names, because the build system is an ancient filing cabinet and we have agreed not to kick it.

## The machine, described by witnesses

- **Platforms:** Android, iOS, Windows, macOS, and Linux. The iOS host app comes from [Applesauce](https://github.com/johnny901901901/Applesauce); see [`platform/ios`](platform/ios/README.md). The name Applesauce is unrelated to the emulator's fruit intake.
- **Device impersonation:** virtual iPhone and iPad profiles. No tiny person is placed inside the device.
- **Android input:** touch, fullscreen, and orientation handling. The screen is not asked to identify north; it has refused before.
- **Performance display:** full counters with a 128-frame graph, a minimal panel, or a title-bar summary. **F1** toggles it; **F2** cycles panel corners. Mobile builds offer Full and Minimal modes in the app picker. `titlebar` falls back to Minimal where there is no title bar. The corners are purely decorative and have not been appointed to public office.
- **Graphics accounting:** the GLES backend reports `Q DIRECT` and tracked `GLALLOC` storage instead of inventing Vulkan queue or allocation counts. Android GPU load uses readable vendor counters; some kernels hide them, so `GPU N/A` is possible. `GC 0/0/0` is expected because the emulator is written in Rust and has no garbage collector. These are counters, not a cry for help.
- **Ongoing work:** GLES, EAGL, Core Animation, audio, and iOS framework compatibility. The TODO list is sentient only in the metaphorical sense, and has not yet requested a salary.
- **Builds:** find artifacts in the [HyperHLE GitHub Actions workflow](https://github.com/KlugKlugTG/HyperHLE-Reborn/actions/workflows/HyperHLE_release.yml). The workflow's old name is grandfathered in; it has seniority.

## Registry of apps observed near the time portal

These titles have been tested or reported to run in HyperHLE-Reborn. Compatibility can vary with app version, virtual device profile, host GPU/driver, and settings. This table is a field notebook, not a warranty, certification, speedrun record, or notarized treaty with the year 2010. **Playable** does not mean every feature works on every device or that a full playthrough has been completed. If an update breaks something, please report it; the emulator receives no mail from the past.

| App | Status in the notebook |
| --- | --- |
| N.O.V.A. 3 | Playable |
| Gangstar Vegas | In-game |
| Geometry Dash | Playable |
| Modern Combat 3 | Playable |
| Asphalt 7 | Playable |
| Turbo Dismount | Playable |
| Scarface | Playable |
| Zombie Safari | Playable |
| Real Racing 1 | Playable |
| Real Racing 3 | Playable |
| Silent Ops | Playable |
| Need for Speed: Most Wanted | Playable |
| Minecraft 0.16.2 | Playable |
| Oceanhorn | Playable |
| Asphalt 8 (versions 1.0.0–1.4.0) | Playable |
| GTA 3 | In-game |
| Granny | In-game |
| BioShock | In-game; not independently tested |
| Talking Tom Hero Dash | Playable |
| My Talking Hank | Playable |
| Subway Surfers 1.118.0 | Playable |
| Minion Rush 6.3.0 | Playable |

Additional sightings exist. Their paperwork is currently being carried down a very long corridor by a small wheeled cart.

## Instructions for operating the apparatus

Build and run a desktop version with Cargo:

```sh
cargo run --release -- path/to/app.app
```

The path is a real path to an app bundle, not a metaphorical path to personal growth. For iOS build steps and unsigned IPAs, see [`platform/ios`](platform/ios/README.md). For Android build steps, see `android/` and the [CI workflow](https://github.com/KlugKlugTG/HyperHLE-Reborn/actions/workflows/HyperHLE_release.yml). Command-line options are documented in [`OPTIONS_HELP.txt`](OPTIONS_HELP.txt); compatibility changes are tracked in [`CHANGELOG.md`](CHANGELOG.md).

## Legal department (one folding chair)

This is not an official Apple product and does not boot iOS. The repository does not include commercial apps or iOS firmware. Supply an app bundle (`.app`) or IPA (`.ipa`) that you are entitled to use by placing it in `touchHLE_apps/`; it will appear in the app picker. The folder is not a portal, despite several suspicious noises from inside it.

## Incident reporting

Join the [HyperHLE Discord server](https://discord.gg/ZpEkAV47H9) to discuss the project, report bugs, or contribute. A useful report includes the game version, device/OS, graphics backend, and a log. “It broke” is an acceptable opening statement; it is not, by itself, enough evidence for the Court of Pixels to issue a ruling.

## Credits, debts, and license

Thanks to everyone who got BioShock this far in HyperHLE:

- **Tog991 (goodvids56)**, developer of [HyperHLE](https://github.com/HyperHLE/HyperHLE), fixed the game's launch itself.
- **uxazu**, creator of Aemulator, fixed the game up to the menu and half-implemented the video player.
- The [Podroid](https://github.com/shawnw67/Podroid) fork got it through its level loads and in-game.

HyperHLE-Reborn-Fork-V2 carries forward work from HyperHLE and touchHLE contributors and uses open-source libraries. It is licensed under the [Mozilla Public License 2.0](LICENSE); see the individual projects for their respective notices and licenses. The V2 suffix does not change the license, upstream, laws of physics, or the number of sandwiches in the build room.

## Frequently misplaced questions

**Does V2 mean this is version 2.0?** No. It is part of the display name. The actual project version is determined by the build metadata, which is less dramatic.

**Is this a time machine?** No. It emulates selected frameworks. Time remains one-way, except when debugging makes five minutes last an hour.

**Why do some files still say touchHLE or HyperHLE?** History, compatibility, build scripts, and the principle that one should not rename every drawer while the filing cabinet is running.

**Can I put any app in it?** Only apps you are entitled to use. The emulator has no legal department beyond the folding chair mentioned above.
