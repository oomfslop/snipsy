use crate::{app, geometry::Rect, platform, raster};
use gpui::{prelude::*, *};
use image::RgbaImage;
use std::sync::Arc;

pub fn open_pin(image: Arc<RgbaImage>, bounds: Option<Rect>, cx: &mut App) {
    let logical_size = bounds
        .map(|bounds| size(px(bounds.size.x), px(bounds.size.y)))
        .unwrap_or_else(|| {
            let scale = (image.width() as f32 / 800.0)
                .max(image.height() as f32 / 600.0)
                .max(1.0);
            size(
                px(image.width() as f32 / scale),
                px(image.height() as f32 / scale),
            )
        });
    let bounds = bounds
        .map(|bounds| Bounds::new(point(px(bounds.pos.x), px(bounds.pos.y)), logical_size))
        .unwrap_or_else(|| Bounds::centered(None, logical_size, cx));
    if let Err(error) = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            kind: WindowKind::Normal,
            show: false,
            focus: false,
            is_resizable: true,
            is_movable: true,
            window_background: WindowBackgroundAppearance::Transparent,
            window_min_size: Some(size(px(40.0), px(40.0))),
            ..Default::default()
        },
        |window, cx| {
            platform::configure_overlay(
                window,
                Rect {
                    pos: glam::vec2(bounds.origin.x.into(), bounds.origin.y.into()),
                    size: glam::vec2(bounds.size.width.into(), bounds.size.height.into()),
                },
                true,
            );
            cx.activate(true);
            window.activate_window();
            cx.new(|cx| {
                let focus = cx.focus_handle();
                window.focus(&focus);
                Pin {
                    preview: raster::create_render_image(&image),
                    image,
                    opacity: 1.0,
                    focus,
                    original_size: logical_size,
                }
            })
        },
    ) {
        app::show_error(error.to_string(), cx);
    }
}

struct Pin {
    image: Arc<RgbaImage>,
    preview: Arc<RenderImage>,
    opacity: f32,
    focus: FocusHandle,
    original_size: Size<Pixels>,
}

impl Pin {
    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" => window.remove_window(),
            "c" if event.keystroke.modifiers.platform => {
                if let Err(error) = app::copy_image(&self.image, cx) {
                    app::show_error(error, cx);
                }
            }
            "s" if event.keystroke.modifiers.platform => app::save_image(self.image.clone(), cx),
            "1" => window.resize(self.original_size),
            "+" | "=" | "-" => {
                let factor = if event.keystroke.key == "-" { 0.9 } else { 1.1 };
                let size = window.viewport_size();
                window.resize(gpui::size(
                    (size.width * factor).max(px(40.0)),
                    (size.height * factor).max(px(40.0)),
                ));
            }
            "[" => {
                self.opacity = (self.opacity - 0.1).max(0.2);
                cx.notify();
            }
            "]" => {
                self.opacity = (self.opacity + 0.1).min(1.0);
                cx.notify();
            }
            _ => {}
        }
    }
}

impl Render for Pin {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("pin")
            .group("pin")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .opacity(self.opacity)
            .cursor(CursorStyle::OpenHand)
            .on_key_down(cx.listener(Self::handle_key))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .on_scroll_wheel(cx.listener(|state, event: &ScrollWheelEvent, window, cx| {
                let delta: f32 = event.delta.pixel_delta(px(16.0)).y.into();
                if event.modifiers.platform {
                    state.opacity = (state.opacity + delta * 0.002).clamp(0.2, 1.0);
                    cx.notify();
                } else {
                    let factor = (1.0 + delta * 0.002).clamp(0.8, 1.2);
                    let size = window.viewport_size();
                    window.resize(gpui::size(
                        (size.width * factor).max(px(40.0)),
                        (size.height * factor).max(px(40.0)),
                    ));
                }
            }))
            .child(
                img(self.preview.clone())
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
            .child(
                div()
                    .absolute()
                    .invisible()
                    .group_hover("pin", |s| s.visible())
                    .top_2()
                    .right_2()
                    .flex()
                    .items_center()
                    .gap_1()
                    .p_1()
                    .rounded_md()
                    .bg(rgba(0x242428ee))
                    .text_color(rgb(0xffffff))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children([("copy", 0), ("save", 1), ("cancel", 2)].into_iter().map(
                        |(icon, action)| {
                            div()
                                .id(icon)
                                .size(px(28.0))
                                .rounded_sm()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0x44444c)))
                                .child(svg().path(icon).size(px(18.0)).text_color(rgb(0xffffff)))
                                .on_click(cx.listener(move |state, _, window, cx| match action {
                                    0 => {
                                        if let Err(error) = app::copy_image(&state.image, cx) {
                                            app::show_error(error, cx);
                                        }
                                    }
                                    1 => app::save_image(state.image.clone(), cx),
                                    _ => window.remove_window(),
                                }))
                        },
                    )),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .size(px(14.0))
                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        cx.stop_propagation();
                        window.start_window_resize(ResizeEdge::BottomRight);
                    }),
            )
    }
}
