use crate::{app, geometry::Rect, platform, raster};
use gpui::{prelude::*, *};
use image::RgbaImage;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

static NEXT_PIN_ID: AtomicU64 = AtomicU64::new(0);

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
        .unwrap_or_else(|| get_logical_size(&image));
    let bounds = bounds
        .map(|bounds| Bounds::new(point(px(bounds.pos.x), px(bounds.pos.y)), logical_size))
        .unwrap_or_else(|| Bounds::centered(None, logical_size, cx));
    open_pin_window(image, bounds, logical_size, None, 1.0, text, None, true, cx);
}

fn open_pin_window(
    image: Arc<RgbaImage>,
    bounds: Bounds<Pixels>,
    original_size: Size<Pixels>,
    thumbnail_size: Option<Size<Pixels>>,
    opacity: f32,
    text: Option<Arc<str>>,
    id: Option<u128>,
    activate: bool,
    cx: &mut App,
) {
    let id = id.unwrap_or_else(create_pin_id);
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
            if activate {
                cx.activate(true);
                window.activate_window();
            } else {
                platform::set_window_visible(window, true);
            }
            cx.new(|cx| {
                let focus = cx.focus_handle();
                if activate {
                    window.focus(&focus);
                }
                cx.observe_window_bounds(window, |state: &mut Pin, window, cx| {
                    state.schedule_backup(window, cx)
                })
                .detach();
                Pin {
                    preview: raster::create_render_image(&image),
                    image,
                    opacity,
                    focus,
                    original_size,
                    thumbnail_size,
                    click_through: false,
                    text,
                    id,
                    backup_task: None,
                    replace_task: None,
                }
            })
        },
    ) {
        Ok(handle) => {
            cx.global_mut::<app::AppState>().pins.push(handle);
            let _ = handle.update(cx, |state, window, cx| state.schedule_backup(window, cx));
        }
        Err(error) => app::show_error(error.to_string(), cx),
    }
}

fn create_pin_id() -> u128 {
    return (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        << 32)
        | NEXT_PIN_ID.fetch_add(1, Ordering::Relaxed) as u128;
}

pub struct ClosedPin {
    id: u128,
    image: Arc<RgbaImage>,
    bounds: Bounds<Pixels>,
    opacity: f32,
    original_size: Size<Pixels>,
    thumbnail_size: Option<Size<Pixels>>,
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
        pin.thumbnail_size,
        pin.opacity,
        pin.text,
        Some(pin.id),
        true,
        cx,
    );
}

pub struct PinBackup {
    pub id: u128,
    pub image: Arc<RgbaImage>,
    pub bounds: Bounds<Pixels>,
    pub opacity: f32,
    pub original_size: Size<Pixels>,
    pub thumbnail_size: Option<Size<Pixels>>,
    pub text: Option<Arc<str>>,
}

pub enum PinBackupCommand {
    Save(PinBackup),
    Remove(u128),
}

pub fn restore_backup(pin: PinBackup, cx: &mut App) {
    open_pin_window(
        pin.image,
        pin.bounds,
        pin.original_size,
        pin.thumbnail_size,
        pin.opacity,
        pin.text,
        Some(pin.id),
        false,
        cx,
    );
}

pub struct Pin {
    image: Arc<RgbaImage>,
    preview: Arc<RenderImage>,
    opacity: f32,
    focus: FocusHandle,
    original_size: Size<Pixels>,
    thumbnail_size: Option<Size<Pixels>>,
    click_through: bool,
    text: Option<Arc<str>>,
    id: u128,
    backup_task: Option<Task<()>>,
    replace_task: Option<Task<()>>,
}

impl Pin {
    fn schedule_backup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backup_task = Some(cx.spawn_in(window, async move |this, cx| {
            Timer::after(Duration::from_millis(200)).await;
            let _ = this.update_in(cx, |state, window, cx| {
                app::persist_pin(
                    PinBackupCommand::Save(PinBackup {
                        id: state.id,
                        image: state.image.clone(),
                        bounds: window.bounds(),
                        opacity: state.opacity,
                        original_size: state.original_size,
                        thumbnail_size: state.thumbnail_size,
                        text: state.text.clone(),
                    }),
                    cx,
                );
            });
        }));
    }

    fn replace_from_clipboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let load = match app::load_clipboard_content(cx) {
            Ok(load) => load,
            Err(error) => {
                app::show_error(error, cx);
                return;
            }
        };
        self.replace_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |state, window, cx| match result {
                Ok(content) => {
                    let previous = std::mem::replace(
                        &mut state.preview,
                        raster::create_render_image(&content.image),
                    );
                    cx.drop_image(previous, Some(window));
                    state.original_size = get_logical_size(&content.image);
                    state.thumbnail_size = None;
                    state.image = content.image;
                    state.text = content.text.map(Into::into);
                    state.schedule_backup(window, cx);
                    cx.notify();
                }
                Err(error) => app::show_error(error, cx),
            });
        }));
    }

    fn close(&self, window: &mut Window, cx: &mut Context<Self>) {
        app::persist_pin(PinBackupCommand::Remove(self.id), cx);
        let state = cx.global_mut::<app::AppState>();
        state.closed_pins.push(ClosedPin {
            id: self.id,
            image: self.image.clone(),
            bounds: window.bounds(),
            opacity: self.opacity,
            original_size: self.original_size,
            thumbnail_size: self.thumbnail_size,
            text: self.text.clone(),
            clipboard_change_count: platform::get_clipboard_change_count(),
        });
        if state.closed_pins.len() > 10 {
            state.closed_pins.remove(0);
        }
        window.remove_window();
    }

    fn destroy(&self, window: &mut Window, cx: &mut Context<Self>) {
        app::persist_pin(PinBackupCommand::Remove(self.id), cx);
        window.remove_window();
    }

    fn toggle_thumbnail(&mut self, window: &mut Window) {
        if let Some(size) = self.thumbnail_size.take() {
            window.resize(size);
            return;
        }
        let viewport = window.viewport_size();
        let target = create_thumbnail_size(viewport.width.into(), viewport.height.into());
        self.thumbnail_size = Some(viewport);
        window.resize(size(px(target.0), px(target.1)));
    }

    fn reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.opacity = 1.0;
        self.thumbnail_size = None;
        window.resize(self.original_size);
        self.schedule_backup(window, cx);
        cx.notify();
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
            "escape" if event.keystroke.modifiers.shift => self.destroy(window, cx),
            "escape" => self.close(window, cx),
            "w" if event.keystroke.modifiers.platform => self.close(window, cx),
            "v" if event.keystroke.modifiers.platform => self.replace_from_clipboard(window, cx),
            "c" if event.keystroke.modifiers.alt => {
                if let Some((_, _, color)) =
                    sample_pin_pixel(&self.image, window.viewport_size(), window.mouse_position())
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(create_color_label(
                        color,
                        event.keystroke.modifiers.shift,
                    )));
                }
            }
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
            "s" if event.keystroke.modifiers.platform && event.keystroke.modifiers.shift => {
                app::quick_save_image(self.image.clone(), None, cx)
            }
            "s" if event.keystroke.modifiers.platform => {
                app::save_image(self.image.clone(), None, cx)
            }
            "0" => self.reset(window, cx),
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
                    self.thumbnail_size = self
                        .thumbnail_size
                        .map(|size| gpui::size(size.height, size.width));
                }
                self.schedule_backup(window, cx);
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
                    self.schedule_backup(window, cx);
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
                self.schedule_backup(window, cx);
                cx.notify();
            }
            "]" => {
                self.opacity = (self.opacity + 0.1).min(1.0);
                self.schedule_backup(window, cx);
                cx.notify();
            }
            "left" | "right" | "up" | "down" => {
                let step = if event.keystroke.modifiers.shift {
                    10.0
                } else {
                    1.0
                };
                let (x, y) = match event.keystroke.key.as_str() {
                    "left" => (-step, 0.0),
                    "right" => (step, 0.0),
                    "up" => (0.0, -step),
                    _ => (0.0, step),
                };
                platform::move_window(window, x, y);
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

fn get_logical_size(image: &RgbaImage) -> Size<Pixels> {
    let scale = (image.width() as f32 / 800.0)
        .max(image.height() as f32 / 600.0)
        .max(1.0);
    return size(
        px(image.width() as f32 / scale),
        px(image.height() as f32 / scale),
    );
}

fn create_thumbnail_size(width: f32, height: f32) -> (f32, f32) {
    let scale = (width / 180.0)
        .max(height / 120.0)
        .max(1.0)
        .min((width / 40.0).min(height / 40.0).max(1.0));
    return (width / scale, height / scale);
}

fn sample_pin_pixel(
    image: &RgbaImage,
    viewport: Size<Pixels>,
    pointer: Point<Pixels>,
) -> Option<(u32, u32, crate::geometry::Color)> {
    let viewport = glam::vec2(viewport.width.into(), viewport.height.into());
    let pointer = glam::vec2(pointer.x.into(), pointer.y.into());
    if image.width() == 0 || image.height() == 0 || viewport.min_element() <= 0.0 {
        return None;
    }
    let image_size = glam::vec2(image.width() as f32, image.height() as f32);
    let scale = (viewport.x / image_size.x).min(viewport.y / image_size.y);
    let displayed = image_size * scale;
    let point = pointer - (viewport - displayed) / 2.0;
    if point.cmplt(glam::Vec2::ZERO).any() || point.cmpge(displayed).any() {
        return None;
    }
    let x = (point.x / scale).floor() as u32;
    let y = (point.y / scale).floor() as u32;
    let pixel = image.get_pixel(x, y);
    return Some((
        x,
        y,
        crate::geometry::Color::from_rgb(pixel[0], pixel[1], pixel[2]),
    ));
}

fn create_color_label(color: crate::geometry::Color, rgb_format: bool) -> String {
    return if rgb_format {
        format!(
            "{}, {}, {}",
            color.0 >> 16 & 255,
            color.0 >> 8 & 255,
            color.0 & 255
        )
    } else {
        format!("#{:06x}", color.0)
    };
}

impl Render for Pin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pointer = window.mouse_position();
        let viewport = window.viewport_size();
        let sample = window
            .modifiers()
            .alt
            .then(|| sample_pin_pixel(&self.image, viewport, pointer))
            .flatten();
        div()
            .id("pin")
            .group("pin")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .opacity(self.opacity)
            .cursor(CursorStyle::OpenHand)
            .on_key_down(cx.listener(Self::handle_key))
            .on_modifiers_changed(cx.listener(|_, _, _, cx| cx.notify()))
            .on_mouse_move(cx.listener(|_, _, window, cx| {
                if window.modifiers().alt {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|state, event: &MouseDownEvent, window, cx| {
                    if event.click_count >= 2 {
                        if event.modifiers.shift {
                            state.toggle_thumbnail(window);
                        } else {
                            state.close(window, cx);
                        }
                    } else {
                        window.start_window_move();
                    }
                }),
            )
            .on_drop(cx.listener(|_, paths: &ExternalPaths, _, cx| {
                app::open_paths(paths.paths().to_vec(), cx)
            }))
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|state, _, window, cx| {
                    cx.stop_propagation();
                    state.reset(window, cx);
                }),
            )
            .on_scroll_wheel(cx.listener(|state, event: &ScrollWheelEvent, window, cx| {
                let delta: f32 = event.delta.pixel_delta(px(16.0)).y.into();
                if event.modifiers.platform {
                    state.opacity = (state.opacity + delta * 0.002).clamp(0.2, 1.0);
                    state.schedule_backup(window, cx);
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
            .when_some(sample, |root, (x, y, color)| {
                let left = (f32::from(pointer.x) + 12.0)
                    .clamp(4.0, (f32::from(viewport.width) - 128.0).max(4.0));
                let top = (f32::from(pointer.y) + 12.0)
                    .clamp(4.0, (f32::from(viewport.height) - 48.0).max(4.0));
                root.child(
                    div()
                        .absolute()
                        .left(px(left))
                        .top(px(top))
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .bg(rgba(0x17171bee))
                        .text_color(rgb(0xffffff))
                        .font_family("Menlo")
                        .text_size(px(11.0))
                        .child(format!(
                            "({x}, {y})  {}",
                            create_color_label(color, window.modifiers().shift)
                        )),
                )
            })
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
                                    1 => app::save_image(state.image.clone(), None, cx),
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
    use super::{
        create_pin_id, create_thumbnail_size, get_logical_size, sample_pin_pixel, transform_image,
    };
    use gpui::{point, px, size};
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

    #[test]
    fn thumbnails_fit_without_upscaling() {
        assert_eq!(create_thumbnail_size(90.0, 60.0), (90.0, 60.0));
        assert_eq!(create_thumbnail_size(360.0, 120.0), (180.0, 60.0));
        assert_eq!(create_thumbnail_size(100.0, 400.0), (40.0, 160.0));
    }

    #[test]
    fn pin_sampling_accounts_for_contain_letterboxing() {
        let image = RgbaImage::from_fn(2, 1, |x, _| Rgba([x as u8, 2, 3, 255]));
        assert_eq!(
            sample_pin_pixel(
                &image,
                size(px(100.0), px(100.0)),
                point(px(75.0), px(50.0))
            )
            .unwrap()
            .0,
            1
        );
        assert!(
            sample_pin_pixel(
                &image,
                size(px(100.0), px(100.0)),
                point(px(50.0), px(10.0))
            )
            .is_none()
        );
    }

    #[test]
    fn pin_ids_are_unique_without_clock_progress() {
        assert_ne!(create_pin_id(), create_pin_id());
    }

    #[test]
    fn logical_pin_size_caps_large_images_without_upscaling() {
        let small = get_logical_size(&RgbaImage::new(100, 50));
        assert_eq!(
            (f32::from(small.width), f32::from(small.height)),
            (100.0, 50.0)
        );
        let large = get_logical_size(&RgbaImage::new(1600, 1200));
        assert_eq!(
            (f32::from(large.width), f32::from(large.height)),
            (800.0, 600.0)
        );
    }
}
