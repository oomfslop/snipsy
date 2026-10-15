use crate::{
    app::{
        self,
        hotkeys::{self, HotkeyKind},
        toolbar_layout::{self, ToolbarItem, ToolbarLayout},
    },
    capture::toolbar::get_item_spec,
    ui,
};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Disableable as _, Icon, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        group_box::GroupBoxVariant,
        h_flex,
        separator::Separator,
        setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
        v_flex,
    },
    prelude::*,
    *,
};

const SIDEBAR_WIDTH: Pixels = px(200.);

#[derive(Clone)]
struct DraggedItem {
    item: ToolbarItem,
    index: Option<usize>,
}

pub struct PreferencesView {
    focus: FocusHandle,
    recording: Option<HotkeyKind>,
    error: Option<(HotkeyKind, SharedString)>,
    drop_index: Option<usize>,
}

impl PreferencesView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        cx.observe_global::<ToolbarLayout>(|_, cx| cx.notify())
            .detach();
        return Self {
            focus,
            recording: None,
            error: None,
            drop_index: None,
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
            .and_then(|hotkey| hotkeys::set_hotkey(kind, Some(hotkey), cx));
        match result {
            Ok(()) => {
                self.recording = None;
                self.error = None;
            }
            Err(error) => self.error = Some((kind, format!("{error:#}").into())),
        }
        cx.notify();
    }

    fn drop_item(&mut self, dragged: &DraggedItem, cx: &mut Context<Self>) {
        let mut items = cx.global::<ToolbarLayout>().items.clone();
        let index = self.drop_index.take().unwrap_or(items.len());
        move_item(&mut items, dragged, index);
        update_toolbar(items, cx);
    }

    fn remove_item(&mut self, dragged: &DraggedItem, cx: &mut Context<Self>) {
        self.drop_index = None;
        let Some(index) = dragged.index else {
            return;
        };
        let mut items = cx.global::<ToolbarLayout>().items.clone();
        items.remove(index);
        update_toolbar(items, cx);
    }
}

fn update_toolbar(items: Vec<ToolbarItem>, cx: &mut App) {
    if let Err(error) = toolbar_layout::set_items(items, cx) {
        app::show_error(error);
    }
}

fn move_item(items: &mut Vec<ToolbarItem>, dragged: &DraggedItem, mut index: usize) {
    if let Some(from) = dragged.index {
        items.remove(from);
        if from < index {
            index -= 1;
        }
    }
    items.insert(index.min(items.len()), dragged.item);
}

impl Render for PreferencesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let shortcuts = SettingGroup::new()
            .description("Available everywhere, even while Snipsy is in the background.")
            .items(HotkeyKind::ALL.map(|kind| {
                let view = view.clone();
                SettingItem::new(
                    kind.get_label(),
                    SettingField::render(move |_, window, cx| {
                        render_hotkey_field(&view, kind, window, cx)
                    }),
                )
            }));
        let toolbar = SettingGroup::new().item(
            SettingItem::render(move |_, window, cx| render_toolbar_editor(&view, window, cx))
                .keywords(["toolbar", "buttons", "tools"])
                .on_reset(
                    |cx| !cx.global::<ToolbarLayout>().get_is_default(),
                    |_, cx| {
                        if let Err(error) = toolbar_layout::reset(cx) {
                            app::show_error(error);
                        }
                    },
                ),
        );
        let theme = cx.theme();
        let header = StyleRefinement::default()
            .pt(px(17.))
            .pb_1()
            .border_b_0()
            .text_size(px(15.))
            .font_weight(FontWeight::BOLD);
        return div()
            .id("preferences")
            .map(crate::ui::handle_window_actions)
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::handle_key))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .left(SIDEBAR_WIDTH)
                    .bg(theme.background),
            )
            .child(
                Settings::new("settings")
                    .sidebar_width(SIDEBAR_WIDTH)
                    .sidebar_size_range(SIDEBAR_WIDTH..SIDEBAR_WIDTH)
                    .with_group_variant(GroupBoxVariant::Fill)
                    .header_style(&StyleRefinement::default().pt(px(44.)))
                    .page(
                        SettingPage::new("Shortcuts")
                            .icon(IconName::Keyboard)
                            .header_style(&header)
                            .group(shortcuts),
                    )
                    .page(
                        SettingPage::new("Toolbar")
                            .icon(IconName::PanelTop)
                            .header_style(&header)
                            .group(toolbar),
                    ),
            );
    }
}

fn render_hotkey_field(
    view: &Entity<PreferencesView>,
    kind: HotkeyKind,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let state = view.read(cx);
    let recording = state.recording == Some(kind);
    let error = state
        .error
        .clone()
        .filter(|(failed, _)| *failed == kind)
        .map(|(_, error)| error);
    let hotkey = hotkeys::get_settings(cx).map(|settings| settings.get(kind));
    let label = match hotkey {
        _ if recording => "Press shortcut…".into(),
        None => "Unavailable".into(),
        Some(None) => "Not set".into(),
        Some(Some(hotkey)) => format_hotkey(hotkey),
    };
    return v_flex()
        .items_end()
        .gap_1()
        .child(
            h_flex()
                .gap_1()
                .child(
                    Button::new(("hotkey", kind as usize))
                        .secondary()
                        .small()
                        .min_w(px(140.))
                        .selected(recording)
                        .label(label)
                        .on_click(window.listener_for(view, move |state, _, window, cx| {
                            state.recording = Some(kind);
                            state.error = None;
                            window.focus(&state.focus, cx);
                            cx.notify();
                        })),
                )
                .child(
                    Button::new(("clear-hotkey", kind as usize))
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .tooltip("Clear shortcut")
                        .disabled(!matches!(hotkey, Some(Some(_))))
                        .on_click(window.listener_for(view, move |state, _, _, cx| {
                            state.recording = None;
                            state.error = hotkeys::set_hotkey(kind, None, cx)
                                .err()
                                .map(|error| (kind, format!("{error:#}").into()));
                            cx.notify();
                        })),
                ),
        )
        .when_some(error, |field, error| {
            field.child(div().text_xs().text_color(cx.theme().danger).child(error))
        });
}

fn render_toolbar_editor(
    view: &Entity<PreferencesView>,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement + use<> {
    let items = cx.global::<ToolbarLayout>().items.clone();
    let drop_index = view.read(cx).drop_index;
    let theme = cx.theme();
    let accent = theme.primary;
    let count = items.len();
    let preview = items.iter().copied().enumerate().fold(
        ui::create_toolbar("toolbar-preview"),
        |toolbar, (index, item)| {
            let indicator = |left: bool| {
                div()
                    .absolute()
                    .top_1()
                    .bottom_1()
                    .w(px(2.))
                    .rounded_full()
                    .bg(accent)
                    .map(|bar| {
                        if left {
                            bar.left(px(-3.))
                        } else {
                            bar.right(px(-3.))
                        }
                    })
            };
            let slot = div()
                .id(("toolbar-slot", index))
                .relative()
                .flex()
                .items_center()
                .h_8()
                .cursor_grab()
                .on_drag(
                    DraggedItem {
                        item,
                        index: Some(index),
                    },
                    |dragged, _, _, cx| cx.new(|_| ItemDragPreview(dragged.item)),
                )
                .on_drag_move(window.listener_for(
                    view,
                    move |state, event: &DragMoveEvent<DraggedItem>, _, cx| {
                        let position = event.event.position;
                        if !event.bounds.contains(&position) {
                            return;
                        }
                        let after = position.x > event.bounds.center().x;
                        let target = Some(index + usize::from(after));
                        if state.drop_index != target {
                            state.drop_index = target;
                            cx.notify();
                        }
                    },
                ))
                .when(drop_index == Some(index), |slot| {
                    slot.child(indicator(true))
                })
                .when(index + 1 == count && drop_index == Some(count), |slot| {
                    slot.child(indicator(false))
                });
            toolbar.content(match get_item_spec(item, false) {
                Some(spec) => slot.child(
                    Button::new(("toolbar-item", item as usize))
                        .with_size(component::Size::Medium)
                        .prepare_for_toolbar()
                        .icon(spec.icon)
                        .tooltip(spec.get_tooltip()),
                ),
                None => slot.px_1().child(Separator::vertical().h_4()),
            })
        },
    );
    let palette = ToolbarItem::ALL
        .into_iter()
        .filter(|item| *item == ToolbarItem::Separator || !items.contains(item))
        .map(|item| {
            let (icon, label) = match get_item_spec(item, false) {
                Some(spec) => (spec.icon, spec.label),
                None => (IconName::GripVertical, "Separator"),
            };
            v_flex()
                .id(("palette-item", item as usize))
                .w(px(84.))
                .h(px(72.))
                .items_center()
                .justify_center()
                .gap_1p5()
                .px_1()
                .rounded_lg()
                .cursor_grab()
                .hover(|style| style.bg(theme.accent))
                .on_drag(DraggedItem { item, index: None }, |dragged, _, _, cx| {
                    cx.new(|_| ItemDragPreview(dragged.item))
                })
                .child(Icon::new(icon).size_5())
                .child(
                    div()
                        .w_full()
                        .text_xs()
                        .text_center()
                        .line_height(rems(1.1))
                        .text_color(theme.muted_foreground)
                        .child(label),
                )
        })
        .collect::<Vec<_>>();
    return v_flex()
        .w_full()
        .gap_4()
        .child(
            ui::create_toolbar_panel(cx)
                .id("toolbar-drop")
                .w_full()
                .flex()
                .overflow_x_scroll()
                .map(|panel| {
                    if items.is_empty() {
                        panel.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("Drag items here"),
                        )
                    } else {
                        panel.child(preview)
                    }
                })
                .on_drag_move(window.listener_for(
                    view,
                    |state, event: &DragMoveEvent<DraggedItem>, _, cx| {
                        if !event.bounds.contains(&event.event.position)
                            && state.drop_index.take().is_some()
                        {
                            cx.notify();
                        }
                    },
                ))
                .drag_over::<DraggedItem>(move |style, _, _, _| style.border_color(accent))
                .on_drop(
                    window.listener_for(view, |state, dragged, _, cx| state.drop_item(dragged, cx)),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Drag items into the toolbar to add them, or back here to remove them."),
        )
        .child(
            div()
                .id("toolbar-palette")
                .flex()
                .flex_wrap()
                .gap_2()
                .rounded_xl()
                .border_1()
                .border_color(transparent_black())
                .drag_over::<DraggedItem>(move |style, dragged, _, _| {
                    if dragged.index.is_some() {
                        style.border_color(accent)
                    } else {
                        style
                    }
                })
                .on_drop(
                    window
                        .listener_for(view, |state, dragged, _, cx| state.remove_item(dragged, cx)),
                )
                .children(palette),
        );
}

struct ItemDragPreview(ToolbarItem);

impl Render for ItemDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = get_item_spec(self.0, false).map_or(IconName::GripVertical, |spec| spec.icon);
        return ui::create_toolbar_panel(cx)
            .size(px(42.))
            .flex()
            .items_center()
            .justify_center()
            .child(Icon::new(icon));
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
    use super::{
        DraggedItem, PreferencesView, create_hotkey, format_hotkey, move_item,
        render_toolbar_editor,
    };
    use crate::app::toolbar_layout::{ToolbarItem::*, ToolbarLayout};
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, Keystroke, Modifiers, Render,
        TestAppContext, Window,
    };

    struct ToolbarEditorHost(Entity<PreferencesView>);

    impl Render for ToolbarEditorHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            return render_toolbar_editor(&self.0, window, cx);
        }
    }

    #[gpui_kit::test]
    fn toolbar_editor_renders_layout_and_empty_toolbar(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(ToolbarLayout::default());
        });
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|cx| PreferencesView::new(window, cx));
            let host = cx.new(|_| ToolbarEditorHost(view));
            gpui_kit::base::Root::new(host, window, cx)
        });
        cx.run_until_parked();
        cx.update(|cx| cx.global_mut::<ToolbarLayout>().items.clear());
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn preferences_render_both_pages(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(ToolbarLayout::default());
        });
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|cx| PreferencesView::new(window, cx));
            gpui_kit::base::Root::new(view, window, cx)
        });
        cx.run_until_parked();
        window.update(cx, |_, window, _| window.refresh()).unwrap();
        cx.run_until_parked();
    }

    #[test]
    fn dragging_items_moves_inserts_and_keeps_positions() {
        let drag = |item, index| DraggedItem { item, index };
        let mut items = vec![Pen, Text, Copy];
        move_item(&mut items, &drag(Pen, Some(0)), 2);
        assert_eq!(items, [Text, Pen, Copy]);
        move_item(&mut items, &drag(Copy, Some(2)), 0);
        assert_eq!(items, [Copy, Text, Pen]);
        move_item(&mut items, &drag(Text, Some(1)), 1);
        assert_eq!(items, [Copy, Text, Pen]);
        move_item(&mut items, &drag(Separator, None), 1);
        assert_eq!(items, [Copy, Separator, Text, Pen]);
        move_item(&mut items, &drag(Record, None), 10);
        assert_eq!(items, [Copy, Separator, Text, Pen, Record]);
    }

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
