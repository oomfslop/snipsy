mod backup;

use crate::{
    app::{
        self,
        config::{self, Config},
        content::{self, PinContent},
        export, storage,
    },
    capture,
    geometry::Rect,
    platform, raster,
    ui::{self, View, magnifier},
};
use anyhow::Result;
use backup::BackupCommand;
use glam::{Vec2, Vec2Swizzles as _, vec2};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        native_menu::NativeMenu,
    },
    prelude::*,
    *,
};
use image::{Frame, RgbaImage};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

pub const MIN_SIZE: f32 = 40.0;
const CLOSED_LIMIT: usize = 10;

#[derive(Clone)]
pub struct PinData {
    pub id: u128,
    pub content: PinContent,
    pub bounds: Rect,
    pub opacity: f32,
    pub original_size: Vec2,
    pub thumbnail_size: Option<Vec2>,
}

struct ClosedPin {
    data: PinData,
    clipboard_change_count: i64,
}

#[derive(Default)]
struct Pins {
    views: Vec<View<Pin>>,
    closed: Vec<ClosedPin>,
    hidden: bool,
    backup: Option<async_channel::Sender<BackupCommand>>,
}

impl Global for Pins {}

actions!(
    pin,
    [
        Copy,
        CopyText,
        CopyColor,
        CopyColorRgb,
        Save,
        QuickSave,
        Print,
        Edit,
        Paste,
        ToggleThumbnail,
        ToggleFrame,
        Reset,
        Close,
        Destroy,
        ZoomIn,
        ZoomOut,
        MoreOpaque,
        LessOpaque,
        RotateClockwise,
        RotateCounterclockwise,
        FlipHorizontal,
        FlipVertical,
        ToggleAnimation,
        PreviousFrame,
        NextFrame,
    ]
);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = pin, no_json)]
struct Nudge(Vec2);

pub fn init(data_dir: &Path, cx: &mut App) {
    let context = Some("Pin");
    let still = Some("Pin && !animated");
    let animated = Some("Pin && animated");
    let mut bindings = vec![
        KeyBinding::new("escape", Close, context),
        KeyBinding::new("cmd-w", Close, context),
        KeyBinding::new("shift-escape", Destroy, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-shift-c", CopyText, context),
        KeyBinding::new("alt-c", CopyColor, context),
        KeyBinding::new("alt-shift-c", CopyColorRgb, context),
        KeyBinding::new("cmd-s", Save, context),
        KeyBinding::new("cmd-shift-s", QuickSave, context),
        KeyBinding::new("cmd-p", Print, context),
        KeyBinding::new("cmd-v", Paste, context),
        KeyBinding::new("space", Edit, context),
        KeyBinding::new("0", Reset, context),
        KeyBinding::new("=", ZoomIn, context),
        KeyBinding::new("+", ZoomIn, context),
        KeyBinding::new("shift-=", ZoomIn, context),
        KeyBinding::new("-", ZoomOut, context),
        KeyBinding::new("]", MoreOpaque, context),
        KeyBinding::new("cmd-=", MoreOpaque, context),
        KeyBinding::new("[", LessOpaque, context),
        KeyBinding::new("cmd--", LessOpaque, context),
        KeyBinding::new("1", RotateClockwise, still),
        KeyBinding::new("2", RotateCounterclockwise, still),
        KeyBinding::new("3", FlipHorizontal, context),
        KeyBinding::new("4", FlipVertical, context),
        KeyBinding::new("p", ToggleAnimation, animated),
        KeyBinding::new("1", PreviousFrame, animated),
        KeyBinding::new("2", NextFrame, animated),
    ];
    for (key, direction) in [
        ("left", vec2(-1., 0.)),
        ("right", vec2(1., 0.)),
        ("up", vec2(0., -1.)),
        ("down", vec2(0., 1.)),
    ] {
        bindings.push(KeyBinding::new(key, Nudge(direction), context));
        bindings.push(KeyBinding::new(
            &format!("shift-{key}"),
            Nudge(direction * 10.),
            context,
        ));
    }
    cx.bind_keys(bindings);
    cx.set_global(Pins::default());
    cx.on_window_closed(|cx, id| {
        cx.global_mut::<Pins>()
            .views
            .retain(|view| view.window.window_id() != id);
    })
    .detach();
    let directory = data_dir.join("pins");
    let (sender, receiver) = async_channel::unbounded();
    let (loaded, load) = async_channel::bounded(1);
    let pins = cx.global_mut::<Pins>();
    pins.backup = Some(sender);
    cx.background_executor()
        .spawn(async move {
            let pins = backup::load(&directory);
            let written = pins
                .iter()
                .flatten()
                .map(|pin| (pin.id, pin.content.clone()))
                .collect();
            let _ = loaded.try_send(pins);
            backup::run_worker(&directory, receiver, written).await;
        })
        .detach();
    cx.spawn(async move |cx| {
        let pins = load.recv().await.unwrap_or_else(|error| Err(error.into()));
        cx.update(|cx| match pins {
            Ok(pins) => {
                for pin in pins {
                    open(pin, false, cx);
                }
            }
            Err(error) => eprintln!("could not restore pins: {error:#}"),
        });
    })
    .detach();
}

fn send_backup(command: BackupCommand, cx: &App) {
    if let Some(backup) = &cx.global::<Pins>().backup {
        let _ = backup.try_send(command);
    }
}

fn create_id() -> u128 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    return storage::get_timestamp_id() << 32 | COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
}

fn get_logical_size(image: &RgbaImage) -> Vec2 {
    let size = vec2(image.width() as f32, image.height() as f32);
    return size / (size / vec2(800., 600.)).max_element().max(1.0);
}

pub fn open_image(image: Arc<RgbaImage>, bounds: Option<Rect>, cx: &mut App) {
    open_content(PinContent::from_image(image), bounds, cx);
}

fn open_content(content: PinContent, bounds: Option<Rect>, cx: &mut App) {
    let bounds = bounds.unwrap_or_else(|| {
        let size = get_logical_size(&content.image);
        ui::to_rect(Bounds::centered(
            None,
            gpui_kit::size(px(size.x), px(size.y)),
            cx,
        ))
    });
    open(
        PinData {
            id: create_id(),
            content,
            bounds,
            opacity: 1.0,
            original_size: bounds.size,
            thumbnail_size: None,
        },
        true,
        cx,
    );
}

pub fn open_text(text: String, cx: &mut App) -> Result<()> {
    let fonts = cx.global::<app::AppState>().fonts.clone();
    open_content(content::load_text(text, &fonts)?, None, cx);
    return Ok(());
}

pub fn open_paths(paths: Vec<PathBuf>, cx: &mut App) {
    for path in paths {
        let load = content::load_path_in_background(&path, cx);
        cx.spawn(async move |cx| {
            let result = load.await;
            cx.update(|cx| match result {
                Ok(content) => open_content(content, None, cx),
                Err(error) => app::show_error(error),
            });
        })
        .detach();
    }
}

pub fn paste(cx: &mut App) -> Result<()> {
    let change_count = platform::get_clipboard_change_count();
    let pins = cx.global_mut::<Pins>();
    if let Some(closed) = pins
        .closed
        .pop_if(|closed| closed.clipboard_change_count == change_count)
    {
        open(closed.data, true, cx);
        return Ok(());
    }
    let paths = platform::get_clipboard_paths();
    if !paths.is_empty() {
        open_paths(paths, cx);
        return Ok(());
    }
    let load = content::load_clipboard(cx)?;
    cx.spawn(async move |cx| {
        let result = load.await;
        cx.update(|cx| match result {
            Ok(content) => open_content(content, None, cx),
            Err(error) => app::show_error(error),
        });
    })
    .detach();
    return Ok(());
}

fn open(data: PinData, activate: bool, cx: &mut App) {
    if activate && cx.global::<Pins>().hidden {
        cx.global_mut::<Pins>().hidden = false;
        refresh_visibility(cx);
    }
    let view = ui::open_overlay(data.bounds, true, cx, |window, cx| {
        cx.new(|cx| Pin::new(data, window, cx))
    });
    let view = match view {
        Ok(view) => view,
        Err(error) => return app::show_error(error),
    };
    let _ = view.update(cx, |pin, window, cx| {
        pin.schedule_backup(window, cx);
        platform::set_window_shadow(window, cx.global::<Config>().pin_frame);
        if pin.get_is_visible(cx) {
            platform::set_window_visible(window, true);
        }
        if activate {
            platform::make_key_window(window);
            cx.activate(true);
            window.activate_window();
            window.focus(&pin.focus, cx);
        }
    });
    cx.global_mut::<Pins>().views.push(view);
}

fn update_pins(cx: &mut App, mut update: impl FnMut(&mut Pin, &mut Window, &mut Context<Pin>)) {
    let views = cx.global::<Pins>().views.clone();
    for view in views {
        let _ = view.update(cx, &mut update);
    }
}

pub fn refresh_visibility(cx: &mut App) {
    update_pins(cx, |pin, window, cx| {
        platform::set_window_visible(window, pin.get_is_visible(cx))
    });
}

fn toggle_frame(cx: &mut App) {
    if let Err(error) = config::update(cx, |config| config.pin_frame = !config.pin_frame) {
        return app::show_error(error);
    }
    let enabled = cx.global::<Config>().pin_frame;
    update_pins(cx, |_, window, cx| {
        platform::set_window_shadow(window, enabled);
        cx.notify();
    });
}

pub fn toggle_hidden(cx: &mut App) {
    let pins = cx.global_mut::<Pins>();
    pins.hidden = !pins.hidden;
    refresh_visibility(cx);
}

pub fn toggle_click_through(cx: &mut App) {
    let mut found = false;
    update_pins(cx, |pin, window, cx| {
        if !found && platform::is_window_under_pointer(window) {
            found = true;
            pin.set_click_through(!pin.click_through, window, cx);
        }
    });
    if !found {
        update_pins(cx, |pin, window, cx| {
            pin.set_click_through(false, window, cx)
        });
    }
}

pub struct Pin {
    data: PinData,
    preview: Arc<RenderImage>,
    focus: FocusHandle,
    click_through: bool,
    paused_frame: Option<(usize, Arc<RgbaImage>)>,
    backup_task: Option<Task<()>>,
    replace_task: Option<Task<()>>,
    zoom_badge_task: Option<Task<()>>,
}

fn create_preview(content: &PinContent) -> Arc<RenderImage> {
    return match &content.frames {
        Some(frames) => raster::create_render_frames(frames),
        None => raster::create_render_image(&content.image),
    };
}

impl Pin {
    fn new(data: PinData, window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe_window_bounds(window, |pin, window, cx| pin.schedule_backup(window, cx))
            .detach();
        return Self {
            preview: create_preview(&data.content),
            data,
            focus: cx.focus_handle(),
            click_through: false,
            paused_frame: None,
            backup_task: None,
            replace_task: None,
            zoom_badge_task: None,
        };
    }

    pub fn get_is_visible(&self, cx: &App) -> bool {
        return !cx.global::<Pins>().hidden;
    }

    fn get_image(&self) -> &Arc<RgbaImage> {
        return self
            .paused_frame
            .as_ref()
            .map_or(&self.data.content.image, |(_, image)| image);
    }

    fn get_export_content(&self) -> PinContent {
        let mut content = self.data.content.clone();
        if let Some((_, image)) = &self.paused_frame {
            content.image = image.clone();
            content.frames = None;
        }
        return content;
    }

    fn get_snapshot(&self, window: &Window) -> PinData {
        return PinData {
            bounds: ui::to_rect(window.bounds()),
            ..self.data.clone()
        };
    }

    fn schedule_backup(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.backup_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let _ = this.update_in(cx, |pin, window, cx| {
                send_backup(BackupCommand::Save(pin.get_snapshot(window)), cx)
            });
        }));
    }

    fn set_preview(
        &mut self,
        preview: Arc<RenderImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous = std::mem::replace(&mut self.preview, preview);
        cx.drop_image(previous, Some(window));
        cx.notify();
    }

    fn set_content(&mut self, content: PinContent, window: &mut Window, cx: &mut Context<Self>) {
        let paused_frame = self.paused_frame.take().map(|(index, _)| index);
        self.data.content = content;
        if let Some(index) = paused_frame
            && self.data.content.frames.is_some()
        {
            self.show_frame(index, window, cx);
        } else {
            self.set_preview(create_preview(&self.data.content), window, cx);
        }
        self.schedule_backup(window, cx);
    }

    pub fn replace_content(
        &mut self,
        content: PinContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paused_frame = None;
        self.data.original_size = get_logical_size(&content.image);
        self.data.thumbnail_size = None;
        self.set_content(content, window, cx);
    }

    fn load_replacement(
        &mut self,
        load: Task<Result<PinContent>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = load.await;
            let _ = this.update_in(cx, |pin, window, cx| match result {
                Ok(content) => pin.replace_content(content, window, cx),
                Err(error) => app::show_error(error),
            });
        }));
    }

    fn set_opacity(&mut self, opacity: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.data.opacity = opacity.clamp(0.2, 1.0);
        self.schedule_backup(window, cx);
        cx.notify();
    }

    fn scale(&mut self, factor: f32, window: &mut Window, cx: &mut Context<Self>) {
        platform::set_window_bounds(
            window,
            get_zoomed_bounds(ui::to_rect(window.bounds()), factor),
        );
        self.zoom_badge_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let _ = this.update(cx, |pin, cx| {
                pin.zoom_badge_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn set_click_through(&mut self, enabled: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.click_through = enabled;
        platform::set_window_click_through(window, enabled);
        cx.notify();
    }

    fn transform(
        &mut self,
        transform: fn(&RgbaImage) -> RgbaImage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = &self.data.content;
        let frames = content.frames.as_ref().map(|frames| {
            Arc::new(
                frames
                    .iter()
                    .map(|frame| Frame::from_parts(transform(frame.buffer()), 0, 0, frame.delay()))
                    .collect::<Vec<_>>(),
            )
        });
        let image = Arc::new(transform(&content.image));
        let swapped = image.dimensions() != content.image.dimensions();
        self.set_content(
            PinContent {
                image,
                frames,
                text: content.text.clone(),
            },
            window,
            cx,
        );
        if swapped {
            let size = window.viewport_size();
            window.resize(gpui_kit::size(size.height, size.width));
            self.data.original_size = self.data.original_size.yx();
            self.data.thumbnail_size = self.data.thumbnail_size.map(|size| size.yx());
        }
    }

    fn show_frame(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(frames) = &self.data.content.frames else {
            return;
        };
        let index = index % frames.len();
        let preview = raster::create_render_frames(std::slice::from_ref(&frames[index]));
        self.paused_frame = Some((index, Arc::new(frames[index].buffer().clone())));
        self.set_preview(preview, window, cx);
    }

    fn resume_animation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.paused_frame.take().is_some() {
            self.set_preview(create_preview(&self.data.content), window, cx);
        }
    }

    fn adjust_animation_speed(&mut self, factor: f64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(frames) = &self.data.content.frames else {
            return;
        };
        let frames = frames
            .iter()
            .map(|frame| scale_frame_delay(frame, factor))
            .collect();
        self.set_content(
            PinContent {
                frames: Some(Arc::new(frames)),
                ..self.data.content.clone()
            },
            window,
            cx,
        );
    }

    fn get_sample(&self, window: &Window) -> Option<magnifier::Sample> {
        let image = self.get_image();
        let viewport = window.viewport_size();
        let pointer = window.mouse_position();
        let (x, y) = get_image_pixel(
            vec2(image.width() as f32, image.height() as f32),
            vec2(viewport.width.into(), viewport.height.into()),
            vec2(pointer.x.into(), pointer.y.into()),
        )?;
        return magnifier::sample_pixel(image, x, y);
    }

    fn copy_color(&mut self, rgb: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(sample) = self.get_sample(window) {
            cx.write_to_clipboard(ClipboardItem::new_string(sample.color.format(rgb)));
        }
    }

    fn close(&mut self, _: &Close, window: &mut Window, cx: &mut Context<Self>) {
        let data = self.get_snapshot(window);
        send_backup(BackupCommand::Remove(data.id), cx);
        let closed = &mut cx.global_mut::<Pins>().closed;
        closed.push(ClosedPin {
            data,
            clipboard_change_count: platform::get_clipboard_change_count(),
        });
        let overflow = closed.len().saturating_sub(CLOSED_LIMIT);
        closed.drain(..overflow);
        window.remove_window();
    }

    fn destroy(&mut self, _: &Destroy, window: &mut Window, cx: &mut Context<Self>) {
        send_backup(BackupCommand::Remove(self.data.id), cx);
        window.remove_window();
    }

    fn toggle_thumbnail(
        &mut self,
        _: &ToggleThumbnail,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(size) = self.data.thumbnail_size.take() {
            window.resize(gpui_kit::size(px(size.x), px(size.y)));
        } else {
            let viewport = window.viewport_size();
            let size = vec2(viewport.width.into(), viewport.height.into());
            let thumbnail = get_thumbnail_size(size);
            self.data.thumbnail_size = Some(size);
            window.resize(gpui_kit::size(px(thumbnail.x), px(thumbnail.y)));
        }
        self.schedule_backup(window, cx);
    }

    fn reset(&mut self, _: &Reset, window: &mut Window, cx: &mut Context<Self>) {
        self.data.thumbnail_size = None;
        let size = self.data.original_size;
        window.resize(gpui_kit::size(px(size.x), px(size.y)));
        self.resume_animation(window, cx);
        self.set_opacity(1.0, window, cx);
    }

    fn edit(&mut self, _: &Edit, window: &mut Window, cx: &mut Context<Self>) {
        let target = View {
            window: window.window_handle(),
            entity: cx.entity(),
        };
        let image = self.get_image();
        let window_bounds = ui::to_rect(window.bounds());
        let Some(displayed) = get_contained_rect(
            vec2(image.width() as f32, image.height() as f32),
            window_bounds.size,
        ) else {
            return;
        };
        let bounds = Rect {
            pos: window_bounds.pos + displayed.pos,
            size: displayed.size,
        };
        if capture::open_pin_editor(image.clone(), bounds, target, cx) {
            platform::set_window_visible(window, false);
        }
    }

    fn show_menu(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        NativeMenu::new()
            .menu("Copy", Box::new(Copy))
            .menu("Save…", Box::new(Save))
            .menu("Quick Save", Box::new(QuickSave))
            .menu("Print…", Box::new(Print))
            .separator()
            .menu("Edit", Box::new(Edit))
            .menu_with_check(
                "Border and Shadow",
                cx.global::<Config>().pin_frame,
                Box::new(ToggleFrame),
            )
            .separator()
            .menu("Close", Box::new(Close))
            .menu("Destroy", Box::new(Destroy))
            .show(position, window, cx);
    }
}

impl Render for Pin {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let animated = self.data.content.frames.is_some();
        let sample = window
            .modifiers()
            .alt
            .then(|| self.get_sample(window))
            .flatten();
        let viewport = window.viewport_size();
        let viewport = vec2(viewport.width.into(), viewport.height.into());
        let pointer = window.mouse_position();
        let pointer = vec2(pointer.x.into(), pointer.y.into());
        let image = self.get_image().clone();
        let zoom = self
            .zoom_badge_task
            .as_ref()
            .map(|_| (viewport.x / self.data.original_size.x * 100.0).round());
        return div()
            .id("pin")
            .group("pin")
            .key_context(if animated { "Pin animated" } else { "Pin" })
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .opacity(self.data.opacity)
            .cursor(CursorStyle::OpenHand)
            .on_action(
                cx.listener(|pin, _: &Copy, _, cx| export::copy_image(pin.get_image().clone(), cx)),
            )
            .on_action(cx.listener(|pin, _: &CopyText, _, cx| {
                if let Some(text) = &pin.data.content.text {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                }
            }))
            .on_action(
                cx.listener(|pin, _: &CopyColor, window, cx| pin.copy_color(false, window, cx)),
            )
            .on_action(
                cx.listener(|pin, _: &CopyColorRgb, window, cx| pin.copy_color(true, window, cx)),
            )
            .on_action(cx.listener(|pin, _: &Save, _, cx| {
                export::save_content(pin.get_export_content(), None, cx)
            }))
            .on_action(cx.listener(|pin, _: &QuickSave, _, cx| {
                export::quick_save_content(pin.get_export_content(), None, cx)
            }))
            .on_action(
                cx.listener(|pin, _: &Print, _, cx| {
                    export::print_image(pin.get_image().clone(), cx)
                }),
            )
            .on_action(cx.listener(|pin, _: &Paste, window, cx| {
                match content::load_clipboard(cx) {
                    Ok(load) => pin.load_replacement(load, window, cx),
                    Err(error) => app::show_error(error),
                }
            }))
            .on_action(cx.listener(Self::edit))
            .on_action(cx.listener(Self::toggle_thumbnail))
            .on_action(cx.listener(Self::reset))
            .on_action(cx.listener(Self::close))
            .on_action(cx.listener(Self::destroy))
            .on_action(cx.listener(|_, _: &ToggleFrame, _, cx| cx.defer(toggle_frame)))
            .on_action(cx.listener(|pin, _: &ZoomIn, window, cx| pin.scale(1.1, window, cx)))
            .on_action(cx.listener(|pin, _: &ZoomOut, window, cx| pin.scale(0.9, window, cx)))
            .on_action(cx.listener(|pin, _: &MoreOpaque, window, cx| {
                pin.set_opacity(pin.data.opacity + 0.1, window, cx)
            }))
            .on_action(cx.listener(|pin, _: &LessOpaque, window, cx| {
                pin.set_opacity(pin.data.opacity - 0.1, window, cx)
            }))
            .on_action(cx.listener(|pin, _: &RotateClockwise, window, cx| {
                pin.transform(image::imageops::rotate90, window, cx)
            }))
            .on_action(cx.listener(|pin, _: &RotateCounterclockwise, window, cx| {
                pin.transform(image::imageops::rotate270, window, cx)
            }))
            .on_action(cx.listener(|pin, _: &FlipHorizontal, window, cx| {
                pin.transform(image::imageops::flip_horizontal, window, cx)
            }))
            .on_action(cx.listener(|pin, _: &FlipVertical, window, cx| {
                pin.transform(image::imageops::flip_vertical, window, cx)
            }))
            .on_action(cx.listener(|pin, _: &ToggleAnimation, window, cx| {
                if pin.paused_frame.is_some() {
                    pin.resume_animation(window, cx);
                } else {
                    // shortcut: GPUI hides the playback index, start at frame 0 until frame control is exposed.
                    pin.show_frame(0, window, cx);
                }
            }))
            .on_action(cx.listener(|pin, _: &PreviousFrame, window, cx| {
                let length = pin
                    .data
                    .content
                    .frames
                    .as_ref()
                    .map_or(1, |frames| frames.len());
                pin.show_frame(
                    pin.paused_frame.as_ref().map_or(0, |(index, _)| *index) + length - 1,
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|pin, _: &NextFrame, window, cx| {
                pin.show_frame(
                    pin.paused_frame.as_ref().map_or(0, |(index, _)| *index) + 1,
                    window,
                    cx,
                )
            }))
            .on_action(
                cx.listener(|_, nudge: &Nudge, window, _| platform::move_window(window, nudge.0)),
            )
            .on_modifiers_changed(cx.listener(|_, _, _, cx| cx.notify()))
            .on_mouse_move(cx.listener(|_, _, window, cx| {
                if window.modifiers().alt {
                    cx.notify();
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|pin, event: &MouseDownEvent, window, cx| {
                    if event.click_count < 2 {
                        window.start_window_move();
                    } else if event.modifiers.shift {
                        pin.toggle_thumbnail(&ToggleThumbnail, window, cx);
                    } else {
                        pin.close(&Close, window, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|pin, event: &MouseDownEvent, window, cx| {
                    pin.show_menu(event.position, window, cx)
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|pin, _, window, cx| pin.reset(&Reset, window, cx)),
            )
            .on_drop(cx.listener(|pin, paths: &ExternalPaths, window, cx| {
                let mut paths = paths.paths().to_vec();
                if !window.modifiers().shift && !paths.is_empty() {
                    let load = content::load_path_in_background(&paths.remove(0), cx);
                    pin.load_replacement(load, window, cx);
                }
                open_paths(paths, cx);
            }))
            .on_scroll_wheel(cx.listener(|pin, event: &ScrollWheelEvent, window, cx| {
                let delta: f32 = event.delta.pixel_delta(px(16.)).y.into();
                if delta == 0.0 {
                    return;
                }
                if event.modifiers.alt && pin.data.content.frames.is_some() {
                    pin.adjust_animation_speed(if delta > 0.0 { 0.9 } else { 1.1 }, window, cx);
                } else if event.modifiers.platform {
                    pin.set_opacity(pin.data.opacity + delta * 0.002, window, cx);
                } else {
                    pin.scale((1.0 + delta * 0.002).clamp(0.8, 1.2), window, cx);
                }
            }))
            .child(
                img(self.preview.clone())
                    .id("pin-image")
                    .size_full()
                    .object_fit(ObjectFit::Contain),
            )
            .when_some(zoom, |root, zoom| {
                root.child(
                    ui::panel(cx)
                        .absolute()
                        .top_1()
                        .left_1()
                        .px_1p5()
                        .text_xs()
                        .child(format!("{zoom}%")),
                )
            })
            .when_some(sample, |root, sample| {
                root.child(magnifier::render(
                    image,
                    sample,
                    magnifier::get_position(pointer, viewport),
                    window.modifiers().shift,
                    cx,
                ))
            })
            .child(
                ui::panel(cx)
                    .absolute()
                    .top_1()
                    .right_1()
                    .p_0p5()
                    .rounded(cx.theme().radius + px(3.))
                    .flex()
                    .invisible()
                    .group_hover("pin", |style| style.visible())
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(
                        [
                            (
                                "copy",
                                IconName::Copy,
                                "Copy",
                                Box::new(Copy) as Box<dyn Action>,
                            ),
                            ("save", IconName::Save, "Save", Box::new(Save)),
                            ("close", IconName::Close, "Close", Box::new(Close)),
                        ]
                        .map(|(id, icon, label, action)| {
                            let focus = self.focus.clone();
                            Button::new(id)
                                .ghost()
                                .small()
                                .icon(icon)
                                .tooltip(label)
                                .on_click(move |_, window, cx| {
                                    window.focus(&focus, cx);
                                    window.dispatch_action(action.boxed_clone(), cx);
                                })
                        }),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .size_3p5()
                    .cursor(CursorStyle::ResizeUpLeftDownRight)
                    .on_mouse_down(MouseButton::Left, |_, window, cx| {
                        cx.stop_propagation();
                        window.start_window_resize(ResizeEdge::BottomRight);
                    }),
            );
    }
}

fn scale_frame_delay(frame: &Frame, factor: f64) -> Frame {
    let (numerator, denominator) = frame.delay().numer_denom_ms();
    let milliseconds = (numerator as f64 / denominator as f64 * factor).clamp(10.0, 10_000.0);
    return Frame::from_parts(
        frame.buffer().clone(),
        frame.left(),
        frame.top(),
        image::Delay::from_saturating_duration(Duration::from_secs_f64(milliseconds / 1000.0)),
    );
}

fn get_zoomed_bounds(bounds: Rect, factor: f32) -> Rect {
    let factor = factor.max(MIN_SIZE / bounds.size.min_element());
    let size = bounds.size * factor;
    return Rect {
        pos: bounds.pos + (bounds.size - size) / 2.0,
        size,
    };
}

fn get_thumbnail_size(size: Vec2) -> Vec2 {
    let scale = (size / vec2(180., 120.))
        .max_element()
        .max(1.0)
        .min((size / MIN_SIZE).min_element().max(1.0));
    return size / scale;
}

fn get_image_pixel(image_size: Vec2, viewport: Vec2, pointer: Vec2) -> Option<(u32, u32)> {
    let displayed = get_contained_rect(image_size, viewport)?;
    let point = (pointer - displayed.pos) / displayed.size * image_size;
    if point.cmplt(Vec2::ZERO).any() || point.cmpge(image_size).any() {
        return None;
    }
    return Some((point.x as u32, point.y as u32));
}

fn get_contained_rect(image_size: Vec2, viewport: Vec2) -> Option<Rect> {
    if image_size.min_element() <= 0.0 || viewport.min_element() <= 0.0 {
        return None;
    }
    let displayed = image_size * (viewport / image_size).min_element();
    return Some(Rect {
        pos: (viewport - displayed) / 2.0,
        size: displayed,
    });
}

#[cfg(test)]
mod tests {
    use super::{
        Copy, Pin, PinData, Pins, create_id, get_contained_rect, get_image_pixel, get_logical_size,
        get_thumbnail_size, get_zoomed_bounds, init, scale_frame_delay, update_pins,
    };
    use crate::{app::content::PinContent, geometry::Rect, ui::View};
    use glam::vec2;
    use gpui_kit::{
        AppContext, ClipboardEntry, InputEvent, TestAppContext, WindowHandle, test::TestWindowExt,
    };
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::sync::Arc;

    fn create_pin_window(cx: &mut TestAppContext, content: PinContent) -> WindowHandle<Pin> {
        cx.update(|cx| cx.set_global(crate::app::config::Config::default()));
        let window = cx.add_window(|window, cx| {
            Pin::new(
                PinData {
                    id: create_id(),
                    content,
                    bounds: Rect {
                        pos: vec2(0.0, 0.0),
                        size: vec2(100.0, 100.0),
                    },
                    opacity: 1.0,
                    original_size: vec2(100.0, 100.0),
                    thumbnail_size: None,
                },
                window,
                cx,
            )
        });
        let entity = window.entity(cx).unwrap();
        cx.update(|cx| {
            cx.global_mut::<Pins>().views.push(View {
                window: window.into(),
                entity,
            })
        });
        return window;
    }

    #[gpui_kit::test]
    fn animated_pins_schedule_more_frames(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(directory.path(), cx);
        });
        let frames = vec![Frame::new(RgbaImage::new(2, 1)); 2];
        let window = create_pin_window(
            cx,
            PinContent {
                image: Arc::new(frames[0].buffer().clone()),
                frames: Some(Arc::new(frames)),
                text: None,
            },
        );
        window
            .update(cx, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.simulate_next_frame(cx);
            window.render_frame(cx);
            assert!(window.simulate_next_frame(cx) > 0);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn animated_pins_ignore_horizontal_scroll(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(directory.path(), cx);
        });
        let frames = Arc::new(vec![
            Frame::from_parts(
                RgbaImage::new(2, 1),
                0,
                0,
                Delay::from_numer_denom_ms(100, 1),
            );
            2
        ]);
        let window = create_pin_window(
            cx,
            PinContent {
                image: Arc::new(frames[0].buffer().clone()),
                frames: Some(frames.clone()),
                text: None,
            },
        );
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: gpui_kit::point(gpui_kit::px(50.0), gpui_kit::px(50.0)),
                    delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(12.0),
                        gpui_kit::px(0.0),
                    )),
                    modifiers: gpui_kit::Modifiers {
                        alt: true,
                        ..Default::default()
                    },
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        window
            .update(cx, |pin, _, _| {
                assert!(Arc::ptr_eq(
                    pin.data.content.frames.as_ref().unwrap(),
                    &frames
                ));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn copying_paused_animation_uses_the_selected_frame(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(directory.path(), cx);
        });
        let frames = vec![
            Frame::new(RgbaImage::from_pixel(2, 1, Rgba([255, 0, 0, 255]))),
            Frame::new(RgbaImage::from_pixel(2, 1, Rgba([0, 0, 255, 255]))),
        ];
        let window = create_pin_window(
            cx,
            PinContent {
                image: Arc::new(frames[0].buffer().clone()),
                frames: Some(Arc::new(frames)),
                text: None,
            },
        );
        window
            .update(cx, |pin, window, cx| {
                pin.show_frame(1, window, cx);
                window.focus(&pin.focus, cx);
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.dispatch_action(Box::new(Copy), cx);
        })
        .unwrap();
        cx.run_until_parked();
        cx.update(|cx| {
            let clipboard = cx.read_from_clipboard().unwrap();
            let ClipboardEntry::Image(image) = &clipboard.entries()[0] else {
                panic!("expected a copied image");
            };
            let image = image::load_from_memory(&image.bytes).unwrap().to_rgba8();
            assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255, 255]);
        });
        let directory = tempfile::tempdir().unwrap();
        window
            .update(cx, |pin, window, cx| {
                crate::app::content::write_path(
                    &pin.get_export_content(),
                    &directory.path().join("paused"),
                )
                .unwrap();
                let saved =
                    crate::app::content::load_image_file(&directory.path().join("paused.png"))
                        .unwrap();
                assert!(saved.frames.is_none());
                assert_eq!(saved.image.get_pixel(0, 0).0, [0, 0, 255, 255]);
                pin.resume_animation(window, cx);
                let content = pin.get_export_content();
                assert_eq!(content.get_default_extension(), "gif");
                assert_eq!(content.frames.as_ref().unwrap().len(), 2);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn transforming_and_retiming_animations_preserves_the_paused_frame(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(directory.path(), cx);
        });
        let delay = Delay::from_numer_denom_ms(80, 1);
        let frames = Arc::new(vec![
            Frame::from_parts(RgbaImage::new(2, 1), 0, 0, delay),
            Frame::from_parts(
                RgbaImage::from_fn(2, 1, |x, _| Rgba([x as u8, 0, 0, 255])),
                0,
                0,
                delay,
            ),
        ]);
        let window = create_pin_window(
            cx,
            PinContent {
                image: Arc::new(frames[0].buffer().clone()),
                frames: Some(frames),
                text: None,
            },
        );
        window
            .update(cx, |pin, window, cx| {
                pin.show_frame(1, window, cx);
                pin.transform(image::imageops::flip_horizontal, window, cx);
                assert_eq!(pin.paused_frame.as_ref().map(|(index, _)| *index), Some(1));
                assert_eq!(pin.get_image().get_pixel(0, 0).0, [1, 0, 0, 255]);
                pin.adjust_animation_speed(0.5, window, cx);
                assert_eq!(pin.paused_frame.as_ref().map(|(index, _)| *index), Some(1));
                assert_eq!(
                    pin.data.content.frames.as_ref().unwrap()[1]
                        .delay()
                        .numer_denom_ms(),
                    (40, 1)
                );
                assert_eq!(pin.get_image().get_pixel(0, 0).0, [1, 0, 0, 255]);
                pin.replace_content(pin.data.content.clone(), window, cx);
                assert!(pin.paused_frame.is_none());
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn closing_pin_windows_releases_images(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            init(directory.path(), cx);
        });
        for bulk in [false, true] {
            let window =
                create_pin_window(cx, PinContent::from_image(Arc::new(RgbaImage::new(1, 1))));
            let pin = window.entity(cx).unwrap().downgrade();
            if bulk {
                cx.update(|cx| update_pins(cx, |_, window, _| window.remove_window()));
            } else {
                window
                    .update(cx, |_, window, _| window.remove_window())
                    .unwrap();
            }
            cx.run_until_parked();
            cx.update(|cx| assert!(cx.global::<Pins>().views.is_empty()));
            assert!(pin.upgrade().is_none());
        }
    }

    #[test]
    fn animation_speed_stays_within_safe_delay_bounds() {
        let frame = Frame::from_parts(
            RgbaImage::new(1, 1),
            0,
            0,
            Delay::from_numer_denom_ms(100, 1),
        );
        assert_eq!(
            scale_frame_delay(&frame, 0.5).delay().numer_denom_ms(),
            (50, 1)
        );
        assert_eq!(
            scale_frame_delay(&frame, 0.0).delay().numer_denom_ms(),
            (10, 1)
        );
    }

    #[test]
    fn zoom_keeps_center_and_aspect_ratio_above_minimum_size() {
        let bounds = Rect {
            pos: vec2(100.0, 100.0),
            size: vec2(200.0, 100.0),
        };
        let center = bounds.pos + bounds.size / 2.0;
        let zoomed = get_zoomed_bounds(bounds, 1.5);
        assert_eq!(zoomed.size, vec2(300.0, 150.0));
        assert_eq!(zoomed.pos + zoomed.size / 2.0, center);
        let smallest = get_zoomed_bounds(bounds, 0.1);
        assert_eq!(smallest.size, vec2(80.0, 40.0));
        assert_eq!(smallest.pos + smallest.size / 2.0, center);
    }

    #[test]
    fn thumbnails_fit_without_upscaling() {
        assert_eq!(get_thumbnail_size(vec2(90.0, 60.0)), vec2(90.0, 60.0));
        assert_eq!(get_thumbnail_size(vec2(360.0, 120.0)), vec2(180.0, 60.0));
        assert_eq!(get_thumbnail_size(vec2(100.0, 400.0)), vec2(40.0, 160.0));
    }

    #[test]
    fn sampling_accounts_for_letterboxing() {
        let image = vec2(2.0, 1.0);
        let viewport = vec2(100.0, 100.0);
        assert_eq!(
            get_image_pixel(image, viewport, vec2(75.0, 50.0)),
            Some((1, 0))
        );
        assert_eq!(get_image_pixel(image, viewport, vec2(50.0, 10.0)), None);
        assert_eq!(get_image_pixel(image, vec2(0.0, 0.0), vec2(0.0, 0.0)), None);
    }

    #[test]
    fn ids_are_unique_without_clock_progress() {
        assert_ne!(create_id(), create_id());
    }

    #[test]
    fn logical_size_caps_large_images_without_upscaling() {
        assert_eq!(
            get_logical_size(&RgbaImage::new(100, 50)),
            vec2(100.0, 50.0)
        );
        assert_eq!(
            get_logical_size(&RgbaImage::new(1600, 1200)),
            vec2(800.0, 600.0)
        );
    }

    #[test]
    fn editor_uses_visible_image_bounds() {
        let bounds = get_contained_rect(vec2(200.0, 100.0), vec2(100.0, 100.0)).unwrap();
        assert_eq!(bounds.pos, vec2(0.0, 25.0));
        assert_eq!(bounds.size, vec2(100.0, 50.0));
    }
}
