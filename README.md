# Snipsy

GPUI screenshot editor for macOS 14+. Capture a frozen screen region or click a window, annotate it, copy/save it, or pin it above other apps.

Tools: rectangle, ellipse, arrow, pen, highlighter, text, mosaic, and annotation eraser. Annotations can be moved, recolored, resized in stroke width, deleted, and undone/redone. Double-click text to edit; Unicode input and IME composition use GPUI's input handler. Ordinary annotations preview through GPUI GPU paths. Mosaic composites render on a background worker; exports use the same annotation coordinates and styles.

## Build

Requires Rust, Xcode, and its Metal toolchain. Install the toolchain if missing:

```sh
xcodebuild -downloadComponent MetalToolchain
```

```sh
cargo build
python3 scripts/bundle_macos.py
```

Open `target/debug/Snipsy.app` in Finder. For a release bundle, use `cargo build --release` and `python3 scripts/bundle_macos.py release`. `cargo run` also works for development.

Grant Screen Recording to Snipsy in System Settings → Privacy & Security. When launched through a terminal, macOS may attribute permission to that terminal instead. Snipsy starts in the background with no default window. Capture overlaps graphics setup, and the editor stays hidden between captures to reuse its GPU resources. Use a global shortcut to capture or pin an image. Reopening the app starts a capture. Quit with `⌘Q` while an editor or pin is focused.

## Shortcuts

| Action | Shortcut |
| --- | --- |
| Capture | `⌘⇧A` or `⌘⇧L` |
| Pin clipboard image | `⌘⇧P` |
| Select / rectangle / ellipse / arrow | `V` / `R` / `O` / `A` |
| Pen / highlighter / text / mosaic / eraser | `B` / `H` / `T` / `M` / `E` |
| Copy / save / pin selection | `Enter` or `⌘C` / `⌘S` / `⌘P` |
| Undo / redo | `⌘Z` / `⌘⇧Z` |
| Select full display | `⌘A` or `Space` before selecting |
| Move selection or selected annotation | Arrow keys; `Shift` for 10 points |
| Constrain square, circle, or arrow angle | Hold `Shift` while drawing |
| Stroke width | `[` / `]` |
| Commit text / new line | `Enter` / `Shift+Enter` |
| Cancel text or capture | `Escape` |

Pins: drag to move; scroll or `+`/`-` to resize; `⌘`+scroll or `[`/`]` for opacity; `1` restores the initial size; `⌘C` copies; `⌘S` saves; `Escape` closes. Hover a pin for its controls.

PNG and JPEG exports use an atomic file replacement. The last ten finished captures remain in memory; history UI and settings are deferred. History does not persist after quitting.

Google Lens remains available in the editor toolbar. It submits the edited PNG directly from the default browser, via a temporary local handoff page deleted after 30 seconds. The Lens endpoint is undocumented and may change.

## Architecture

- `editor.rs`, `geometry.rs`: annotation model, hit testing, editing, undo/redo; no OS or GPUI types.
- `raster.rs`: crop, annotation rendering, mosaic, PNG encoding, and GPUI image conversion.
- `app.rs`, `selection.rs`, `pin.rs`: GPUI lifecycle and views; GPUI handles clipboard and windowing.
- `platform/macos.rs`: ScreenCaptureKit, exact borderless overlay bounds, background app policy, and native dialogs. This is the boundary to replace for another OS; there is no speculative backend trait.
- `lens.rs`: platform-independent browser handoff document.

Capture currently targets the display under the pointer. OCR, scrolling capture, and persistent history are not implemented.

## Checks

```sh
cargo check
cargo test
```
