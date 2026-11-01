# Snipsy

Screenshot tool for macOS 14+, inspired by Lightshot and Snipaste. Built with [GPUI Kit](https://gpui-kit.com).

## Build

Requires Rust and Xcode with its Metal toolchain (`xcodebuild -downloadComponent MetalToolchain`). 

```sh
# we're using a patched version of gpui-pre-macos
./scripts/prepare-vendor.sh
cargo build --release
python3 scripts/bundle_macos.py release

# or you can just
cargo run
```

The main reason we need a `.app` is the macOS permissions stuff - `cargo run`-ning will attribute the permission to the terminal, which won't apply when running separately (e.g. on system launch via launchd).

To bypass this, the bundle script signs the app with a self-signed `snipsy-dev` identity, created in the login keychain on first run, so the grant survives rebuilds.

To install, run `scripts/install.sh` - it builds, bundles, quits the installed copy, replaces `/Applications/Snipsy.app`, and launches it. Then you can enable "Launch at login" in Preferences → General.

## Command line

```
snipsy [snip [--position X,Y] [--size WxH] [--delay SECONDS] | custom-snip
        | record | paste [--plain TEXT... | --files FILE...]
        | whiteboard [--color COLOR] | toggle-pins | toggle-click-through
        | preferences | quit]
```

`snip --position` and `--size` are in pixels from the desktop's top-left. Given both, the region is selected right away; given one, a fixed-size or fixed-corner selection follows the pointer and a click places it. `custom-snip` opens the same options in a dialog.

Commands are forwarded to the running instance and start it when needed. `COLOR` accepts names, `#rgb[a]`, `#rrggbb[aa]`, and `r g b` (0–255 or 0.0–1.0); `--color transparent` makes a transparent whiteboard.

## Shortcuts

Global shortcuts and the capture toolbar layout are configurable in Preferences, including custom snip shortcuts with a fixed region or delay.

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

```sh
cargo test
cargo clippy --all-targets
```
