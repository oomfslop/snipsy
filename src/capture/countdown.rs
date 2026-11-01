use crate::{geometry::Rect, platform, ui};
use glam::{Vec2, vec2};
use gpui_kit::*;
use std::time::{Duration, Instant};

const SIZE: f32 = 72.0;
const MARGIN: f32 = 16.0;
const TICK: Duration = Duration::from_millis(50);

struct Countdown {
    seconds: u32,
}

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        return div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_2xl()
            .bg(rgba(0x00000099))
            .text_color(white())
            .text_3xl()
            .font_weight(FontWeight::BOLD)
            .child(self.seconds.to_string());
    }
}

fn get_seconds(left: Duration) -> u32 {
    return left.as_secs_f32().ceil() as u32;
}

pub fn run(
    delay: Duration,
    cx: &mut App,
    done: impl FnOnce(Option<AnyWindowHandle>, &mut App) + 'static,
) {
    let deadline = Instant::now() + delay;
    let pointer = platform::get_pointer_position();
    let screens = platform::get_visible_screen_bounds();
    let corners = screens
        .iter()
        .find(|screen| screen.contains(pointer))
        .or(screens.first())
        .map(|screen| {
            let size = Vec2::splat(SIZE);
            [
                vec2(screen.end().x - SIZE - MARGIN, screen.pos.y + MARGIN),
                vec2(screen.pos.x + MARGIN, screen.end().y - SIZE - MARGIN),
            ]
            .map(|pos| Rect { pos, size })
        });
    let view = corners.and_then(|corners| {
        let seconds = get_seconds(delay);
        let view = ui::open_overlay(corners[0], false, cx, |_, cx| {
            cx.new(|_| Countdown { seconds })
        })
        .inspect_err(|error| eprintln!("could not show countdown: {error:#}"))
        .ok()?;
        let _ = view.update(cx, |_, window, _| {
            platform::set_window_click_through(window, true);
            platform::set_window_visible(window, true);
        });
        Some((view, corners))
    });
    cx.spawn(async move |cx| {
        let mut corner = 0;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            if let Some((view, corners)) = &view {
                cx.update(|cx| {
                    let _ = view.update(cx, |countdown, window, cx| {
                        let seconds = get_seconds(left);
                        if countdown.seconds != seconds {
                            countdown.seconds = seconds;
                            cx.notify();
                        }
                        if corners[corner].contains(platform::get_pointer_position()) {
                            corner = 1 - corner;
                            platform::set_window_bounds(window, corners[corner]);
                        }
                    });
                });
            }
            cx.background_executor().timer(left.min(TICK)).await;
        }
        cx.update(|cx| done(view.map(|(view, _)| view.window), cx));
    })
    .detach();
}
