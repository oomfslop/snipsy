# Snipsy

Screenshot editor and floating image pins for macOS 14+, in the spirit of Snipaste. Built with [GPUI Kit](https://gpui-kit.com).

Capture a frozen screen region or click a window, annotate it, then copy, save, pin, print, record, or search it with Google Lens. Pins float above other apps and survive relaunches.

## Build

Requires Rust and Xcode with its Metal toolchain (`xcodebuild -downloadComponent MetalToolchain`).

```sh
cargo build --release
python3 scripts/bundle_macos.py release
```

Open `target/release/Snipsy.app`. Grant it Screen Recording in System Settings → Privacy & Security; when run from a terminal, macOS attributes the permission to the terminal instead. The bundle script signs the app with a self-signed `snipsy-dev` identity, created in the login keychain on first run, so the grant survives rebuilds.

To install, run `scripts/install.sh` (pass `debug` for a debug build). It builds, bundles, quits the installed copy, replaces `/Applications/Snipsy.app`, and launches it. Enable Launch at login in Preferences → General.

Snipsy runs in the background with a menu-bar item. Settings (`config.toml`), history, and pin backups live in `~/Library/Application Support/Snipsy`. Debug builds use `Snipsy Debug` instead, so `cargo run` starts its own instance next to the installed one.

## Command line

```
snipsy [snip | record | paste [--plain TEXT... | --files FILE...] | history | clear-history
        | whiteboard [--color COLOR] | toggle-pins | toggle-click-through
        | preferences | quit]
```

Commands are forwarded to the running instance and start it when needed. `COLOR` accepts names, `#rgb[a]`, `#rrggbb[aa]`, and `r g b` (0–255 or 0.0–1.0); `--color transparent` makes a transparent whiteboard.

## Shortcuts

Global shortcuts and the capture toolbar layout are configurable in Preferences.

| Action | Shortcut |
| --- | --- |
| Capture or record a region | not set |
| Pin clipboard (image, files, color, or text) | not set |
| Toggle click-through for the pin under the pointer | not set |
| Show or hide pins | not set |

While capturing:

| Action | Shortcut |
| --- | --- |
| Select, rectangle, oval, arrow, line | `V` `R` `O` `A` `L` |
| Pen, highlighter, text, blur, eraser | `B` `H` `T` `M` `E` |
| Smart eraser: fill a rectangle with the color under the drag start | `F` |
| Recognize text and QR codes: drag to select, double-click for a word, click a highlighted link to open it | `X` |
| Copy recognized text (selection, all, or the codes when there is no text) / select all | `Enter` or `⌘C` / `⌘A` |
| Copy / save / quick-save / pin / print | `Enter` or `⌘C` / `⌘S` / `⌘⇧S` / `⌘T` / `⌘P` |
| Undo / redo | `⌘Z` / `⌘⇧Z` |
| Select the whole display | `⌘A`, or `Space` before selecting |
| Show or hide the toolbars | `Space` after selecting |
| Restore the last region | `R` before selecting |
| Previous / next capture | `,` / `.` |
| Toggle cursor capture | `` ` `` |
| Copy the magnified color (`⇧` for RGB) | `C` |
| Move the pointer before selecting | `W` `A` `S` `D` |
| Move / grow / shrink the selection | arrows / `⌘` arrows / `⇧` arrows |
| Move the selected annotation (`⇧` for 10 points) | arrows |
| Constrain to squares, circles, or 45° | hold `⇧` while drawing |
| Stroke width | `[` / `]`, or scroll the size label |
| Bold / italic / underline text | `⌘B` / `⌘I` / `⌘U` |
| New line / finish text | `⇧Enter` / `Enter` |
| Cancel | `Esc` or right-click |

On pins:

| Action | Shortcut |
| --- | --- |
| Move | drag, or arrows (`⇧` for 10 points) |
| Resize | scroll, `+` / `-`, or drag the bottom-right corner |
| Opacity | `⌘` scroll, `[` / `]`, or `⌘=` / `⌘-` |
| Rotate / flip | `1` `2` / `3` `4` |
| Reset size and opacity | `0` or middle-click |
| Thumbnail | `⇧` double-click |
| Annotate | `Space` |
| Copy image / source text | `⌘C` / `⌘⇧C` |
| Copy the color under the pointer (`⇧` for RGB) | hold `⌥`, press `C` |
| Replace from clipboard / drop a file (`⇧` opens separately) | `⌘V` |
| Save / quick-save / print | `⌘S` / `⌘⇧S` / `⌘P` |
| Close (reopen by pinning the clipboard until it changes) / destroy | `Esc`, `⌘W`, or double-click / `⇧Esc` |
| Animated GIFs: pause, step frames, speed | `P`, `1` `2`, `⌥` scroll |

Right-click a pin for the full menu.

Save and quick-save preserve animated GIF frames and timing. `P` enters frame selection at the first frame; use `1`/`2` to choose a frame to copy, save, print, or annotate. Selected frames save as PNG by default. Choose another image extension in Save to export a still image.

## Recording

Click record in the capture toolbar to pick the codec, frame rate, bitrate, and whether to record system audio or the microphone (macOS 15+), all remembered in `config.toml`, then press Record. Double-click the toolbar button to record with the last options. The region stays outlined on screen, and a stop button appears in the menu bar; pins and other windows are recorded.

When it stops, the recording opens in an editor. Drag the timeline handles to trim, annotate with the capture tools (resizing the selection crops), then export to MP4. Each annotation gets a bar under the timeline: drag its ends or body to show it only in that part of the video. Audio gets one track per source under the video: mute it from the track header or drag the clip up and down to change its volume. Export mixes the tracks into one. Unexported recordings are discarded when the editor closes.

| Action | Shortcut |
| --- | --- |
| Play / pause | `Space` |
| Export | `Enter` or `⌘S` |
| Discard | `Esc` or `⌘W` |

## Layout

- `editor.rs`, `geometry.rs`, `raster.rs`: annotation model, undo, hit testing, and CPU rendering for exports. No UI types.
- `capture/`: the capture overlay. `layout.rs` holds the placement math, `paint.rs` draws annotations on the GPU, `text.rs` handles text editing and IME, `recognition.rs` selects text and codes recognized by Vision, `toolbar.rs` renders the toolbars, `video.rs` is the recording editor.
- `pin/`: pin windows and their backups.
- `app/`: startup, command dispatch, CLI and single-instance IPC, hotkeys, tray, history, recording, and persistence.
- `ui/`: shared widgets and the History and Preferences windows.
- `platform/macos.rs`: ScreenCaptureKit capture and recording, Vision text and barcode recognition, AVFoundation playback and export, overlay windows, and native dialogs. This is the boundary to port for other platforms.
- `lens.rs`: Google Lens handoff.

```sh
cargo test
cargo clippy --all-targets
```
