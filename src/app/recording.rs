use crate::{
    app::{self, AppState, config::Config, tray},
    capture::{self, layout},
    geometry::Rect,
    platform::{self, ScreenRecorder},
    ui::{self, View},
};
use anyhow::{Result, anyhow, ensure};
use gpui_kit::{component::ActiveTheme as _, *};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
}

impl VideoCodec {
    pub const ALL: [Self; 2] = [Self::H264, Self::Hevc];

    pub fn get_label(self) -> &'static str {
        return match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
        };
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioSource {
    System,
    Microphone,
}

impl AudioSource {
    pub fn get_label(self) -> &'static str {
        return match self {
            Self::System => "System audio",
            Self::Microphone => "Microphone",
        };
    }
}

pub const FPS_OPTIONS: [u32; 3] = [15, 30, 60];
pub const BITRATE_OPTIONS: [u32; 5] = [2, 5, 10, 20, 40];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub codec: VideoCodec,
    pub fps: u32,
    pub bitrate_mbps: u32,
    pub system_audio: bool,
    pub microphone: bool,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        return Self {
            codec: VideoCodec::H264,
            fps: 60,
            bitrate_mbps: 10,
            system_audio: true,
            microphone: false,
        };
    }
}

impl RecordingSettings {
    pub fn validate(self) -> Result<()> {
        ensure!(
            (1..=120).contains(&self.fps),
            "Recording frame rate must be between 1 and 120"
        );
        ensure!(
            (1..=500).contains(&self.bitrate_mbps),
            "Recording bitrate must be between 1 and 500 Mbps"
        );
        return Ok(());
    }
}

#[derive(Default)]
pub enum Recording {
    #[default]
    Idle,
    Starting(View<RegionOverlay>),
    Active {
        recorder: ScreenRecorder,
        overlay: View<RegionOverlay>,
    },
    Stopping,
}

impl Global for Recording {}

pub fn toggle(cx: &mut App) {
    match cx.global::<Recording>() {
        Recording::Idle => capture::start(Default::default(), cx),
        Recording::Active { .. } => stop(None, cx),
        Recording::Starting(_) | Recording::Stopping => {}
    }
}

pub fn start(bounds: Rect, cx: &mut App) {
    if !matches!(cx.global::<Recording>(), Recording::Idle) {
        app::show_error(anyhow!("A screen recording is already active"));
        return;
    }
    let overlay = match open_overlay(bounds, cx) {
        Ok(overlay) => overlay,
        Err(error) => return app::show_error(error),
    };
    let overlay_id = overlay
        .update(cx, |_, window, _| platform::get_window_id(window))
        .unwrap_or_default();
    cx.set_global(Recording::Starting(overlay));
    let (sender, receiver) = async_channel::bounded(1);
    let (error_sender, errors) = async_channel::bounded(1);
    platform::start_screen_recording(
        bounds,
        cx.global::<Config>().recording,
        overlay_id,
        sender,
        error_sender,
    );
    cx.spawn(async move |cx| {
        let result = receiver
            .recv()
            .await
            .unwrap_or_else(|error| Err(error.into()));
        cx.update(|cx| {
            let Recording::Starting(overlay) = std::mem::take(cx.global_mut::<Recording>()) else {
                unreachable!("recording state changed while starting");
            };
            let recorder = match result {
                Ok(recorder) => recorder,
                Err(error) => {
                    close_overlay(overlay, cx);
                    return app::show_error(error);
                }
            };
            cx.set_global(Recording::Active { recorder, overlay });
            tray::set_recording(true, cx);
            cx.spawn(async move |cx| {
                if let Ok(error) = errors.recv().await {
                    cx.update(|cx| {
                        if matches!(cx.global::<Recording>(), Recording::Active { .. }) {
                            stop(Some(anyhow!(error)), cx);
                        }
                    });
                }
            })
            .detach();
        });
    })
    .detach();
}

fn stop(error: Option<anyhow::Error>, cx: &mut App) {
    let Recording::Active { recorder, overlay } =
        std::mem::replace(cx.global_mut::<Recording>(), Recording::Stopping)
    else {
        unreachable!("only an active recording can stop");
    };
    close_overlay(overlay, cx);
    tray::set_recording(false, cx);
    let (sender, receiver) = async_channel::bounded(1);
    platform::stop_screen_recording(recorder, sender);
    cx.spawn(async move |cx| {
        let result = receiver
            .recv()
            .await
            .unwrap_or_else(|error| Err(error.into()));
        cx.update(|cx| {
            cx.set_global(Recording::Idle);
            if let Some(error) = error {
                app::show_error(error);
            }
            match result {
                Ok(video) => capture::open_video_editor(video, cx),
                Err(error) => app::show_error(error),
            }
        });
    })
    .detach();
}

pub fn quit(cx: &mut App) {
    let unsaved = !matches!(cx.global::<Recording>(), Recording::Idle)
        || cx
            .global::<AppState>()
            .windows
            .iter()
            .any(|(key, _)| *key == capture::VIDEO_EDITOR_WINDOW);
    if unsaved
        && !platform::confirm(
            "Quit Snipsy?",
            "Recordings that have not been exported will be lost.",
            "Quit",
        )
    {
        return;
    }
    cx.quit();
}

pub struct RegionOverlay {
    region: Rect,
}

impl Render for RegionOverlay {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let region = self.region;
        let accent = cx.theme().primary;
        return canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                for rect in layout::get_dim_rects(Some(region), ui::to_rect(bounds)) {
                    window.paint_quad(fill(ui::to_bounds(rect), rgba(0x00000066)));
                }
                window.paint_quad(outline(
                    ui::to_bounds(Rect {
                        pos: region.pos - 1.0,
                        size: region.size + 2.0,
                    }),
                    accent,
                    BorderStyle::Solid,
                ));
            },
        )
        .size_full();
    }
}

fn open_overlay(region: Rect, cx: &mut App) -> Result<View<RegionOverlay>> {
    let desktop = platform::get_desktop_bounds()?;
    let region = Rect {
        pos: region.pos - desktop.pos,
        size: region.size,
    };
    let overlay = ui::open_overlay(desktop, false, cx, |_, cx| {
        cx.new(|_| RegionOverlay { region })
    })?;
    overlay.update(cx, |_, window, _| {
        platform::set_window_click_through(window, true);
        platform::set_window_visible(window, true);
    })?;
    return Ok(overlay);
}

fn close_overlay(overlay: View<RegionOverlay>, cx: &mut App) {
    let _ = overlay.update(cx, |_, window, _| window.remove_window());
}
