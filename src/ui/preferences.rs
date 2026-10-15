use crate::app::hotkeys::{self, HotkeyKind};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use gpui_kit::{
    component::{
        ActiveTheme, Selectable as _, Sizable as _, StyledExt as _, button::Button, h_flex, v_flex,
    },
    prelude::*,
    *,
};

pub struct PreferencesView {
    focus: FocusHandle,
    recording: Option<HotkeyKind>,
    error: Option<SharedString>,
}

impl PreferencesView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
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
        let result = create_hotkey(&event.keystroke)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Use a modifier with a letter, number, or punctuation key, or a function key"
                )
            })
            .and_then(|hotkey| hotkeys::set_hotkey(kind, hotkey, cx));
        match result {
            Ok(()) => {
                self.recording = None;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}").into()),
        }
        cx.notify();
    }
}

impl Render for PreferencesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = hotkeys::get_settings(cx);
        let theme = cx.theme();
        return v_flex()
            .id("preferences")
            .size_full()
            .track_focus(&self.focus)
            .bg(theme.background)
            .p_4()
            .gap_2()
            .on_key_down(cx.listener(Self::handle_key))
            .child(div().text_sm().font_semibold().child("Global shortcuts"))
            .children(HotkeyKind::ALL.map(|kind| {
                let recording = self.recording == Some(kind);
                h_flex().justify_between().child(kind.get_label()).child(
                    Button::new(kind.get_label())
                        .outline()
                        .small()
                        .min_w(px(140.))
                        .selected(recording)
                        .label(if recording {
                            "Press shortcut…".into()
                        } else {
                            settings.map_or("Unavailable".into(), |settings| {
                                format_hotkey(settings.get(kind))
                            })
                        })
                        .on_click(cx.listener(move |state, _, window, cx| {
                            state.recording = Some(kind);
                            state.error = None;
                            window.focus(&state.focus, cx);
                            cx.notify();
                        })),
                )
            }))
            .when_some(self.error.clone(), |root, error| {
                root.child(div().text_sm().text_color(theme.danger).child(error))
            });
    }
}

fn create_hotkey(keystroke: &Keystroke) -> Option<HotKey> {
    let key = keystroke.key.as_str();
    let code = match key {
        "`" => "Backquote",
        "\\" => "Backslash",
        "[" => "BracketLeft",
        "]" => "BracketRight",
        "," => "Comma",
        "=" => "Equal",
        "-" => "Minus",
        "." => "Period",
        "'" => "Quote",
        ";" => "Semicolon",
        "/" => "Slash",
        "space" => "Space",
        "enter" => "Enter",
        "backspace" => "Backspace",
        "delete" => "Delete",
        _ => "",
    };
    let function_key = key.starts_with('f') && key[1..].parse::<u8>().is_ok();
    let code = if !code.is_empty() {
        code.to_string()
    } else if key.len() == 1 && key.as_bytes()[0].is_ascii_alphabetic() {
        format!("Key{}", key.to_ascii_uppercase())
    } else if key.len() == 1 && key.as_bytes()[0].is_ascii_digit() {
        format!("Digit{key}")
    } else if function_key {
        key.to_ascii_uppercase()
    } else {
        return None;
    };
    let code = code.parse::<Code>().ok()?;
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::CONTROL, keystroke.modifiers.control);
    modifiers.set(Modifiers::ALT, keystroke.modifiers.alt);
    modifiers.set(Modifiers::SHIFT, keystroke.modifiers.shift);
    modifiers.set(Modifiers::SUPER, keystroke.modifiers.platform);
    if modifiers.is_empty() && !function_key {
        return None;
    }
    return Some(HotKey::new(Some(modifiers), code));
}

fn format_hotkey(hotkey: HotKey) -> SharedString {
    let mut output = String::new();
    for (modifier, symbol) in [
        (Modifiers::CONTROL, '⌃'),
        (Modifiers::ALT, '⌥'),
        (Modifiers::SHIFT, '⇧'),
        (Modifiers::SUPER, '⌘'),
    ] {
        if hotkey.mods.contains(modifier) {
            output.push(symbol);
        }
    }
    let key = hotkey.key.to_string();
    output.push_str(
        key.strip_prefix("Key")
            .or_else(|| key.strip_prefix("Digit"))
            .unwrap_or(&key),
    );
    return output.into();
}

#[cfg(test)]
mod tests {
    use super::{create_hotkey, format_hotkey};
    use gpui_kit::{Keystroke, Modifiers};

    #[test]
    fn shortcut_recorder_maps_gpui_keys_and_requires_safe_modifiers() {
        let keystroke = |key: &str, modifiers| Keystroke {
            modifiers,
            key: key.into(),
            key_char: None,
        };
        let command_shift = Modifiers {
            platform: true,
            shift: true,
            ..Default::default()
        };
        assert_eq!(
            format_hotkey(create_hotkey(&keystroke("a", command_shift)).unwrap()),
            "⇧⌘A"
        );
        assert!(create_hotkey(&keystroke("a", Modifiers::default())).is_none());
        assert!(create_hotkey(&keystroke("f3", Modifiers::default())).is_some());
        assert!(create_hotkey(&keystroke("weird", command_shift)).is_none());
    }
}
