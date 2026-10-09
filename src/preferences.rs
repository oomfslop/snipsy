use crate::app::{self, HotkeyKind};
use global_hotkey::hotkey::{Code, HotKey, Modifiers as HotKeyModifiers};
use gpui::{prelude::*, *};

pub struct Preferences {
    focus: FocusHandle,
    recording: Option<HotkeyKind>,
    error: Option<String>,
}

impl Preferences {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        return Self {
            focus,
            recording: None,
            error: None,
        };
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "escape" {
            if self.recording.take().is_none() {
                window.remove_window();
            }
            cx.notify();
            return;
        }
        let Some(kind) = self.recording else {
            return;
        };
        let Some(hotkey) = create_hotkey(&event.keystroke) else {
            self.error = Some("Use a letter, number, function key, or standard punctuation".into());
            cx.notify();
            return;
        };
        match app::set_hotkey(kind, hotkey, cx) {
            Ok(()) => {
                self.recording = None;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        cx.notify();
    }
}

impl Render for Preferences {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = app::get_hotkey_settings(cx);
        let rows = [
            ("Capture", HotkeyKind::Capture),
            ("Pin clipboard", HotkeyKind::Paste),
            ("Toggle click-through", HotkeyKind::ClickThrough),
            ("Show/hide pins", HotkeyKind::TogglePins),
        ];
        div()
            .id("preferences")
            .size_full()
            .track_focus(&self.focus)
            .bg(rgb(0x202024))
            .text_color(rgb(0xffffff))
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
            .on_key_down(cx.listener(Self::handle_key))
            .children(rows.into_iter().map(|(label, kind)| {
                let recording = self.recording == Some(kind);
                let value: SharedString = if recording {
                    "Press shortcut…".into()
                } else {
                    format_hotkey(settings.get(kind)).into()
                };
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label)
                    .child(
                        div()
                            .id(("hotkey", kind as usize))
                            .min_w(px(150.0))
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .text_center()
                            .cursor_pointer()
                            .bg(rgb(if recording { 0x327fff } else { 0x34343a }))
                            .hover(|style| style.bg(rgb(0x414148)))
                            .child(value)
                            .on_click(cx.listener(move |state, _, window, cx| {
                                state.recording = Some(kind);
                                state.error = None;
                                window.focus(&state.focus);
                                cx.notify();
                            })),
                    )
            }))
            .when_some(self.error.clone(), |root, error| {
                root.child(div().text_color(rgb(0xff8686)).child(error))
            })
    }
}

fn create_hotkey(keystroke: &Keystroke) -> Option<HotKey> {
    let key = match keystroke.key.as_str() {
        key if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() => {
            format!("Key{}", key.to_ascii_uppercase())
        }
        key if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() => format!("Digit{key}"),
        "`" => "Backquote".into(),
        "\\" => "Backslash".into(),
        "[" => "BracketLeft".into(),
        "]" => "BracketRight".into(),
        "," => "Comma".into(),
        "=" => "Equal".into(),
        "-" => "Minus".into(),
        "." => "Period".into(),
        "'" => "Quote".into(),
        ";" => "Semicolon".into(),
        "/" => "Slash".into(),
        "space" => "Space".into(),
        "enter" => "Enter".into(),
        "backspace" => "Backspace".into(),
        "delete" => "Delete".into(),
        key if key.starts_with('f') && key[1..].parse::<u8>().is_ok() => key.to_ascii_uppercase(),
        _ => return None,
    };
    let code = key.parse::<Code>().ok()?;
    let mut modifiers = HotKeyModifiers::empty();
    modifiers.set(HotKeyModifiers::CONTROL, keystroke.modifiers.control);
    modifiers.set(HotKeyModifiers::ALT, keystroke.modifiers.alt);
    modifiers.set(HotKeyModifiers::SHIFT, keystroke.modifiers.shift);
    modifiers.set(HotKeyModifiers::SUPER, keystroke.modifiers.platform);
    if modifiers.is_empty() && !keystroke.key.starts_with('f') {
        return None;
    }
    return Some(HotKey::new(Some(modifiers), code));
}

fn format_hotkey(hotkey: HotKey) -> String {
    let mut output = String::new();
    if hotkey.mods.contains(HotKeyModifiers::CONTROL) {
        output.push('⌃');
    }
    if hotkey.mods.contains(HotKeyModifiers::ALT) {
        output.push('⌥');
    }
    if hotkey.mods.contains(HotKeyModifiers::SHIFT) {
        output.push('⇧');
    }
    if hotkey.mods.contains(HotKeyModifiers::SUPER) {
        output.push('⌘');
    }
    let key = hotkey.key.to_string();
    output.push_str(
        key.strip_prefix("Key")
            .or_else(|| key.strip_prefix("Digit"))
            .unwrap_or(&key),
    );
    return output;
}

#[cfg(test)]
mod tests {
    use super::{create_hotkey, format_hotkey};
    use gpui::{Keystroke, Modifiers};

    #[test]
    fn shortcut_recorder_maps_gpui_keys_and_requires_safe_modifiers() {
        let hotkey = create_hotkey(&Keystroke {
            modifiers: Modifiers {
                platform: true,
                shift: true,
                ..Default::default()
            },
            key: "a".into(),
            key_char: None,
        })
        .unwrap();
        assert_eq!(format_hotkey(hotkey), "⇧⌘A");
        assert!(
            create_hotkey(&Keystroke {
                key: "a".into(),
                ..Default::default()
            })
            .is_none()
        );
        assert!(
            create_hotkey(&Keystroke {
                key: "f3".into(),
                ..Default::default()
            })
            .is_some()
        );
    }
}
