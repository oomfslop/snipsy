use crate::{
    app::{self, tray},
    capture,
    geometry::Rect,
    platform::{self, ScreenRecorder},
};
use anyhow::anyhow;
use gpui_kit::{App, Global};

#[derive(Default)]
pub enum Recording {
    #[default]
    Idle,
    Starting {
        quit_after: bool,
    },
    Active(ScreenRecorder),
    Stopping {
        quit_after: bool,
    },
}

impl Global for Recording {}

pub fn toggle(cx: &mut App) {
    match cx.global::<Recording>() {
        Recording::Idle => capture::start(cx),
        Recording::Active(_) => stop(None, false, cx),
        Recording::Starting { .. } | Recording::Stopping { .. } => {}
    }
}

pub fn start(bounds: Rect, cx: &mut App) {
    if !matches!(cx.global::<Recording>(), Recording::Idle) {
        app::show_error(anyhow!("A screen recording is already active"));
        return;
    }
    cx.set_global(Recording::Starting { quit_after: false });
    let path = platform::prompt_save_recording();
    cx.spawn(async move |cx| {
        let path = match path.recv().await.unwrap_or_else(|error| Err(error.into())) {
            Ok(Some(path)) => path,
            Ok(None) => return cx.update(|cx| finish_start(None, cx)),
            Err(error) => return cx.update(|cx| finish_start(Some(error), cx)),
        };
        let (sender, receiver) = async_channel::bounded(1);
        let (error_sender, errors) = async_channel::bounded(1);
        platform::start_screen_recording(bounds, path, sender, error_sender);
        let result = receiver
            .recv()
            .await
            .unwrap_or_else(|error| Err(error.into()));
        cx.update(|cx| match result {
            Ok(recorder) => {
                let Recording::Starting { quit_after } =
                    std::mem::replace(cx.global_mut::<Recording>(), Recording::Active(recorder))
                else {
                    unreachable!("recording state changed while starting");
                };
                tray::set_recording(true, cx);
                if quit_after {
                    return stop(None, true, cx);
                }
                cx.spawn(async move |cx| {
                    if let Ok(error) = errors.recv().await {
                        cx.update(|cx| {
                            if matches!(cx.global::<Recording>(), Recording::Active(_)) {
                                stop(Some(anyhow!(error)), false, cx);
                            }
                        });
                    }
                })
                .detach();
            }
            Err(error) => finish_start(Some(error), cx),
        });
    })
    .detach();
}

fn finish_start(error: Option<anyhow::Error>, cx: &mut App) {
    let Recording::Starting { quit_after } = std::mem::take(cx.global_mut::<Recording>()) else {
        unreachable!("recording state changed while starting");
    };
    report_error(error, quit_after);
    if quit_after {
        cx.quit();
    }
}

fn report_error(error: Option<anyhow::Error>, quitting: bool) {
    match error {
        Some(error) if quitting => eprintln!("could not record: {error:#}"),
        Some(error) => app::show_error(error),
        None => {}
    }
}

fn stop(error: Option<anyhow::Error>, quit_after: bool, cx: &mut App) {
    let Recording::Active(recorder) = std::mem::replace(
        cx.global_mut::<Recording>(),
        Recording::Stopping { quit_after },
    ) else {
        unreachable!("only an active recording can stop");
    };
    let (sender, receiver) = async_channel::bounded(1);
    platform::stop_screen_recording(recorder, sender);
    cx.spawn(async move |cx| {
        let result = receiver
            .recv()
            .await
            .unwrap_or_else(|error| Err(error.into()));
        cx.update(|cx| {
            let Recording::Stopping { quit_after } = std::mem::take(cx.global_mut::<Recording>())
            else {
                unreachable!("recording state changed while stopping");
            };
            tray::set_recording(false, cx);
            report_error(result.err().or(error), quit_after);
            if quit_after {
                cx.quit();
            }
        });
    })
    .detach();
}

/// Quits after finalizing an active recording so the file is not left truncated.
pub fn quit(cx: &mut App) {
    match cx.global_mut::<Recording>() {
        Recording::Active(_) => stop(None, true, cx),
        Recording::Starting { quit_after } | Recording::Stopping { quit_after } => {
            *quit_after = true
        }
        Recording::Idle => cx.quit(),
    }
}
