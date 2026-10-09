# Snipsy

GPUI screenshot editor for macOS 14+. Capture a frozen screen region or click a window, annotate it, copy/save it, or pin it above other apps.

Tools: outlined/filled rectangles and ovals, line strips/arrows, pen, highlighter, rich text, pixelated/Gaussian blur, and annotation eraser. Secondary controls update the selected or latest annotation. Scroll the size control to adjust stroke width or text size; blur has an intensity slider. Color circles accompany the native color picker. Layers can be selected, moved, reordered, deleted, and undone/redone. Double-click text to edit; select text ranges for bold/italic/underline. Unicode input and IME composition use GPUI's input handler. Ordinary annotations preview through GPUI GPU paths. Blur composites render on a background worker; exports use the same annotation coordinates and styles.

## Build

Requires Rust, Xcode, and its Metal toolchain. Install the toolchain if missing:

```sh
xcodebuild -downloadComponent MetalToolchain
```

```sh
python3 scripts/prepare_vendor.py
cargo build
python3 scripts/bundle_macos.py
```

Open `target/debug/Snipsy.app` in Finder. For a release bundle, use `cargo build --release` and `python3 scripts/bundle_macos.py release`. `cargo run` also works for development.

Run `scripts/prepare_vendor.py` once per checkout, or again after changing the patch. It verifies the GPUI 0.2.2 archive and applies `patches/gpui-frame-resize.patch` to an ignored copy under `vendor/gpui`. Normal Cargo commands then work; `cargo clean` preserves the prepared dependency. Python 3 and the system `patch` utility are required.

Grant Screen Recording to Snipsy in System Settings → Privacy & Security. When launched through a terminal, macOS may attribute permission to that terminal instead. Snipsy starts in the background with no default window. Capture overlaps graphics setup, and the editor stays hidden between captures to reuse its GPU resources. Use a global shortcut to capture or pin an image. Reopening the app starts a capture. Quit with `⌘Q` while an editor or pin is focused.

The menu-bar item provides capture, clipboard pinning, history, whiteboard, pin visibility, and quit actions.
Global capture, paste, click-through, and pin-visibility shortcuts are configurable from Preferences.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Capture | `⌘⇧A` or `⌘⇧L` |
| Pin clipboard image, color, or text | `⌘⇧P` |
| Show / hide all pins | `⇧F3` |
| Select / rectangle / ellipse / arrow | `V` / `R` / `O` / `A` |
| Straight line | `L` |
| Pen / highlighter / text / mosaic / eraser | `B` / `H` / `T` / `M` / `E` |
| Copy / save / pin selection | `Enter` or `⌘C` / `⌘S` / `⌘T` |
| Print selection or pin | `⌘P` |
| Quick-save selection | `⌘⇧S` |
| Undo / redo | `⌘Z` / `⌘⇧Z` |
| Select full display | `⌘A` or `Space` before selecting |
| Show / hide annotation bar | `Space` after selecting |
| Restore last capture region | `R` before selecting |
| Previous / next capture | `,` / `.` |
| Toggle cursor capture | Backtick (`` ` ``) |
| Copy magnifier color / show RGB | `C` / hold `Shift` while selecting a capture region |
| Move pointer before selecting | `W` / `A` / `S` / `D` |
| Move / grow / shrink selection | Arrow keys / `⌘`+arrows / `⇧`+arrows |
| Move selected annotation | Arrow keys; `⇧` for 10 points |
| Constrain square, circle, or arrow angle | Hold `Shift` while drawing |
| Stroke width | `[` / `]` |
| Commit text / new line | `Enter` / `Shift+Enter` |
| Bold / italic / underline text | `⌘B` / `⌘I` / `⌘U` |
| Cancel text or capture | `Escape` |

Pins: drag to move; dropping a file replaces the pin (`⇧` opens separately); arrow keys move by one point (`⇧` for ten); scroll or `+`/`-` to resize; `⌘`+scroll, `⌘`+`+`/`-`, or `[`/`]` for opacity; `1`/`2` rotate; `3`/`4` flip; `0` or middle-click restores size and opacity; `⇧`+double-click toggles thumbnail mode; `Space` edits annotations; hold `⌥` to sample a pixel and press `C` to copy it (`⇧C` for RGB); `⌘⇧X` toggles click-through for the pin under the pointer (or restores all pins); `⌘C` copies; `⌘⇧C` copies source text; `⌘V` replaces from the clipboard; `⌘S` saves; `⌘⇧S` quick-saves; `Escape`, `⌘W`, or double-click closes recoverably; `⇧Escape` destroys. Hover a pin for its controls.

Animated GIF pins use `P` to pause/resume, `1`/`2` to step frames, and `⌥`+scroll to adjust playback speed.

Open pins, including animated GIFs, are backed up in Application Support and restored after relaunch with their position, size, opacity, orientation, thumbnail state, and source text.

Startup commands: `snipsy snip`, `snipsy paste`, `snipsy paste --plain TEXT`, `snipsy paste --files FILE...`, `snipsy history`, `snipsy whiteboard [--color COLOR]`, and `snipsy transparent-whiteboard`.
Commands are forwarded to the resident Snipsy process; a command starts the resident process when needed.
Image groups are available from the menu-bar manager or `create-group`, `switch-group`, `switch-groups`, and `empty-group` commands; new pins enter the active group.

PNG, JPEG, WebP, BMP, TIFF, TGA, ICO, and static GIF exports use an atomic file replacement. The last ten screen captures can be replayed while snipping, and the last ten finished selections persist in Application Support and are available from the History menu.

Google Lens remains available in the editor toolbar. It submits the edited PNG directly from the default browser, via a temporary local handoff page deleted after 30 seconds. The Lens endpoint is undocumented and may change.

## Architecture

- `editor.rs`, `geometry.rs`: annotation model, hit testing, editing, undo/redo; no OS or GPUI types.
- `raster.rs`: crop, annotation rendering, mosaic, PNG encoding, and GPUI image conversion.
- `app.rs`, `selection.rs`, `pin.rs`: GPUI lifecycle and views; GPUI handles clipboard and windowing.
- `platform/macos.rs`: ScreenCaptureKit, exact borderless overlay bounds, background app policy, and native dialogs. This is the boundary to replace for another OS; there is no speculative backend trait.
- `lens.rs`: platform-independent browser handoff document.

Capture spans the connected desktop, including mixed-scale displays. OCR and scrolling capture are not implemented.

## Checks

```sh
cargo check
cargo test
```
