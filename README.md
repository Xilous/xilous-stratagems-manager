# Xilous Stratagems Manager

Xilous Stratagems Manager is a Windows utility for Helldivers 2 that

- **saves and applies loadout presets** on the loadout screen (four Stratagems and an optional Booster, four preset slots), and
- **calls in Stratagems during missions** with hotkeys: `F1`–`F4` enter the input code of the matching slot of the loadout you last saved or applied, and any mission Stratagem (Reinforce, Resupply, SOS Beacon, Hellbomb, …) can get its own key.

Everything is managed from a window: what the tool thinks is equipped, the saved presets, the hotkeys, and the input timing.

The program recognizes the game interface through screen capture and completes selections using standard mouse and keyboard input. It does not modify game files, inject code into the game process, read game memory, or alter network traffic. Calling in a Stratagem sends the same key presses you would type yourself; the game is not observed while doing so.

## Download and install

1. Download `XilousStratagemsManager-Setup-<version>.exe` from the [latest release](../../releases/latest) and run it. It installs for the current user (no administrator rights), adds a Start Menu entry, and offers a desktop shortcut and start-with-Windows.
2. Windows SmartScreen may warn because the download is not code-signed; choose *More info* → *Run anyway*. Every release is built by GitHub Actions from the tagged source and carries a build attestation you can check with `gh attestation verify <file> --owner Xilous`.

To update, run the newer installer over the existing installation. To uninstall, use *Apps & features*; your presets and settings are kept unless you choose to remove them.

Prefer no installer? The release also has the portable `XilousStratagemsManager.exe`: put it in a folder of its own and run it. It keeps its settings, presets, and logs in a `data` folder next to itself.

The application shows a window and a tray icon. Closing the window minimizes it; use **Exit** in the window or the tray menu to stop the tool.

To uninstall, exit the application and delete the extracted folder. This also removes its configuration, presets, and logs.

## Game settings

In Helldivers 2, set **Stratagem input** to the **arrow keys** (Options → Mouse & Keyboard). With the default movement-key input, holding `W` while a code is entered would be read as an extra ↑ and corrupt the code. The tool defaults to arrow keys and `Left Ctrl` as the Stratagem menu key; change them in the window if your keybinds differ.

## Quick start

### Presets (loadout screen)

The default preset keys are `Shift+F1` through `Shift+F4`, mapped to preset slots 1–4.

1. Open the loadout home screen and select all four Stratagems and, optionally, a Booster.
2. Press `Shift+F1`–`F4` and release all keys. The tool saves the loadout to that preset slot, identifies which Stratagem is in which slot, and makes it the **active loadout**.
3. To apply a preset later, return to the loadout home screen with all four Stratagem slots empty and press the same key combination. The applied loadout becomes the active loadout, in the order the game shows it.

| Loadout state | Result |
| --- | --- |
| All four Stratagem slots filled | Save or overwrite the preset |
| All four Stratagem slots empty | Apply the preset |
| Partially filled | Reject the action without changing the loadout |

If a slot could not be identified with confidence, the window says so; pick the right Stratagem from the dropdown once and it is remembered.

### Missions

While Helldivers 2 is focused and an active loadout exists, `F1`–`F4` enter the input code of slots 1–4. The tool holds the Stratagem menu key, taps the arrows, and releases; you throw the ball as usual. If no loadout is active, the keys do nothing and the window explains why.

Because sprinting holds `Shift`, pressing `F1` while sprinting sends `Shift+F1`, which is a preset key. The tool checks whether the loadout screen is visible and, if it is not, treats it as the slot call instead.

Mission Stratagems are universal, so they are not part of a loadout. Assign a key to each one you want in the **Mission stratagems** table of the window; keys can include `Ctrl`, `Shift`, `Alt`, or `Win`. Unassigned ones are inactive.

All hotkeys are only registered while Helldivers 2 is the foreground window, so they keep working normally in other applications.

### The window

- **Loadout presets** — the four presets with their identified Stratagems, labels, and buttons to set one as active or delete it.
- **Active loadout** — the four slots mapped to `F1`–`F4` with their input codes. Slots can be corrected by hand and the loadout can be cleared.
- **Mission stratagems** — key assignments for the universal Stratagems.
- **Settings** — preset options, slot keys, and the Stratagem input keys and timing. Changes are saved to `data/config.toml` immediately.
- **Activity log** — what the tool is doing and why something did not happen.

## Compatibility

Windows 10 version 1903 or later and Windows 11 are supported. Preset recognition has been tested from 720p to 2160p in Windowed, Borderless Window, and Fullscreen modes, including several non-16:9 resolutions and Windows HDR.

For Borderless Window or Fullscreen, match the in-game resolution to the Windows desktop resolution.

## Configuration

Keep the application in a writable folder. It creates and uses the following runtime files beside the executable:

- `data/config.toml` — hotkeys, mission input settings, labels, and options. Most of it is editable from the window.
- `data/presets.json` — saved preset metadata, including the identified Stratagem of each slot.
- `data/local_templates/` — icon samples captured with each preset.
- `data/active_loadout.json` — the active loadout.
- `data/app.log` — diagnostic log from the latest launch.

Preset hotkeys (`[hotkey]`) are read at startup; change them in the file and restart. Everything under `[mission]` can be changed from the window.

## Stratagem catalog

Input codes, names, and icons come from the [Stratagems page of the Helldivers wiki](https://helldivers.wiki.gg/wiki/Stratagems) and are embedded in the executable, so the tool never needs network access. When a warbond adds Stratagems, refresh the catalog and rebuild:

```powershell
powershell -ExecutionPolicy Bypass -File tools/sync-stratagems.ps1
cargo build --release
```

The script queries the wiki database, writes `data/stratagems.json` and the icons under `data/stratagems/`, and the diff shows exactly what changed.

## Troubleshooting

- **A preset key does nothing.** Check that the window shows *Helldivers 2: focused* and *Hotkeys: armed*, that the loadout home screen is open with all four Stratagem slots filled or empty, and that no other application uses the same shortcut.
- **`F1`–`F4` do nothing in a mission.** There must be an active loadout (see the window) and mission hotkeys must be enabled in Settings.
- **The code is entered wrong or partially.** Check the menu key and the four direction keys under *Game keybinds* in the window match the game, prefer arrow keys, then raise the hold and gap times.
- **A slot shows “not identified”.** Pick the Stratagem from the dropdown. If it happens often at your resolution, include `data/app.log` and `data/local_templates/` in a bug report.

For bug reports, include `data/app.log`, the resolution and display mode, Windows scaling and HDR status, and a screenshot of the affected screen.

## Building from source

Install a current stable Rust toolchain and the Visual Studio Build Tools with the "Desktop development with C++" workload, then run:

```powershell
cargo build --release
```

The executable is written to `target\release\XilousStratagemsManager.exe`.

To install the build as an application for the current user (Start Menu shortcut, data kept under the install folder), run:

```powershell
powershell -ExecutionPolicy Bypass -File tools/install.ps1 -Launch
```

Add `-Desktop` for a desktop shortcut and `-Startup` to start it at sign-in. Re-run after every rebuild; existing presets and settings are preserved. `-MigrateFrom target\release\data` carries data over from a copy that was run from the build folder.

## Legal

This is an unofficial third-party utility and is not affiliated with or endorsed by Arrowhead Game Studios or Sony Interactive Entertainment.

The source code is licensed under the [GNU General Public License version 3 or later](LICENSE).

Xilous Stratagems Manager is a modified fork of [HD2 Preset Helper](https://github.com/xmg228/hd2-preset-helper) by xmg228, used under the terms of the GPL. Changes have been made to the original work.
