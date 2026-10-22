use crate::{
    app::{self, AppState, config::Config},
    capture::{CaptureView, Mode, layout, toolbar},
    editor::{Annotation, Tool},
    geometry::Rect,
    platform::{self, RecordedVideo, VideoExport, VideoPlayer, VideoThumbnails},
    raster::{self, Screenshot},
    ui,
};
use anyhow::{Result, ensure};
use glam::{Vec2, vec2};
use gpui_kit::{
    assets::IconName,
    component::{
        ActiveTheme, Disableable as _, Icon, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::{Scrollbar, ScrollbarMode},
        v_flex,
    },
    prelude::*,
    *,
};
use image::RgbaImage;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub const VIDEO_EDITOR_WINDOW: &str = "video-editor";
const TRANSPORT_HEIGHT: f32 = 40.0;
const RULER_HEIGHT: f32 = 22.0;
const LANE_HEIGHT: f32 = 26.0;
const ANNOTATION_LANES: f32 = 3.0;
const VIDEO_LANE_HEIGHT: f32 = 36.0;
const TIMELINE_HEIGHT: f32 =
    TRANSPORT_HEIGHT + RULER_HEIGHT + ANNOTATION_LANES * LANE_HEIGHT + VIDEO_LANE_HEIGHT;
const MIN_TIMELINE_HEIGHT: f32 = TRANSPORT_HEIGHT + RULER_HEIGHT + LANE_HEIGHT + VIDEO_LANE_HEIGHT;
const THUMBNAIL_HEIGHT: f32 = VIDEO_LANE_HEIGHT - 9.0;
const HEADER_WIDTH: f32 = 120.0;
const TRACK_INSET: f32 = 10.0;
const TRACK_END_INSET: f32 = 18.0;
const RULER_STEPS: [f64; 13] = [
    0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0,
];
const TOP_PADDING: f32 = 36.0;
const SIDE_PADDING: f32 = 16.0;
const TOOLBAR_SPACE: f32 = 100.0;
const MIN_WIDTH: f32 = 680.0;
const MIN_CANVAS_HEIGHT: f32 = 120.0;
const HANDLE_WIDTH: f32 = 6.0;
const VOLUME_DRAG_HEIGHT: f32 = 60.0;
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
    Layer(u64, LayerDrag),
    Volume(usize, f32, f32),
    Resize,
}

struct AudioTrack {
    id: i32,
    label: SharedString,
    volume: f32,
    muted: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum LayerDrag {
    Start,
    End,
    Move(f64),
}

pub struct VideoEditor {
    video: RecordedVideo,
    player: VideoPlayer,
    capture: Entity<CaptureView>,
    video_size: Vec2,
    window_size: Vec2,
    timeline_height: f32,
    audio: Vec<AudioTrack>,
    canvas: Rect,
    duration: f64,
    trim: (f64, f64),
    loaded: bool,
    frame: Option<SurfaceSource>,
    frame_deadline: Option<Instant>,
    track_width: f32,
    thumbnail_width: f32,
    thumbnails: Vec<Option<Arc<RenderImage>>>,
    lanes_scroll: ScrollHandle,
    _thumbnail_generator: Option<VideoThumbnails>,
    _thumbnail_task: Task<()>,
    drag: Option<TimelineDrag>,
    mosaics: Vec<(Annotation, Option<(f64, f64)>)>,
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
    let video_size = pixels / platform::get_desktop_scale();
    let timeline_height = TIMELINE_HEIGHT + video.audio.len() as f32 * LANE_HEIGHT;
    let window_size = get_window_size(video_size, screen, timeline_height);
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
        window_min_size: Some(size(
            px(MIN_WIDTH),
            px(TOP_PADDING + MIN_CANVAS_HEIGHT + TOOLBAR_SPACE + timeline_height),
        )),
        ..Default::default()
    };
    let mut video = Some(video);
    let mut player = Some(player);
    app::open_window(VIDEO_EDITOR_WINDOW, options, cx, |window, cx| {
        cx.new(|cx| {
            VideoEditor::new(
                video.take().unwrap(),
                player.take().unwrap(),
                video_size,
                timeline_height,
                screen.x,
                window,
                cx,
            )
        })
    })?;
    return Ok(());
}

fn get_window_size(video: Vec2, screen: Vec2, timeline_height: f32) -> Vec2 {
    let chrome = vec2(
        2.0 * SIDE_PADDING,
        TOP_PADDING + TOOLBAR_SPACE + timeline_height,
    );
    let available = (screen * 0.9 - chrome - vec2(0.0, 28.0)).max(vec2(160.0, 90.0));
    let scale = (available / video.max(Vec2::ONE)).min_element().min(1.0);
    let canvas = (video * scale).ceil().max(Vec2::ONE);
    return vec2((canvas.x + chrome.x).max(MIN_WIDTH), canvas.y + chrome.y);
}

fn get_canvas_rect(video: Vec2, window: Vec2, timeline_height: f32) -> Rect {
    let chrome = vec2(
        2.0 * SIDE_PADDING,
        TOP_PADDING + TOOLBAR_SPACE + timeline_height,
    );
    let available = (window - chrome).max(Vec2::ONE);
    let scale = (available / video.max(Vec2::ONE)).min_element();
    let canvas = (video * scale).floor().max(Vec2::ONE);
    return Rect {
        pos: vec2(((window.x - canvas.x) / 2.0).round(), TOP_PADDING),
        size: canvas,
    };
}

fn get_track_width(window_width: f32) -> f32 {
    return (window_width - HEADER_WIDTH - TRACK_INSET - TRACK_END_INSET).max(1.0);
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
        video_size: Vec2,
        timeline_height: f32,
        screen_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let duration = player.get_duration();
        let window_size = ui::to_rect(Bounds::new(Point::default(), window.viewport_size())).size;
        let canvas = get_canvas_rect(video_size, window_size, timeline_height);
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: window_size - vec2(0.0, timeline_height),
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
            cx.observe(&capture, |editor, _, cx| editor.sync_capture(cx)),
            cx.observe_global::<Config>(|editor, cx| {
                editor.capture.update(cx, |_, cx| cx.notify())
            }),
            cx.observe_window_bounds(window, |editor, window, cx| {
                editor.update_layout(window, cx)
            }),
        ];
        let thumbnail_width = (THUMBNAIL_HEIGHT * video_size.x / video_size.y.max(1.0)).max(1.0);
        let count = (screen_width / thumbnail_width).ceil().max(1.0) as usize;
        let times = (0..count)
            .map(|index| (index as f64 + 0.5) / count as f64 * duration)
            .collect::<Vec<_>>();
        let (sender, receiver) = async_channel::bounded(count);
        let scale = window.scale_factor();
        let (thumbnail_generator, error) = match platform::generate_video_thumbnails(
            &video.path,
            &times,
            (
                (thumbnail_width * scale).ceil() as u32,
                (THUMBNAIL_HEIGHT * scale).ceil() as u32,
            ),
            sender,
        ) {
            Ok(generator) => (Some(generator), None),
            Err(error) => (None, Some(format!("{error:#}").into())),
        };
        let thumbnail_task = cx.spawn(async move |this, cx| {
            while let Ok((index, image)) = receiver.recv().await {
                let image = raster::create_render_image(&image);
                let stored = this.update(cx, |editor, cx| {
                    editor.thumbnails[index] = Some(image);
                    cx.notify();
                });
                if stored.is_err() {
                    break;
                }
            }
        });
        let close_capture = capture.clone();
        window.on_window_should_close(cx, move |window, cx| {
            close_capture.update(cx, |capture, cx| capture.cancel(window, cx));
            return false;
        });
        return Self {
            video,
            player,
            capture,
            video_size,
            window_size,
            timeline_height,
            audio: Vec::new(),
            canvas,
            duration,
            trim: (0.0, duration),
            loaded: false,
            frame: None,
            frame_deadline: None,
            track_width: get_track_width(window_size.x),
            thumbnail_width,
            thumbnails: vec![None; count],
            lanes_scroll: ScrollHandle::new(),
            _thumbnail_generator: thumbnail_generator,
            _thumbnail_task: thumbnail_task,
            drag: None,
            mosaics: Vec::new(),
            export: None,
            export_task: None,
            error,
            _subscriptions: subscriptions,
        };
    }

    fn update_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.window_size = ui::to_rect(Bounds::new(Point::default(), window.viewport_size())).size;
        self.resize_timeline(self.timeline_height, cx);
    }

    fn resize_timeline(&mut self, height: f32, cx: &mut Context<Self>) {
        let size = self.window_size;
        let max = size.y - TOP_PADDING - MIN_CANVAS_HEIGHT - TOOLBAR_SPACE;
        let min = MIN_TIMELINE_HEIGHT + self.audio.len() as f32 * LANE_HEIGHT;
        self.timeline_height = height.min(max).max(min);
        let canvas = get_canvas_rect(self.video_size, size, self.timeline_height);
        self.track_width = get_track_width(size.x);
        if canvas != self.canvas {
            self.canvas = canvas;
            let bounds = Rect {
                pos: Vec2::ZERO,
                size: size - vec2(0.0, self.timeline_height),
            };
            self.capture
                .update(cx, |capture, cx| capture.resize_video(bounds, canvas, cx));
        }
        cx.notify();
    }

    fn load_audio(&mut self, cx: &mut Context<Self>) {
        let ids = self.player.get_audio_track_ids();
        let sources = &self.video.audio;
        self.audio = ids
            .iter()
            .enumerate()
            .map(|(index, id)| AudioTrack {
                id: *id,
                label: match sources.get(index).filter(|_| ids.len() == sources.len()) {
                    Some(source) => source.get_label().into(),
                    None => format!("Audio {}", index + 1).into(),
                },
                volume: 1.0,
                muted: false,
            })
            .collect();
        let added = self.audio.len() as f32 - sources.len() as f32;
        self.resize_timeline(self.timeline_height + added * LANE_HEIGHT, cx);
    }

    fn get_audio_volumes(&self) -> Vec<(i32, f32)> {
        return self
            .audio
            .iter()
            .map(|track| (track.id, if track.muted { 0.0 } else { track.volume }))
            .collect();
    }

    fn sync_audio(&self, cx: &mut Context<Self>) {
        self.player.set_audio_volumes(&self.get_audio_volumes());
        cx.notify();
    }

    fn seek(&mut self, time: f64, cx: &mut Context<Self>) {
        let time = time.clamp(0.0, self.duration);
        self.player.seek(time);
        self.sync_time(time, cx);
        self.await_frame();
        cx.notify();
    }

    fn sync_time(&self, time: f64, cx: &mut App) {
        self.capture.update(cx, |capture, cx| {
            if capture.editor.set_time(time) {
                cx.notify();
            }
        });
    }

    fn await_frame(&mut self) {
        self.frame_deadline = Some(Instant::now() + Duration::from_secs(1));
    }

    fn toggle_playback(&mut self, cx: &mut Context<Self>) {
        if self.player.get_is_playing() {
            self.player.pause();
            self.sync_time(self.player.get_time(), cx);
            return cx.notify();
        }
        let time = self.player.get_time();
        if time < self.trim.0 || time >= self.trim.1 - 0.05 {
            self.player.seek(self.trim.0);
        }
        self.player.play(self.trim.1);
        cx.notify();
    }

    fn sync_capture(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let editor = &self.capture.read(cx).editor;
        let hidden_selection = editor
            .selected
            .filter(|index| !editor.get_is_layer_visible(*index))
            .and_then(|index| editor.layers[index].time);
        let mosaics = editor
            .get_timed_annotations()
            .filter(|(annotation, _)| annotation.tool == Tool::Mosaic)
            .map(|(annotation, time)| (annotation.clone(), time))
            .collect::<Vec<_>>();
        if let Some((start, _)) = hidden_selection {
            self.seek(start, cx);
        }
        if mosaics == self.mosaics {
            return;
        }
        let fonts = cx.global::<AppState>().fonts.clone();
        let annotations = mosaics.iter().map(|(annotation, time)| (annotation, *time));
        match raster::create_video_effects(self.video.size, self.canvas, annotations, &fonts) {
            Ok(effects) => self.player.set_effects(effects),
            Err(error) => self.error = Some(format!("{error:#}").into()),
        }
        self.mosaics = mosaics;
        self.await_frame();
    }

    fn get_layer_time(&self, id: u64, cx: &App) -> Option<(usize, (f64, f64))> {
        let layers = &self.capture.read(cx).editor.layers;
        let index = layers.iter().position(|layer| layer.id == id)?;
        return Some((index, layers[index].time.unwrap_or((0.0, self.duration))));
    }

    fn get_track_time(&self, position: Point<Pixels>) -> f64 {
        let x: f32 = position.x.into();
        let fraction = (x - HEADER_WIDTH - TRACK_INSET) / self.track_width;
        return fraction.clamp(0.0, 1.0) as f64 * self.duration;
    }

    fn get_track_x(&self, time: f64) -> f32 {
        return TRACK_INSET + (time / self.duration) as f32 * self.track_width;
    }

    fn begin_timeline_drag(
        &mut self,
        drag: TimelineDrag,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        if let TimelineDrag::Layer(id, _) = drag {
            self.capture
                .update(cx, |capture, cx| capture.select_layer_by_id(id, window, cx));
        }
        self.drag = Some(drag);
        if !matches!(drag, TimelineDrag::Volume(..)) {
            self.player.pause();
        }
        self.update_timeline_drag(event.position, cx);
    }

    fn finish_timeline_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.capture
                .update(cx, |capture, _| capture.editor.finish_settings_change());
        }
    }

    fn update_timeline_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else {
            return;
        };
        let time = self.get_track_time(position);
        let time = match drag {
            TimelineDrag::Resize => {
                let y: f32 = position.y.into();
                return self.resize_timeline(self.window_size.y - y, cx);
            }
            TimelineDrag::Volume(index, origin, volume) => {
                let y: f32 = position.y.into();
                let track = &mut self.audio[index];
                track.volume = (volume - (y - origin) / VOLUME_DRAG_HEIGHT).clamp(0.0, 1.0);
                track.muted = false;
                return self.sync_audio(cx);
            }
            TimelineDrag::Start => {
                self.trim.0 = time.min(self.trim.1 - MIN_CLIP).max(0.0);
                self.trim.0
            }
            TimelineDrag::End => {
                self.trim.1 = time.max(self.trim.0 + MIN_CLIP).min(self.duration);
                self.trim.1
            }
            TimelineDrag::Playhead => time,
            TimelineDrag::Layer(id, drag) => {
                let Some((index, (start, end))) = self.get_layer_time(id, cx) else {
                    return;
                };
                let (range, time) = match drag {
                    LayerDrag::Start => {
                        let start = time.min(end - MIN_CLIP).max(0.0);
                        ((start, end), start)
                    }
                    LayerDrag::End => {
                        let end = time.max(start + MIN_CLIP).min(self.duration);
                        ((start, end), end)
                    }
                    LayerDrag::Move(offset) => {
                        let length = end - start;
                        let start = (time - offset).clamp(0.0, self.duration - length);
                        ((start, start + length), time.clamp(start, start + length))
                    }
                };
                self.capture.update(cx, |capture, cx| {
                    capture.editor.set_layer_time(index, range);
                    cx.notify();
                });
                time
            }
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
                    .get_timed_annotations()
                    .map(|(annotation, time)| (annotation.clone(), time))
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
                        let annotations = annotations
                            .iter()
                            .map(|(annotation, time)| (annotation, *time));
                        raster::create_video_effects(size, canvas, annotations, &fonts)
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
                        &editor.get_audio_volumes(),
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

    fn render_timeline(&self, time: f64, playing: bool, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (trim, duration) = (self.trim, self.duration);
        let export_label: SharedString = match &self.export {
            Some(export) => {
                format!("Exporting {}%", (export.get_progress() * 100.0).round()).into()
            }
            None => "Export".into(),
        };
        let transport = h_flex()
            .flex_shrink_0()
            .h(px(TRANSPORT_HEIGHT))
            .px_2()
            .gap_2()
            .border_b_1()
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
                    .small()
                    .on_click(cx.listener(|editor, _, _, cx| editor.toggle_playback(cx))),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .font_family(theme.mono_font_family.clone())
                    .text_sm()
                    .child(
                        div()
                            .text_color(theme.primary)
                            .child(format_time(time.clamp(trim.0, trim.1) - trim.0)),
                    )
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(format!("/ {}", format_time(trim.1 - trim.0))),
                    ),
            )
            .child(div().flex_1())
            .child(
                Button::new("export")
                    .label(export_label)
                    .primary()
                    .small()
                    .disabled(self.export_task.is_some())
                    .on_click(cx.listener(|editor, _, window, cx| editor.export(window, cx))),
            );
        let step = RULER_STEPS
            .into_iter()
            .find(|step| step * self.track_width as f64 / duration >= 64.0)
            .unwrap_or(600.0);
        let mut ticks = Vec::new();
        for index in 0..=(duration / step * 5.0).floor() as usize {
            let tick = index as f64 * step / 5.0;
            let x = self.get_track_x(tick);
            let major = index % 5 == 0;
            ticks.push(
                div()
                    .absolute()
                    .left(px(x))
                    .bottom_0()
                    .w(px(1.))
                    .h(px(if major { 8. } else { 4. }))
                    .bg(theme
                        .muted_foreground
                        .opacity(if major { 0.8 } else { 0.4 })),
            );
            if major {
                let seconds = tick.round() as u64;
                ticks.push(
                    div()
                        .absolute()
                        .left(px(x + 4.0))
                        .top(px(2.))
                        .text_size(px(10.))
                        .text_color(theme.muted_foreground)
                        .child(if step < 1.0 {
                            format_time(tick)
                        } else {
                            format!("{}:{:02}", seconds / 60, seconds % 60)
                        }),
                );
            }
        }
        let ruler = h_flex()
            .flex_shrink_0()
            .h(px(RULER_HEIGHT))
            .border_b_1()
            .border_color(theme.border)
            .child(div().w(px(HEADER_WIDTH)).flex_shrink_0())
            .child(
                div()
                    .id("ruler")
                    .relative()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .children(ticks)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, event: &MouseDownEvent, window, cx| {
                            editor.begin_timeline_drag(TimelineDrag::Playhead, event, window, cx)
                        }),
                    ),
            );
        let editor = &self.capture.read(cx).editor;
        let lanes = editor
            .layers
            .iter()
            .enumerate()
            .rev()
            .map(|(index, layer)| {
                let id = layer.id;
                let range = layer.time.unwrap_or((0.0, duration));
                let selected = editor.selected == Some(index);
                let label = toolbar::get_layer_label(layer);
                let color = if layer.tool == Tool::Mosaic {
                    theme.muted_foreground
                } else {
                    rgb(layer.color.0).into()
                };
                let header = h_flex()
                    .gap_1p5()
                    .px_2()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .when(selected, |header| {
                        header.bg(theme.list_active).text_color(theme.foreground)
                    })
                    .child(Icon::new(toolbar::get_tool_spec(layer.tool).icon).xsmall())
                    .child(div().truncate().child(label.clone()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |editor, _, window, cx| {
                            editor.capture.update(cx, |capture, cx| {
                                capture.select_layer_by_id(id, window, cx)
                            })
                        }),
                    );
                let clip = self.render_clip(
                    ("clip", id as usize),
                    range,
                    color,
                    label,
                    selected,
                    Vec::new(),
                    [
                        TimelineDrag::Layer(id, LayerDrag::Start),
                        TimelineDrag::Layer(id, LayerDrag::End),
                    ],
                    move |time, _| TimelineDrag::Layer(id, LayerDrag::Move(time - range.0)),
                    cx,
                );
                self.render_track(("lane", id as usize), LANE_HEIGHT, header, clip, cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let empty = lanes.is_empty();
        let video_header = h_flex()
            .gap_1p5()
            .px_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(Icon::new(IconName::Film).xsmall())
            .child("Video");
        let clip_left = self.get_track_x(trim.0) + 1.0;
        let slots = (self.track_width / self.thumbnail_width).ceil() as usize;
        let count = self.thumbnails.len();
        let thumbnails = (0..slots)
            .filter_map(|slot| {
                let x = slot as f32 * self.thumbnail_width;
                let center = (x + self.thumbnail_width / 2.0) / self.track_width;
                let index = ((center * count as f32) as usize).min(count - 1);
                let image = self.thumbnails[index].clone()?;
                Some(
                    img(image)
                        .absolute()
                        .left(px(TRACK_INSET + x - clip_left))
                        .top_0()
                        .w(px(self.thumbnail_width))
                        .h(px(THUMBNAIL_HEIGHT))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                )
            })
            .collect();
        let video_clip = self.render_clip(
            "video-clip",
            trim,
            hsla(0.7, 0.45, 0.6, 1.0),
            format!("Recording  {}", format_time(trim.1 - trim.0)).into(),
            false,
            thumbnails,
            [TimelineDrag::Start, TimelineDrag::End],
            |_, _| TimelineDrag::Playhead,
            cx,
        );
        let audio_color = hsla(0.38, 0.45, 0.5, 1.0);
        let audio_lanes = self
            .audio
            .iter()
            .enumerate()
            .map(|(index, track)| {
                let volume = if track.muted { 0.0 } else { track.volume };
                let header = h_flex()
                    .gap_1()
                    .px_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        Button::new(("mute", index))
                            .icon(if track.muted {
                                IconName::VolumeX
                            } else {
                                IconName::Volume2
                            })
                            .tooltip(if track.muted { "Unmute" } else { "Mute" })
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(move |editor, _, _, cx| {
                                let track = &mut editor.audio[index];
                                track.muted = !track.muted;
                                editor.sync_audio(cx);
                            })),
                    )
                    .child(div().truncate().child(track.label.clone()));
                let level = div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px((1.0 - volume) * (LANE_HEIGHT - 9.0)))
                    .h(px(1.))
                    .bg(audio_color)
                    .into_any_element();
                let label = if track.muted {
                    "Muted".to_string()
                } else {
                    format!("{}%", (volume * 100.0).round())
                };
                let clip = self
                    .render_clip(
                        ("audio-clip", index),
                        trim,
                        audio_color,
                        label.into(),
                        false,
                        vec![level],
                        [TimelineDrag::Start, TimelineDrag::End],
                        move |_, y| TimelineDrag::Volume(index, y, volume),
                        cx,
                    )
                    .cursor(CursorStyle::ResizeUpDown)
                    .when(track.muted, |clip| clip.opacity(0.5));
                self.render_track(("audio-lane", index), LANE_HEIGHT, header, clip, cx)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let playhead = self.get_track_x(time);
        let dim = theme.background.opacity(0.6);
        return div()
            .absolute()
            .left_0()
            .right_0()
            .bottom_0()
            .h(px(self.timeline_height))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .child(
                div()
                    .id("timeline-splitter")
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(-3.))
                    .h(px(6.))
                    .cursor(CursorStyle::ResizeUpDown)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, _, _, cx| {
                            cx.stop_propagation();
                            editor.drag = Some(TimelineDrag::Resize);
                        }),
                    ),
            )
            .child(transport)
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(HEADER_WIDTH))
                            .bg(theme.tab_bar)
                            .border_r_1()
                            .border_color(theme.border),
                    )
                    .child(ruler)
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .child(
                                div()
                                    .id("annotation-lanes")
                                    .flex()
                                    .flex_col()
                                    .size_full()
                                    .overflow_y_scroll()
                                    .track_scroll(&self.lanes_scroll)
                                    .when(empty, |lanes| {
                                        lanes.child(
                                            div()
                                                .pl(px(HEADER_WIDTH + TRACK_INSET))
                                                .pt_2()
                                                .text_xs()
                                                .text_color(theme.muted_foreground)
                                                .child("Annotations appear here as tracks"),
                                        )
                                    })
                                    .children(lanes),
                            )
                            .child(div().absolute().inset_0().child(
                                Scrollbar::vertical(&self.lanes_scroll).mode(ScrollbarMode::Always),
                            )),
                    )
                    .child(self.render_track(
                        "video-lane",
                        VIDEO_LANE_HEIGHT,
                        video_header,
                        video_clip,
                        cx,
                    ))
                    .children(audio_lanes)
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(px(HEADER_WIDTH))
                            .right_0()
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left_0()
                                    .w(px(self.get_track_x(trim.0)))
                                    .bg(dim),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(px(self.get_track_x(trim.1)))
                                    .right_0()
                                    .bg(dim),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(px(playhead - 0.5))
                                    .w(px(1.))
                                    .bg(theme.primary),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(px(playhead - 5.5))
                                    .w(px(11.))
                                    .h(px(12.))
                                    .rounded_b(px(3.))
                                    .bg(theme.primary),
                            ),
                    ),
            );
    }

    fn render_track(
        &self,
        id: impl Into<ElementId>,
        height: f32,
        header: Div,
        clip: impl IntoElement,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        return h_flex()
            .flex_shrink_0()
            .h(px(height))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(header.w(px(HEADER_WIDTH)).h_full().flex_shrink_0())
            .child(
                div()
                    .id(id)
                    .relative()
                    .flex_1()
                    .h_full()
                    .overflow_hidden()
                    .child(clip)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|editor, event: &MouseDownEvent, window, cx| {
                            editor.begin_timeline_drag(TimelineDrag::Playhead, event, window, cx)
                        }),
                    ),
            );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_clip(
        &self,
        id: impl Into<ElementId>,
        (start, end): (f64, f64),
        color: Hsla,
        label: SharedString,
        selected: bool,
        content: Vec<AnyElement>,
        [start_drag, end_drag]: [TimelineDrag; 2],
        body_drag: impl Fn(f64, f32) -> TimelineDrag + 'static,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let theme = cx.theme();
        let left = self.get_track_x(start);
        let handle_color = if selected { theme.foreground } else { color };
        let has_content = !content.is_empty();
        let hover_color = theme.foreground.opacity(0.7);
        let handle = |id: &'static str, drag: TimelineDrag| {
            div()
                .id(id)
                .absolute()
                .top_0()
                .bottom_0()
                .w(px(HANDLE_WIDTH))
                .bg(handle_color)
                .hover(|style| style.bg(hover_color))
                .cursor(CursorStyle::ResizeLeftRight)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |editor, event: &MouseDownEvent, window, cx| {
                        editor.begin_timeline_drag(drag, event, window, cx)
                    }),
                )
        };
        return div()
            .id(id)
            .absolute()
            .left(px(left))
            .w(px((self.get_track_x(end) - left).max(2.0 * HANDLE_WIDTH)))
            .top(px(3.))
            .bottom(px(3.))
            .flex()
            .items_center()
            .px(px(HANDLE_WIDTH + 4.0))
            .rounded(px(4.))
            .overflow_hidden()
            .bg(color.opacity(if selected { 0.5 } else { 0.3 }))
            .border_1()
            .border_color(handle_color)
            .children(content)
            .child(
                div()
                    .relative()
                    .truncate()
                    .text_xs()
                    .text_color(theme.foreground)
                    .when(has_content, |label| {
                        label
                            .px_1()
                            .rounded(px(3.))
                            .bg(theme.background.opacity(0.7))
                    })
                    .child(label),
            )
            .child(handle("start", start_drag).left_0())
            .child(handle("end", end_drag).right_0())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |editor, event: &MouseDownEvent, window, cx| {
                    let drag = body_drag(
                        editor.get_track_time(event.position),
                        event.position.y.into(),
                    );
                    editor.begin_timeline_drag(drag, event, window, cx)
                }),
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
                self.load_audio(cx);
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
        if playing {
            self.sync_time(time, cx);
        }
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
                cx.listener(|editor, _, _, cx| editor.finish_timeline_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|editor, _, _, cx| editor.finish_timeline_drag(cx)),
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
                    .bottom(px(self.timeline_height))
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
                        .bottom(px(self.timeline_height + 16.0))
                        .px_2()
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            });
    }
}
