# Cursor Demo

Hover tiles to load their preview and test cursors; click to copy the selector. Missing selectors show Unavailable. Includes all zero-argument cursor getters in [Mousecape’s header](https://github.com/alexzielenski/Mousecape/blob/master/Mousecape/mousecloak/NSCursor_Private.h), excluding `_makeCursors`. Private getters are called only on hover; console logs identify each getter and image lookup.

From the repository root:

```sh
mkdir -p target/CursorDemo.app/Contents/MacOS
swiftc -framework AppKit examples/CursorDemo/main.swift -o target/CursorDemo.app/Contents/MacOS/CursorDemo
cp examples/CursorDemo/Info.plist target/CursorDemo.app/Contents/Info.plist
open target/CursorDemo.app
```
