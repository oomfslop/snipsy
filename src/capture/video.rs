use crate::{
    app::{self, AppState, config::Config},
    capture::{CaptureView, Mode, layout},
    editor::{Annotation, Tool},
    geometry::Rect,
    platform::{self, RecordedVideo, VideoExport, VideoPlayer},
    raster::{self, Screenshot},
    ui,
};
use anyhow::{Result, ensure};
use glam::{Vec2, vec2};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Disableable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
    },
    prelude::*,
    *,
};
use image::RgbaImage;
use std::{
    cell::Cell,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

pub const VIDEO_EDITOR_WINDOW: &str = "video-editor";
const TIMELINE_HEIGHT: f32 = 52.0;
const TOP_PADDING: f32 = 36.0;
const SIDE_PADDING: f32 = 16.0;
const TOOLBAR_SPACE: f32 = 100.0;
const MIN_WIDTH: f32 = 680.0;
const HANDLE_WIDTH: f32 = 8.0;
const MIN_CLIP: f64 = 0.1;

pub enum VideoRequest {
    Export,
    TogglePlayback,
}

#[derive(Clone, Copy, PartialEq)]
enum TimelineDrag {
    Start,
    End,
    Playhead,
}

pub struct VideoEditor {
    video: RecordedVideo,
    player: VideoPlayer,
    capture: Entity<CaptureView>,
    canvas: Rect,
    duration: f64,
    trim: (f64, f64),
    loaded: bool,
    frame: Option<SurfaceSource>,
    frame_deadline: Option<Instant>,
    track: Rc<Cell<Option<Rect>>>,
    drag: Option<TimelineDrag>,
    mosaics: Vec<Annotation>,
    export: Option<VideoExport>,
    export_task: Option<Task<()>>,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

pub fn open_video_editor(video: RecordedVideo, cx: &mut App) {
    if let Err(error) = show_video_editor(video, cx) {
        app::show_error(error);
    }
}

fn show_video_editor(video: RecordedVideo, cx: &mut App) -> Result<()> {
    let player = VideoPlayer::open(&video.path)?;
    let duration = player.get_duration();
    ensure!(duration > 0.0, "The recording is empty");
    let screen = cx.primary_display().map_or(vec2(1440.0, 900.0), |display| {
        ui::to_rect(display.bounds()).size
    });
    let pixels = vec2(video.size.0 as f32, video.size.1 as f32);
    let (window_size, canvas) = get_editor_layout(pixels / platform::get_desktop_scale(), screen);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(window_size.x), px(window_size.y)),
            cx,
        ))),
        titlebar: Some(TitlebarOptions {
            title: Some("Recording".into()),
            ..Default::default()
        }),
        is_resizable: false,
        ..Default::default()
    };
    let mut video = Some(video);
    let mut player = Some(player);
    app::open_window(VIDEO_EDITOR_WINDOW, options, cx, |window, cx| {
        cx.new(|cx| {
            VideoEditor::new(
                video.take().unwrap(),
                player.take().unwrap(),
                duration,
                window_size,
                canvas,
                window,
                cx,
            )
        })
    })?;
    return Ok(());
}

fn get_editor_layout(video: Vec2, screen: Vec2) -> (Vec2, Rect) {
    let chrome = vec2(
        2.0 * SIDE_PADDING,
        TOP_PADDING + TOOLBAR_SPACE + TIMELINE_HEIGHT + 28.0,
    );
    let available = (screen * 0.9 - chrome).max(vec2(160.0, 90.0));
    let scale = (available / video.max(Vec2::ONE)).min_element().min(1.0);
    let canvas = (video * scale).floor().max(Vec2::ONE);
    let window = vec2(
        (canvas.x + 2.0 * SIDE_PADDING).max(MIN_WIDTH),
        TOP_PADDING + canvas.y + TOOLBAR_SPACE + TIMELINE_HEIGHT,
    );
    return (
        window,
        Rect {
            pos: vec2(((window.x - canvas.x) / 2.0).round(), TOP_PADDING),
            size: canvas,
        },
    );
}

fn get_crop(selection: Rect, canvas: Rect, size: (u32, u32)) -> Option<Rect> {
    if selection == canvas {
        return None;
    }
    let pixels = vec2(size.0 as f32, size.1 as f32);
    let scale = pixels / canvas.size;
    let start = ((selection.pos - canvas.pos) * scale)
        .round()
        .max(Vec2::ZERO);
    let end = ((selection.end() - canvas.pos) * scale).round().min(pixels);
    let size = ((end - start) / 2.0).floor().max(Vec2::ONE) * 2.0;
    return Some(Rect {
        pos: start.min(pixels - size),
        size,
    });
}

fn format_time(seconds: f64) -> String {
    let tenths = (seconds.max(0.0) * 10.0).round() as u64;
    return format!("{}:{:02}.{}", tenths / 600, tenths / 10 % 60, tenths % 10);
}

impl VideoEditor {
    fn new(
        video: RecordedVideo,
        player: VideoPlayer,
        duration: f64,
        window_size: Vec2,
        canvas: Rect,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: window_size - vec2(0.0, TIMELINE_HEIGHT),
        };
        let screenshot = Screenshot {
            image: Arc::new(RgbaImage::new(video.size.0, video.size.1)),
            bounds: canvas,
            windows: Vec::new(),
        };
        let capture = cx.new(|cx| {
            CaptureView::new(
                bounds,
                Mode::Video,
                Some(screenshot),
                Vec2::ZERO,
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &capture,
                window,
                |editor, _, request, window, cx| match request {
                    VideoRequest::Export => editor.export(window, cx),
                    VideoRequest::TogglePlayback => editor.toggle_playback(cx),
                },
            ),
            cx.observe(&capture, |editor, _, cx| editor.sync_mosaics(cx)),
            cx.observe_global::<Config>(|editor, cx| {
                editor.capture.update(cx, |_, cx| cx.notify())
            }),
        ];
        let close_capture = capture.clone();
        window.on_window_should_close(cx, move |window, cx| {
            close_capture.update(cx, |capture, cx| capture.cancel(window, cx));
            return false;
        });
        return Self {
            video,
            player,
            capture,
            canvas,
            duration,
            trim: (0.0, duration),
            loaded: false,
            frame: None,
            frame_deadline: None,
            track: Rc::new(Cell::new(None)),
            drag: None,
            mosaics: Vec::new(),
            export: None,
            export_task: None,
            error: None,
            _subscriptions: subscriptions,
        };
    }

    fn seek(&mut self, time: f64, cx: &mut Context<Self>) {
        self.player.seek(time.clamp(0.0, self.duration));
        self.await_frame();
        cx.notify();
    }

    fn await_frame(&mut self) {
        self.frame_deadline = Some(Instant::now() + Duration::from_secs(1));
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.player.get_is_playing() {
            self.player.pause();
            return cx.notify();
        }
        let time = self.player.get_time();
        if time < self.trim.0 || time >= self.trim.1 - 0.05 {
            self.player.seek(self.trim.0);
        }
        self.player.play(self.trim.1);
        cx.notify();
    }

    fn sync_mosaics(&mut self, cx: &mut Context<Self>) {
        let mosaics = self
            .capture
            .read(cx)
            .editor
            .get_visible_annotations()
            .filter(|annotation| annotation.tool == Tool::Mosaic)
            .cloned()
            .collect::<Vec<_>>();
        if mosaics == self.mosaics {
            return;
        }
        let fonts = cx.global::<AppState>().fonts.clone();
        match raster::create_video_effects(self.video.size, self.canvas, mosaics.iter(), &fonts) {
            Ok(effects) => self.player.set_effects(effects),
            Err(error) => self.error = Some(format!("{error:#}").into()),
        }
        self.mosaics = mosaics;
        self.await_frame();
        cx.notify();
    }

    fn get_track_time(&self, position: Point<Pixels>) -> Option<f64> {
        let track = self.track.get()?;
        let x: f32 = position.x.into();
        return Some(((x - track.pos.x) / track.size.x).clamp(0.0, 1.0) as f64 * self.duration);
    }

    fn get_track_x(&self, track: Rect, time: f64) -> f32 {
        return track.pos.x + (time / self.duration) as f32 * track.size.x;
    }

    fn begin_timeline_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some(track) = self.track.get() else {
            return;
        };
        let x: f32 = event.position.x.into();
        let start = (x - self.get_track_x(track, self.trim.0)).abs();
        let end = (x - self.get_track_x(track, self.trim.1)).abs();
        self.drag = Some(if start.min(end) > HANDLE_WIDTH {
            TimelineDrag::Playhead
        } else if start < end {
            TimelineDrag::Start
        } else {
            TimelineDrag::End
        });
        self.player.pause();
        self.update_timeline_drag(event.position, cx);
    }

    fn update_timeline_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let (Some(drag), Some(time)) = (self.drag, self.get_track_time(position)) else {
            return;
        };
        let time = match drag {
            TimelineDrag::Start => {
                self.trim.0 = time.min(self.trim.1 - MIN_CLIP).max(0.0);
                self.trim.0
            }
            TimelineDrag::End => {
                self.trim.1 = time.max(self.trim.0 + MIN_CLIP).min(self.duration);
                self.trim.1
            }
            TimelineDrag::Playhead => time,
        };
        self.seek(time, cx);
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.export_task.is_some() {
            return;
        }
        let (annotations, selection, canvas) = self.capture.update(cx, |capture, _| {
            capture.editor.commit_draft();
            (
                capture
                    .editor
                    .get_visible_annotations()
                    .cloned()
                    .collect::<Vec<_>>(),
                capture.selection,
                capture.get_canvas(),
            )
        });
        let Some(selection) =
            selection.filter(|selection| layout::get_is_valid_selection(*selection))
        else {
            self.error = Some("Select an area first".into());
            return cx.notify();
        };
        self.player.pause();
        self.error = None;
        let fonts = cx.global::<AppState>().fonts.clone();
        let size = self.video.size;
        let destination = platform::prompt_save_video();
        self.export_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = async {
                let Some(destination) = destination.recv().await?? else {
                    return Ok(false);
                };
                let effects = cx
                    .background_spawn(async move {
                        raster::create_video_effects(size, canvas, annotations.iter(), &fonts)
                    })
                    .await?;
                let (sender, receiver) = async_channel::bounded(1);
                this.update(cx, |editor, cx| {
                    editor.export = Some(platform::export_video(
                        &editor.video.path,
                        destination,
                        editor.trim,
                        get_crop(selection, canvas, size),
                        effects,
                        editor.video.codec,
                        sender,
                    )?);
                    cx.notify();
                    anyhow::Ok(())
                })??;
                while receiver.is_empty() && !receiver.is_closed() {
                    cx.background_executor()
                        .timer(Duration::from_millis(100))
                        .await;
                    this.update(cx, |_, cx| cx.notify())?;
                }
                receiver.recv().await??;
                anyhow::Ok(true)
            }
            .await;
            let _ = this.update_in(cx, |editor, window, cx| {
                editor.export = None;
                editor.export_task = None;
                match result {
                    Ok(true) => window.remove_window(),
                    Ok(false) => {}
                    Err(error) => editor.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn render_timeline(
        &self,
        time: f64,
        playing: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let track = self.track.clone();
        let (trim, duration) = (self.trim, self.duration);
        let (accent, muted, foreground) = (theme.primary, theme.muted, theme.foreground);
        let export_label: SharedString = match &self.export {
            Some(export) => {
                format!("Exporting {}%", (export.get_progress() * 100.0).round()).into()
            }
            None => "Export".into(),
        };
        return h_flex()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(TIMELINE_HEIGHT))
            .px_4()
            .gap_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                Button::new("play")
                    .icon(if playing {
                        IconName::Pause
                    } else {
                        IconName::Play
                    })
                    .tooltip(if playing {
                        "Pause (Space)"
                    } else {
                        "Play (Space)"
                    })
                    .ghost()
                    .on_click(cx.listener(|editor, _, _, cx| editor.toggle_playback(cx))),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .min_w(px(96.))
                    .child(format!(
                        "{} / {}",
                        format_time(time.clamp(trim.0, trim.1) - trim.0),
                        format_time(trim.1 - trim.0)
                    )),
            )
            .child(
                div()
                    .id("timeline")
                    .flex_1()
                    .h(px(28.))
                    .cursor(CursorStyle::PointingHand)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, event: &MouseDownEvent, _, cx| {
                            editor.begin_timeline_drag(event, cx)
                        }),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| track.set(Some(ui::to_rect(bounds))),
                            move |bounds, _, window, _| {
                                let rect = ui::to_rect(bounds);
                                let get_x =
                                    |time: f64| rect.pos.x + (time / duration) as f32 * rect.size.x;
                                let mut background = fill(bounds, muted);
                                background.corner_radii = Corners::all(px(4.));
                                window.paint_quad(background);
                                let clip = Rect::from_corners(
                                    vec2(get_x(trim.0), rect.pos.y),
                                    vec2(get_x(trim.1), rect.end().y),
                                );
                                let mut selected =
                                    outline(ui::to_bounds(clip), accent, BorderStyle::Solid);
                                selected.border_widths = Edges {
                                    top: px(2.),
                                    bottom: px(2.),
                                    left: px(HANDLE_WIDTH),
                                    right: px(HANDLE_WIDTH),
                                };
                                selected.corner_radii = Corners::all(px(4.));
                                window.paint_quad(selected);
                                let playhead = Rect {
                                    pos: vec2(get_x(time) - 1.0, rect.pos.y - 3.0),
                                    size: vec2(2.0, rect.size.y + 6.0),
                                };
                                window.paint_quad(fill(ui::to_bounds(playhead), foreground));
                            },
                        )
                        .size_full(),
                    ),
            )
            .child(
                Button::new("export")
                    .label(export_label)
                    .primary()
                    .disabled(self.export_task.is_some())
                    .on_click(cx.listener(|editor, _, window, cx| editor.export(window, cx))),
            );
    }
}

impl Render for VideoEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.loaded {
            match self.player.get_is_ready() {
                Ok(ready) => self.loaded = ready,
                Err(error) => {
                    self.loaded = true;
                    self.error = Some(format!("{error:#}").into());
                }
            }
            if self.loaded {
                self.player.seek(0.0);
                self.await_frame();
            } else {
                window.request_animation_frame();
            }
        }
        let playing = self.player.get_is_playing();
        if let Some(frame) = self.player.copy_frame() {
            self.frame = Some(frame);
            if !playing {
                self.frame_deadline = None;
            }
        }
        if self
            .frame_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.frame_deadline = None;
        }
        if playing || self.frame_deadline.is_some() {
            window.request_animation_frame();
        }
        let canvas = self.canvas;
        let time = self.player.get_time();
        let theme = cx.theme();
        return div()
            .id("video-editor")
            .size_full()
            .relative()
            .bg(theme.background)
            .on_action(cx.listener(|editor, _: &app::CloseWindow, window, cx| {
                editor
                    .capture
                    .update(cx, |capture, cx| capture.cancel(window, cx));
            }))
            .on_action(|_: &app::MinimizeWindow, window, _| window.minimize_window())
            .on_mouse_move(cx.listener(|editor, event: &MouseMoveEvent, _, cx| {
                if editor.drag.is_some() {
                    editor.update_timeline_drag(event.position, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|editor, _, _, _| editor.drag = None),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|editor, _, _, _| editor.drag = None),
            )
            .when_some(self.frame.clone(), |root, frame| {
                root.child(
                    surface(frame)
                        .absolute()
                        .left(px(canvas.pos.x))
                        .top(px(canvas.pos.y))
                        .w(px(canvas.size.x))
                        .h(px(canvas.size.y)),
                )
            })
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .right_0()
                    .bottom(px(TIMELINE_HEIGHT))
                    .child(
                        self.capture
                            .clone()
                            .cached(StyleRefinement::default().size_full()),
                    ),
            )
            .child(self.render_timeline(time, playing, cx))
            .when_some(self.error.clone(), |root, error| {
                root.child(
                    ui::panel(cx)
                        .absolute()
                        .left_4()
                        .bottom(px(TIMELINE_HEIGHT + 16.0))
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            });
    }
}

#[cfg(test)]
mod tests {
    use super::{format_time, get_crop, get_editor_layout};
    use crate::geometry::Rect;
    use glam::vec2;

    #[test]
    fn editor_fits_large_videos_on_screen_and_keeps_small_ones_at_full_size() {
        let (window, canvas) = get_editor_layout(vec2(3000.0, 2000.0), vec2(1440.0, 900.0));
        assert!(window.x <= 1440.0 && window.y <= 900.0);
        assert!((canvas.size.x / canvas.size.y - 1.5).abs() < 0.01);
        let (window, canvas) = get_editor_layout(vec2(200.0, 100.0), vec2(1440.0, 900.0));
        assert_eq!(canvas.size, vec2(200.0, 100.0));
        assert_eq!(canvas.pos.x, (window.x - 200.0) / 2.0);
    }

    #[test]
    fn crop_maps_selection_to_even_video_pixels() {
        let canvas = Rect {
            pos: vec2(10.0, 20.0),
            size: vec2(100.0, 50.0),
        };
        assert_eq!(get_crop(canvas, canvas, (200, 100)), None);
        let selection = Rect {
            pos: vec2(20.0, 30.0),
            size: vec2(40.5, 20.0),
        };
        assert_eq!(
            get_crop(selection, canvas, (200, 100)),
            Some(Rect {
                pos: vec2(20.0, 20.0),
                size: vec2(80.0, 40.0),
            })
        );
    }

    #[test]
    fn times_show_minutes_seconds_and_tenths() {
        assert_eq!(format_time(0.0), "0:00.0");
        assert_eq!(format_time(75.25), "1:15.3");
    }
}
