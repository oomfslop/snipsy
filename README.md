# Snipsy

macOS-only proof of concept: press `Command+Shift+L`, select a screen area, confirm the search, and open the Google Lens results in the default browser.

The selected PNG is submitted directly to Google Lens by the default browser. Snipsy does not upload it to an image host or other intermediary. Temporary local files are deleted after the browser handoff.

## Run

```sh
cargo run
```

`cargo run` builds and launches Snipsy. Keep the process running in the terminal; the shortcut is registered globally while it is running.

Snipsy checks Screen Recording access before showing the selection UI. If access is missing, macOS asks for it instead of silently capturing only the desktop. When using `cargo run`, macOS attributes the permission to the parent app, such as Terminal, iTerm, Visual Studio Code, or Codex. Grant that app access in System Settings → Privacy & Security → Screen & System Audio Recording, then trigger the shortcut again.

Press `Escape` or the cancel button to dismiss the selection. The search button captures the selected area and opens a local handoff page that POSTs `encoded_image` to `https://lens.google.com/v3/upload` in the browser.

The Lens upload endpoint is undocumented and may change. The flow is based on the multipart/redirect pattern used by [Search-on-Google-Lens](https://github.com/typeling1578/Search-on-Google-Lens/blob/main/post_to_lens.mjs) and [this macOS shell action](https://gist.github.com/bxb100/6734e7fdc5396bcfcaf34c809c8334f6).

## macOS screen-capture reminder

macOS may periodically ask an already-authorized app to “bypass the system private window picker.” Avoiding that reminder through supported APIs requires replacing Snipsy's region selector with Apple's picker.

An unsupported workaround changes the per-executable reminder timestamp in `~/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist`. It requires Full Disk Access for the terminal running these commands:

```sh
defaults read "$HOME/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist"

defaults write "$HOME/Library/Group Containers/group.com.apple.replayd/ScreenCaptureApprovals.plist" \
  "<exact executable path from the previous command>" \
  -date "3024-01-01 00:00:00 +0000"
```

Log out and back in afterward. On macOS 15.1+, `replayd` may overwrite the timestamp when capture is used; [screencapture-nag-remover](https://github.com/luckman212/screencapture-nag-remover) handles that with a daily LaunchAgent. This relies on a private macOS implementation detail and may stop working after an update.
