use crate::{
    app::{
        config::Config,
        toolbar_layout::{self, ToolbarItem},
    },
    capture::{CaptureView, EditorAction, Mode, get_tool_family, layout},
    editor::{BlurMode, TextAttribute, Tool},
    geometry::{Color, Rect},
    ui,
};
use glam::{Vec2, vec2};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Disableable as _, Icon, Selectable as _, Sizable as _, StyledExt as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        separator::Separator,
        slider::Slider,
    },
    prelude::*,
    *,
};

pub struct ToolSpec {
    pub tool: Tool,
    pub icon: IconName,
    pub label: &'static str,
    pub key: &'static str,
}

pub const TOOLS: [ToolSpec; 10] = [
    ToolSpec {
        tool: Tool::Select,
        icon: IconName::MousePointer2,
        label: "Select and move",
        key: "v",
    },
    ToolSpec {
        tool: Tool::Rectangle,
        icon: IconName::Square,
        label: "Rectangle",
        key: "r",
    },
    ToolSpec {
        tool: Tool::Ellipse,
        icon: IconName::Circle,
        label: "Oval",
        key: "o",
    },
    ToolSpec {
        tool: Tool::Arrow,
        icon: IconName::MoveUpRight,
        label: "Arrow",
        key: "a",
    },
    ToolSpec {
        tool: Tool::Line,
        icon: IconName::Minus,
        label: "Line",
        key: "l",
    },
    ToolSpec {
        tool: Tool::Pen,
        icon: IconName::PenLine,
        label: "Pen",
        key: "b",
    },
    ToolSpec {
        tool: Tool::Highlighter,
        icon: IconName::Highlighter,
        label: "Highlighter",
        key: "h",
    },
    ToolSpec {
        tool: Tool::Text,
        icon: IconName::Type,
        label: "Text",
        key: "t",
    },
    ToolSpec {
        tool: Tool::Mosaic,
        icon: IconName::Grid2x2,
        label: "Blur",
        key: "m",
    },
    ToolSpec {
        tool: Tool::Eraser,
        icon: IconName::Eraser,
        label: "Eraser",
        key: "e",
    },
];

const COLORS: [u32; 8] = [
    0xff4b4b,
    0xffa62b,
    0xffdf38,
    0x45ce88,
    0x327fff,
    0xbf78ff,
    Color::WHITE.0,
    0x19191c,
];

const LAYERS_PANEL_WIDTH: f32 = 176.0;
const LAYER_ROW_HEIGHT: f32 = 24.0;

fn get_tool_spec(tool: Tool) -> &'static ToolSpec {
    return TOOLS.iter().find(|spec| spec.tool == tool).unwrap();
}

pub struct ToolbarItemSpec {
    pub icon: IconName,
    pub label: &'static str,
    shortcut: Option<SharedString>,
    action: EditorAction,
}

impl ToolbarItemSpec {
    pub fn get_tooltip(&self) -> SharedString {
        return match &self.shortcut {
            Some(shortcut) => format!("{} ({shortcut})", self.label).into(),
            None => self.label.into(),
        };
    }
}

pub fn get_item_spec(item: ToolbarItem, pin_editor: bool) -> Option<ToolbarItemSpec> {
    let create = |icon, label, shortcut: Option<&'static str>, action| ToolbarItemSpec {
        icon,
        label,
        shortcut: shortcut.map(SharedString::from),
        action,
    };
    let create_tool = |tool| {
        let spec = get_tool_spec(tool);
        return ToolbarItemSpec {
            icon: spec.icon,
            label: spec.label,
            shortcut: Some(spec.key.to_uppercase().into()),
            action: EditorAction::Tool(tool),
        };
    };
    return Some(match item {
        ToolbarItem::Record => create(
            IconName::CircleDot,
            "Record region",
            None,
            EditorAction::Record,
        ),
        ToolbarItem::Select => create_tool(Tool::Select),
        ToolbarItem::Shape => create_tool(Tool::Rectangle),
        ToolbarItem::Arrow => create_tool(Tool::Arrow),
        ToolbarItem::Pen => create_tool(Tool::Pen),
        ToolbarItem::Highlighter => create_tool(Tool::Highlighter),
        ToolbarItem::Text => create_tool(Tool::Text),
        ToolbarItem::Blur => create_tool(Tool::Mosaic),
        ToolbarItem::Eraser => create_tool(Tool::Eraser),
        ToolbarItem::Undo => create(IconName::Undo2, "Undo", Some("⌘Z"), EditorAction::Undo),
        ToolbarItem::Redo => create(IconName::Redo2, "Redo", Some("⇧⌘Z"), EditorAction::Redo),
        ToolbarItem::Layers => create(IconName::Layers, "Layers", None, EditorAction::Layers),
        ToolbarItem::Search => create(
            IconName::ScanSearch,
            "Search with Google Lens",
            None,
            EditorAction::Search,
        ),
        ToolbarItem::Print => create(IconName::Printer, "Print", Some("⌘P"), EditorAction::Print),
        ToolbarItem::Save => create(IconName::Save, "Save", Some("⌘S"), EditorAction::Save),
        ToolbarItem::Pin => create(IconName::Pin, "Pin", Some("⌘T"), EditorAction::Pin),
        ToolbarItem::Cancel => create(IconName::Close, "Cancel", Some("Esc"), EditorAction::Cancel),
        ToolbarItem::Copy if pin_editor => {
            create(IconName::Check, "Apply", Some("Enter"), EditorAction::Apply)
        }
        ToolbarItem::Copy => create(IconName::Copy, "Copy", Some("Enter"), EditorAction::Copy),
        ToolbarItem::Separator => return None,
    });
}

impl CaptureView {
    pub(super) fn get_custom_swatch(&self, color: Color) -> Option<u32> {
        return [Some(color), self.custom_color]
            .into_iter()
            .flatten()
            .map(|color| color.0)
            .find(|swatch| !COLORS.contains(swatch));
    }

    fn create_button(
        &self,
        id: impl Into<ElementId>,
        icon: impl Into<Icon>,
        tooltip: impl Into<SharedString>,
        action: EditorAction,
        cx: &Context<Self>,
    ) -> Button {
        return Button::new(id)
            .icon(icon)
            .tooltip(tooltip)
            .on_click(cx.listener(move |capture, _, window, cx| {
                cx.stop_propagation();
                capture.perform_action(action, window, cx);
            }));
    }

    pub(super) fn render_size_badge(
        &self,
        rect: Rect,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement {
        let scale = self
            .screenshot
            .as_ref()
            .map_or(window.scale_factor(), |screenshot| {
                screenshot.image.width() as f32 / self.get_canvas().size.x
            });
        let size = (rect.size * scale).round();
        return ui::panel(cx)
            .absolute()
            .left(px(rect.pos.x.clamp(
                layout::MARGIN,
                (self.bounds.size.x - 120.0).max(layout::MARGIN),
            )))
            .top(px((rect.pos.y - 26.0).max(layout::MARGIN)))
            .px_1p5()
            .text_xs()
            .child(format!("{} × {}", size.x, size.y));
    }

    pub(super) fn render_discard_prompt(
        &self,
        selection: Rect,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let prompt = layout::create_centered_rect(selection, vec2(260.0, 124.0), self.bounds.size);
        return ui::panel(cx)
            .id("discard-prompt")
            .absolute()
            .left(px(prompt.pos.x))
            .top(px(prompt.pos.y))
            .w(px(prompt.size.x))
            .p_4()
            .flex()
            .flex_col()
            .gap_1()
            .text_center()
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().font_semibold().child("Discard annotations?"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Your annotations on this capture will be lost."),
            )
            .child(
                h_flex()
                    .mt_3()
                    .gap_2()
                    .child(
                        Button::new("cancel-discard")
                            .label("Cancel")
                            .flex_1()
                            .on_click(cx.listener(|capture, _, _, cx| {
                                capture.confirming_discard = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("discard")
                            .label("Discard")
                            .primary()
                            .flex_1()
                            .on_click(
                                cx.listener(|capture, _, window, cx| capture.dismiss(window, cx)),
                            ),
                    ),
            );
    }

    pub(super) fn render_toolbars(
        &self,
        selection: Rect,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let screen = self.bounds.size;
        let has_action_bar = !cx.global::<Config>().toolbar.is_empty();
        let measured = self.toolbar_size.get();
        if has_action_bar && measured.is_none() {
            let probe = Rect {
                pos: Vec2::splat(layout::MARGIN),
                size: Vec2::ZERO,
            };
            return vec![
                div()
                    .invisible()
                    .child(self.render_action_bar(probe, cx))
                    .into_any_element(),
            ];
        }
        let bar = layout::create_action_bar_rect(
            selection,
            measured.unwrap_or(vec2(600.0, 36.0)),
            screen,
        );
        let layers_panel = self.show_layers.then(|| {
            let height = 36.0 + self.editor.layers.len().max(1) as f32 * (LAYER_ROW_HEIGHT + 2.0);
            layout::create_layers_panel_rect(
                selection,
                bar,
                vec2(LAYERS_PANEL_WIDTH, height.min(300.0)),
                screen,
            )
        });
        let mut elements = Vec::new();
        if has_action_bar {
            elements.push(self.render_action_bar(bar, cx).into_any_element());
        }
        let tool = self.editor.get_current_tool();
        if !matches!(tool, Tool::Select | Tool::Eraser) {
            elements.push(
                self.render_tool_options(tool, bar, layers_panel, cx)
                    .into_any_element(),
            );
        }
        if let Some(panel) = layers_panel {
            elements.push(self.render_layers_panel(panel, cx).into_any_element());
        }
        return elements;
    }

    fn render_action_bar(&self, bar: Rect, cx: &mut Context<Self>) -> impl IntoElement {
        let measured = self.toolbar_size.clone();
        let entity = cx.entity_id();
        let family = get_tool_family(self.editor.tool);
        let pin_editor = matches!(self.mode, Mode::PinEditor(_));
        let items = toolbar_layout::get_visible_items(&cx.global::<Config>().toolbar, |item| {
            item == ToolbarItem::Record && !matches!(self.mode, Mode::Capture)
        });
        let toolbar = items
            .into_iter()
            .fold(ui::create_toolbar("actions"), |toolbar, item| {
                let Some(spec) = get_item_spec(item, pin_editor) else {
                    return toolbar.content(Separator::vertical().h_4());
                };
                let button = self.create_button(
                    ("toolbar-item", item as usize),
                    spec.icon,
                    spec.get_tooltip(),
                    spec.action,
                    cx,
                );
                toolbar.child(match spec.action {
                    EditorAction::Tool(tool) => button.selected(family == tool),
                    EditorAction::Layers => button.selected(self.show_layers),
                    EditorAction::Undo => {
                        button.disabled(self.editor.undo.is_empty() && self.editor.draft.is_none())
                    }
                    EditorAction::Redo => button.disabled(self.editor.redo.is_empty()),
                    _ => button,
                })
            });
        return ui::create_toolbar_panel(cx)
            .id("action-bar")
            .absolute()
            .left(px(bar.pos.x))
            .top(px(bar.pos.y))
            .max_w(px(self.bounds.size.x - 2.0 * layout::MARGIN))
            .cursor(CursorStyle::Arrow)
            .overflow_x_scroll()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let size = vec2(bounds.size.width.into(), bounds.size.height.into());
                        if measured.replace(Some(size)) != Some(size) {
                            cx.notify(entity);
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(toolbar);
    }

    fn render_tool_options(
        &self,
        tool: Tool,
        bar: Rect,
        layers_panel: Option<Rect>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let screen = self.bounds.size;
        let y = layout::get_options_bar_y(bar, screen);
        let mut max_width = screen.x - bar.pos.x - layout::MARGIN;
        if let Some(panel) = layers_panel
            && panel.pos.x > bar.pos.x
            && y < panel.end().y
            && y + bar.size.y > panel.pos.y
        {
            max_width = max_width.min(panel.pos.x - bar.pos.x - layout::GAP);
        }
        let current = self.editor.get_current_annotation();
        let filled = current.map_or(self.editor.filled, |annotation| annotation.filled);
        let blur_mode = current.map_or(self.editor.blur_mode, |annotation| annotation.blur_mode);
        let color = current.map_or(self.editor.color, |annotation| annotation.color);
        let format = self.get_current_text_format();
        let variant = |variant: Tool, cx: &Context<Self>| {
            let spec = get_tool_spec(variant);
            self.create_button(
                spec.label,
                spec.icon,
                format!("{} ({})", spec.label, spec.key.to_uppercase()),
                EditorAction::Variant(variant),
                cx,
            )
            .selected(tool == variant)
        };
        let toolbar = ui::create_toolbar("tool-options");
        let toolbar = match tool {
            Tool::Rectangle | Tool::Ellipse => toolbar
                .child(variant(Tool::Rectangle, cx))
                .child(variant(Tool::Ellipse, cx))
                .content(Separator::vertical().h_4())
                .child(
                    self.create_button(
                        "outline",
                        if tool == Tool::Ellipse {
                            IconName::Circle
                        } else {
                            IconName::Square
                        },
                        "Outline",
                        EditorAction::Filled(false),
                        cx,
                    )
                    .selected(!filled),
                )
                .child(
                    self.create_button(
                        "filled",
                        Icon::empty().path(if tool == Tool::Ellipse {
                            ui::CIRCLE_FILLED_ICON
                        } else {
                            ui::SQUARE_FILLED_ICON
                        }),
                        "Filled",
                        EditorAction::Filled(true),
                        cx,
                    )
                    .selected(filled),
                ),
            Tool::Arrow | Tool::Line => toolbar
                .child(variant(Tool::Arrow, cx))
                .child(variant(Tool::Line, cx)),
            Tool::Text => [
                (
                    "bold",
                    IconName::Bold,
                    "Bold (⌘B)",
                    TextAttribute::Bold,
                    format.bold,
                ),
                (
                    "italic",
                    IconName::Italic,
                    "Italic (⌘I)",
                    TextAttribute::Italic,
                    format.italic,
                ),
                (
                    "underline",
                    IconName::Underline,
                    "Underline (⌘U)",
                    TextAttribute::Underline,
                    format.underline,
                ),
            ]
            .into_iter()
            .fold(toolbar, |toolbar, (id, icon, label, attribute, active)| {
                toolbar.child(
                    self.create_button(id, icon, label, EditorAction::TextStyle(attribute), cx)
                        .selected(active),
                )
            }),
            Tool::Mosaic => toolbar
                .child(
                    self.create_button(
                        "pixelate",
                        IconName::Grid2x2,
                        "Pixelate",
                        EditorAction::BlurMode(BlurMode::Pixelated),
                        cx,
                    )
                    .selected(blur_mode == BlurMode::Pixelated),
                )
                .child(
                    self.create_button(
                        "gaussian",
                        IconName::Droplet,
                        "Gaussian blur",
                        EditorAction::BlurMode(BlurMode::Gaussian),
                        cx,
                    )
                    .selected(blur_mode == BlurMode::Gaussian),
                )
                .content(div().w(px(112.)).px_1().child(Slider::new(&self.intensity))),
            _ => toolbar,
        };
        let toolbar = if tool == Tool::Mosaic {
            toolbar
        } else {
            toolbar
                .content(Separator::vertical().h_4())
                .content(self.render_size_control(tool, cx))
                .child(self.create_button(
                    "color-picker",
                    IconName::Palette,
                    "More colors",
                    EditorAction::ColorPicker,
                    cx,
                ))
                .content(
                    div().flex().items_center().children(
                        self.get_custom_swatch(color)
                            .into_iter()
                            .chain(COLORS)
                            .map(|swatch| render_swatch(swatch, color.0 == swatch, cx))
                            .collect::<Vec<_>>(),
                    ),
                )
        };
        let toolbar = if self.editor.selected.is_some() {
            toolbar
                .content(Separator::vertical().h_4())
                .child(self.create_button(
                    "delete",
                    IconName::Trash,
                    "Delete (⌫)",
                    EditorAction::Delete,
                    cx,
                ))
        } else {
            toolbar
        };
        return ui::create_toolbar_panel(cx)
            .id("tool-options")
            .absolute()
            .left(px(bar.pos.x))
            .top(px(y))
            .max_w(px(max_width.max(32.0)))
            .cursor(CursorStyle::Arrow)
            .overflow_x_scroll()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(toolbar);
    }

    fn render_size_control(&self, tool: Tool, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.editor.get_current_width().round();
        return div()
            .id("size")
            .min_w(px(36.))
            .px_1()
            .text_xs()
            .text_center()
            .text_color(cx.theme().muted_foreground)
            .child(if tool == Tool::Text {
                format!("{width}pt")
            } else {
                format!("{width}px")
            })
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new("Scroll or press [ and ] to resize")
                    .build(window, cx)
            })
            .on_scroll_wheel(
                cx.listener(move |capture, event: &ScrollWheelEvent, _, cx| {
                    let delta: f32 = event.delta.pixel_delta(px(16.)).y.into();
                    if delta == 0.0 {
                        return;
                    }
                    capture.editor.adjust_size(delta.signum());
                    cx.stop_propagation();
                    cx.notify();
                }),
            );
    }

    fn render_layers_panel(&self, panel: Rect, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let rows = self
            .editor
            .layers
            .iter()
            .enumerate()
            .rev()
            .map(|(index, layer)| {
                let id = layer.id;
                let label: SharedString =
                    format!("{} {}", get_tool_spec(layer.tool).label, id).into();
                let drag_label = label.clone();
                let drop_target = self.layer_drop_target;
                let drag_entity = cx.entity();
                let accent = theme.primary;
                div()
                    .id(("layer", id as usize))
                    .relative()
                    .flex()
                    .items_center()
                    .h(px(LAYER_ROW_HEIGHT))
                    .flex_shrink_0()
                    .px_2()
                    .rounded(theme.radius)
                    .text_sm()
                    .cursor_move()
                    .when(self.editor.selected == Some(index), |row| {
                        row.bg(theme.primary).text_color(theme.primary_foreground)
                    })
                    .when(self.editor.selected != Some(index), |row| {
                        row.hover(|style| style.bg(theme.accent))
                    })
                    .child(label)
                    .on_drag(id, move |dragged: &u64, _, window, cx| {
                        drag_entity.update(cx, |capture, cx| {
                            capture.layer_drop_target = None;
                            capture.select_layer_by_id(*dragged, window, cx);
                        });
                        cx.new(|_| LayerDragPreview(drag_label.clone()))
                    })
                    .on_drag_move(
                        cx.listener(move |capture, event: &DragMoveEvent<u64>, _, cx| {
                            if event.bounds.contains(&event.event.position) {
                                let above = event.event.position.y
                                    < event.bounds.origin.y + event.bounds.size.height / 2.0;
                                let target = Some((id, above));
                                if capture.layer_drop_target != target {
                                    capture.layer_drop_target = target;
                                    cx.notify();
                                }
                            }
                        }),
                    )
                    .drag_over::<u64>(move |style, dragged, _, _| match drop_target {
                        Some((target, above)) if target == id && *dragged != id => {
                            if above {
                                style.border_t_2().border_color(accent)
                            } else {
                                style.border_b_2().border_color(accent)
                            }
                        }
                        _ => style,
                    })
                    .on_drop(cx.listener(move |capture, dragged: &u64, window, cx| {
                        let above = capture
                            .layer_drop_target
                            .take()
                            .filter(|(target, _)| *target == id)
                            .is_none_or(|(_, above)| above);
                        capture.editor.move_layer(*dragged, id, above);
                        capture.select_layer_by_id(*dragged, window, cx);
                    }))
                    .on_click(cx.listener(move |capture, _, window, cx| {
                        capture.editor.commit_draft();
                        capture.select_layer_by_id(id, window, cx);
                    }))
            })
            .collect::<Vec<_>>();
        return ui::panel(cx)
            .id("layers-panel")
            .absolute()
            .left(px(panel.pos.x))
            .top(px(panel.pos.y))
            .w(px(panel.size.x))
            .h(px(panel.size.y))
            .flex()
            .flex_col()
            .cursor(CursorStyle::Arrow)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_drag_move(cx.listener(|capture, event: &DragMoveEvent<u64>, _, cx| {
                if !event.bounds.contains(&event.event.position)
                    && capture.layer_drop_target.take().is_some()
                {
                    cx.notify();
                }
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .flex_shrink_0()
                    .pl_2()
                    .pr_1()
                    .py_0p5()
                    .text_sm()
                    .font_semibold()
                    .child("Layers")
                    .child(
                        self.create_button(
                            "close-layers",
                            IconName::Close,
                            "Close",
                            EditorAction::Layers,
                            cx,
                        )
                        .ghost()
                        .xsmall(),
                    ),
            )
            .child(
                div()
                    .id("layer-list")
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .when(rows.is_empty(), |list| {
                        list.items_center()
                            .justify_center()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("No annotations")
                    })
                    .children(rows),
            );
    }

    fn select_layer_by_id(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.commit_draft();
        self.editor.selected = self.editor.layers.iter().position(|layer| layer.id == id);
        self.editor.tool = Tool::Select;
        window.focus(&self.focus, cx);
        cx.notify();
    }
}

fn render_swatch(swatch: u32, selected: bool, cx: &mut Context<CaptureView>) -> Stateful<Div> {
    let theme = cx.theme();
    return div()
        .id(("color", swatch as usize))
        .flex_shrink_0()
        .p(px(2.))
        .rounded_full()
        .border_2()
        .border_color(if selected {
            theme.foreground
        } else {
            transparent_black()
        })
        .child(
            div()
                .size_4()
                .rounded_full()
                .bg(rgb(swatch))
                .border_1()
                .border_color(theme.border),
        )
        .on_click(cx.listener(move |capture, _, _, cx| {
            cx.stop_propagation();
            capture.editor.set_color(Color(swatch));
            cx.notify();
        }));
}

struct LayerDragPreview(SharedString);

impl Render for LayerDragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        return ui::panel(cx)
            .px_2()
            .py_0p5()
            .text_sm()
            .child(self.0.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::TOOLS;

    #[test]
    fn tool_shortcuts_are_unique() {
        for (index, tool) in TOOLS.iter().enumerate() {
            assert!(!TOOLS[index + 1..].iter().any(|other| other.key == tool.key));
        }
    }
}
