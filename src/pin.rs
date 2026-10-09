use crate::{app, geometry::Rect, platform, raster};
use gpui::{prelude::*, *};
use image::RgbaImage;
use std::sync::Arc;

pub fn open_pin(image: Arc<RgbaImage>, bounds: Option<Rect>, cx: &mut App) {
    open_pin_with_text(image, bounds, None, cx);
}

pub fn open_text_pin(image: Arc<RgbaImage>, text: String, cx: &mut App) {
    open_pin_with_text(image, None, Some(text.into()), cx);
}

fn open_pin_with_text(
    image: Arc<RgbaImage>,
    bounds: Option<Rect>,
    text: Option<Arc<str>>,
    cx: &mut App,
) {
    if cx.global::<app::AppState>().pins_hidden {
        app::set_pins_visible(true, cx);
    }
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
    open_pin_window(image, bounds, logical_size, 1.0, text, cx);
}

fn open_pin_window(
    image: Arc<RgbaImage>,
    bounds: Bounds<Pixels>,
    original_size: Size<Pixels>,
    opacity: f32,
    text: Option<Arc<str>>,
    cx: &mut App,
) {
    match cx.open_window(
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
                    opacity,
                    focus,
                    original_size,
                    click_through: false,
                    text,
                }
            })
        },
    ) {
        Ok(handle) => cx.global_mut::<app::AppState>().pins.push(handle),
        Err(error) => app::show_error(error.to_string(), cx),
    }
}

pub struct ClosedPin {
    image: Arc<RgbaImage>,
    bounds: Bounds<Pixels>,
    opacity: f32,
    original_size: Size<Pixels>,
    text: Option<Arc<str>>,
    clipboard_change_count: i64,
}

impl ClosedPin {
    pub fn clipboard_change_count(&self) -> i64 {
        return self.clipboard_change_count;
    }
}

pub fn restore_pin(pin: ClosedPin, cx: &mut App) {
    app::set_pins_visible(true, cx);
    open_pin_window(
        pin.image,
        pin.bounds,
        pin.original_size,
        pin.opacity,
        pin.text,
        cx,
    );
}

pub struct Pin {
    image: Arc<RgbaImage>,
    preview: Arc<RenderImage>,
    opacity: f32,
    focus: FocusHandle,
    original_size: Size<Pixels>,
    click_through: bool,
    text: Option<Arc<str>>,
}

impl Pin {
    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.global_mut::<app::AppState>();
        state.closed_pins.push(ClosedPin {
            image: self.image.clone(),
            bounds: window.bounds(),
            opacity: self.opacity,
            original_size: self.original_size,
            text: self.text.clone(),
            clipboard_change_count: platform::get_clipboard_change_count(),
        });
        if state.closed_pins.len() > 10 {
            state.closed_pins.remove(0);
        }
        window.remove_window();
    }

    pub fn toggle_click_through(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.click_through = !self.click_through;
        platform::set_window_click_through(window, self.click_through);
        cx.notify();
    }

    pub fn disable_click_through(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.click_through {
            self.toggle_click_through(window, cx);
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "escape" if event.keystroke.modifiers.shift => window.remove_window(),
            "escape" => self.close(window, cx),
            "w" if event.keystroke.modifiers.platform => self.close(window, cx),
            "c" if event.keystroke.modifiers.platform && event.keystroke.modifiers.shift => {
                if let Some(text) = &self.text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                }
            }
            "c" if event.keystroke.modifiers.platform => {
                if let Err(error) = app::copy_image(&self.image, cx) {
                    app::show_error(error, cx);
                }
            }
            "s" if event.keystroke.modifiers.platform => app::save_image(self.image.clone(), cx),
            "0" => window.resize(self.original_size),
            "1" | "2" | "3" | "4" => {
                let swap_dimensions = matches!(event.keystroke.key.as_str(), "1" | "2");
                let image = transform_image(&self.image, &event.keystroke.key);
                let previous =
                    std::mem::replace(&mut self.preview, raster::create_render_image(&image));
                self.image = Arc::new(image);
                cx.drop_image(previous, Some(window));
                if swap_dimensions {
                    let viewport = window.viewport_size();
                    window.resize(size(viewport.height, viewport.width));
                    self.original_size = size(self.original_size.height, self.original_size.width);
                }
                cx.notify();
            }
            "+" | "=" | "-" => {
                if event.keystroke.modifiers.platform {
                    self.opacity = (self.opacity
                        + if event.keystroke.key == "-" {
                            -0.1
                        } else {
                            0.1
                        })
                    .clamp(0.2, 1.0);
                    cx.notify();
                } else {
                    let factor = if event.keystroke.key == "-" { 0.9 } else { 1.1 };
                    let viewport = window.viewport_size();
                    window.resize(size(
                        (viewport.width * factor).max(px(40.0)),
                        (viewport.height * factor).max(px(40.0)),
                    ));
                }
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

fn transform_image(image: &RgbaImage, key: &str) -> RgbaImage {
    return match key {
        "1" => image::imageops::rotate270(image),
        "2" => image::imageops::rotate90(image),
        "3" => image::imageops::flip_horizontal(image),
        "4" => image::imageops::flip_vertical(image),
        _ => unreachable!(),
    };
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|state, event: &MouseDownEvent, window, cx| {
                    if event.click_count >= 2 {
                        state.close(window, cx);
                    } else {
                        window.start_window_move();
                    }
                }),
            )
            .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                app::open_paths(paths.paths().to_vec(), cx)
            }))
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
                    .top_1()
                    .right_1()
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .p(px(3.0))
                    .rounded_sm()
                    .bg(rgba(0x242428ee))
                    .text_color(rgb(0xffffff))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children([("copy", 0), ("save", 1), ("cancel", 2)].into_iter().map(
                        |(icon, action)| {
                            div()
                                .id(icon)
                                .size(px(24.0))
                                .rounded_sm()
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .hover(|s| s.bg(rgb(0x44444c)))
                                .child(svg().path(icon).size(px(16.0)).text_color(rgb(0xffffff)))
                                .on_click(cx.listener(move |state, _, window, cx| match action {
                                    0 => {
                                        if let Err(error) = app::copy_image(&state.image, cx) {
                                            app::show_error(error, cx);
                                        }
                                    }
                                    1 => app::save_image(state.image.clone(), cx),
                                    _ => state.close(window, cx),
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

#[cfg(test)]
mod tests {
    use super::transform_image;
    use image::{Rgba, RgbaImage};

    #[test]
    fn pin_transforms_are_reversible() {
        let image = RgbaImage::from_fn(2, 3, |x, y| Rgba([x as u8, y as u8, 0, 255]));

        let rotated = transform_image(&image, "1");
        assert_eq!(rotated.dimensions(), (3, 2));
        assert_eq!(transform_image(&rotated, "2"), image);
        assert_eq!(transform_image(&transform_image(&image, "3"), "3"), image);
        assert_eq!(transform_image(&transform_image(&image, "4"), "4"), image);
    }
}
