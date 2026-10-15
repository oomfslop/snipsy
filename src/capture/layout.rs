use crate::geometry::Rect;
use glam::{Vec2, vec2};

pub const MARGIN: f32 = 8.0;
pub const GAP: f32 = 4.0;
const HANDLE_HIT_RADIUS: f32 = 4.0;
const MIN_SELECTION: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

pub fn get_resize_handle_centers(selection: Rect) -> [(ResizeHandle, Vec2); 8] {
    let start = selection.pos;
    let center = selection.pos + selection.size / 2.0;
    let end = selection.end();
    return [
        (ResizeHandle::TopLeft, start),
        (ResizeHandle::Top, vec2(center.x, start.y)),
        (ResizeHandle::TopRight, vec2(end.x, start.y)),
        (ResizeHandle::Right, vec2(end.x, center.y)),
        (ResizeHandle::BottomRight, end),
        (ResizeHandle::Bottom, vec2(center.x, end.y)),
        (ResizeHandle::BottomLeft, vec2(start.x, end.y)),
        (ResizeHandle::Left, vec2(start.x, center.y)),
    ];
}

pub fn get_resize_handle(point: Vec2, selection: Rect) -> Option<ResizeHandle> {
    let slop = Vec2::splat(HANDLE_HIT_RADIUS);
    if !point.cmpge(selection.pos - slop).all() || !point.cmple(selection.end() + slop).all() {
        return None;
    }
    let start = (point - selection.pos).abs();
    let end = (point - selection.end()).abs();
    let horizontal = (start.x.min(end.x) <= HANDLE_HIT_RADIUS).then_some(start.x <= end.x);
    let vertical = (start.y.min(end.y) <= HANDLE_HIT_RADIUS).then_some(start.y <= end.y);
    return match (horizontal, vertical) {
        (Some(true), Some(true)) => Some(ResizeHandle::TopLeft),
        (Some(false), Some(true)) => Some(ResizeHandle::TopRight),
        (Some(true), Some(false)) => Some(ResizeHandle::BottomLeft),
        (Some(false), Some(false)) => Some(ResizeHandle::BottomRight),
        (Some(true), None) => Some(ResizeHandle::Left),
        (Some(false), None) => Some(ResizeHandle::Right),
        (None, Some(true)) => Some(ResizeHandle::Top),
        (None, Some(false)) => Some(ResizeHandle::Bottom),
        (None, None) => None,
    };
}

pub fn create_resized_selection(origin: Rect, handle: ResizeHandle, point: Vec2) -> Rect {
    let start = origin.pos;
    let end = origin.end();
    let (anchor, point) = match handle {
        ResizeHandle::TopLeft => (end, point),
        ResizeHandle::TopRight => (vec2(start.x, end.y), point),
        ResizeHandle::BottomRight => (start, point),
        ResizeHandle::BottomLeft => (vec2(end.x, start.y), point),
        ResizeHandle::Top => (vec2(start.x, end.y), vec2(end.x, point.y)),
        ResizeHandle::Bottom => (start, vec2(end.x, point.y)),
        ResizeHandle::Left => (vec2(end.x, start.y), vec2(point.x, end.y)),
        ResizeHandle::Right => (start, vec2(point.x, end.y)),
    };
    return Rect::from_corners(anchor, point);
}

pub fn create_moved_selection(origin: Rect, delta: Vec2, bounds: Vec2) -> Rect {
    return Rect {
        pos: (origin.pos + delta).clamp(Vec2::ZERO, (bounds - origin.size).max(Vec2::ZERO)),
        size: origin.size,
    };
}

pub fn create_keyboard_resized_selection(
    mut selection: Rect,
    key: &str,
    grow: bool,
    bounds: Vec2,
) -> Rect {
    let amount = if grow { 1.0 } else { -1.0 };
    match key {
        "left" => {
            let next = (selection.pos.x - amount).clamp(0.0, selection.end().x - MIN_SELECTION);
            selection.size.x += selection.pos.x - next;
            selection.pos.x = next;
        }
        "right" => {
            selection.size.x = (selection.size.x + amount).clamp(
                MIN_SELECTION,
                (bounds.x - selection.pos.x).max(MIN_SELECTION),
            );
        }
        "up" => {
            let next = (selection.pos.y - amount).clamp(0.0, selection.end().y - MIN_SELECTION);
            selection.size.y += selection.pos.y - next;
            selection.pos.y = next;
        }
        _ => {
            selection.size.y = (selection.size.y + amount).clamp(
                MIN_SELECTION,
                (bounds.y - selection.pos.y).max(MIN_SELECTION),
            );
        }
    }
    return selection;
}

pub fn get_is_valid_selection(selection: Rect) -> bool {
    return selection.size.min_element() >= MIN_SELECTION;
}

pub fn get_dim_rects(selection: Option<Rect>, screen: Vec2) -> Vec<Rect> {
    let Some(rect) = selection else {
        return vec![Rect {
            pos: Vec2::ZERO,
            size: screen,
        }];
    };
    let end = rect.end();
    return vec![
        Rect::from_corners(Vec2::ZERO, vec2(screen.x, rect.pos.y)),
        Rect::from_corners(vec2(0.0, end.y), screen.max(end)),
        Rect::from_corners(vec2(0.0, rect.pos.y), vec2(rect.pos.x, end.y)),
        Rect::from_corners(vec2(end.x, rect.pos.y), vec2(screen.x.max(end.x), end.y)),
    ];
}

pub fn create_action_bar_rect(selection: Rect, size: Vec2, screen: Vec2) -> Rect {
    let size = size.min((screen - 2.0 * MARGIN).max(Vec2::ONE));
    let x = (selection.end().x - size.x).clamp(MARGIN, (screen.x - size.x - MARGIN).max(MARGIN));
    let below = selection.end().y + GAP;
    let above = selection.pos.y - size.y - GAP;
    let y = if below + size.y <= screen.y - MARGIN {
        below
    } else if above >= MARGIN {
        above
    } else {
        (selection.end().y - size.y - GAP).clamp(MARGIN, (screen.y - size.y - MARGIN).max(MARGIN))
    };
    return Rect {
        pos: vec2(x, y),
        size,
    };
}

pub fn get_options_bar_y(bar: Rect, screen: Vec2) -> f32 {
    if bar.end().y + GAP + bar.size.y <= screen.y - MARGIN {
        return bar.end().y + GAP;
    }
    return (bar.pos.y - bar.size.y - GAP).max(MARGIN);
}

pub fn create_layers_panel_rect(selection: Rect, bar: Rect, size: Vec2, screen: Vec2) -> Rect {
    let size = size.min((screen - 2.0 * MARGIN).max(Vec2::ZERO));
    let right = selection.end().x.max(bar.end().x) + GAP;
    let left = selection.pos.x.min(bar.pos.x) - size.x - GAP;
    let below = selection.end().y + GAP;
    let above = selection.pos.y.min(bar.pos.y - bar.size.y - GAP) - size.y - GAP;
    let beside_bar = bar.pos.x - size.x - GAP;
    let pos = if right + size.x <= screen.x - MARGIN {
        vec2(right, selection.pos.y)
    } else if left >= MARGIN {
        vec2(left, selection.pos.y)
    } else if below + size.y <= screen.y - MARGIN && beside_bar >= MARGIN {
        vec2(beside_bar, below)
    } else if above >= MARGIN {
        vec2(selection.pos.x, above)
    } else if beside_bar >= MARGIN {
        vec2(beside_bar, bar.pos.y)
    } else if bar.end().x + GAP + size.x + MARGIN <= screen.x {
        vec2(bar.end().x + GAP, bar.pos.y)
    } else {
        vec2(selection.pos.x, bar.pos.y - size.y - GAP)
    };
    return Rect {
        pos: pos.clamp(
            Vec2::splat(MARGIN),
            (screen - size - MARGIN).max(Vec2::splat(MARGIN)),
        ),
        size,
    };
}

pub fn get_capture_history_index(
    history: &[Rect],
    current: usize,
    direction: i32,
    bounds: Rect,
) -> Option<usize> {
    let length = history.len() as i32;
    return (1..length)
        .map(|offset| (current as i32 + direction * offset).rem_euclid(length) as usize)
        .find(|index| history[*index] == bounds);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyboard_resize_grows_and_shrinks_requested_edges() {
        let selection = Rect {
            pos: vec2(10.0, 20.0),
            size: vec2(30.0, 40.0),
        };
        assert_eq!(
            create_keyboard_resized_selection(selection, "left", true, vec2(100.0, 100.0)),
            Rect {
                pos: vec2(9.0, 20.0),
                size: vec2(31.0, 40.0)
            }
        );
        assert_eq!(
            create_keyboard_resized_selection(selection, "down", false, vec2(100.0, 100.0)).size,
            vec2(30.0, 39.0)
        );
        let minimum = Rect {
            pos: Vec2::ZERO,
            size: vec2(2.0, 2.0),
        };
        assert_eq!(
            create_keyboard_resized_selection(minimum, "right", false, vec2(2.0, 2.0)),
            minimum
        );
    }

    #[test]
    fn capture_history_wraps_and_skips_other_displays() {
        let display = Rect {
            pos: Vec2::ZERO,
            size: vec2(1440.0, 900.0),
        };
        let other = Rect {
            pos: vec2(1440.0, 0.0),
            size: vec2(1920.0, 1080.0),
        };
        let history = [display, other, display];
        assert_eq!(get_capture_history_index(&history, 0, 1, display), Some(2));
        assert_eq!(get_capture_history_index(&history, 2, -1, display), Some(0));
        assert_eq!(get_capture_history_index(&history, 2, 1, display), Some(0));
        assert_eq!(
            get_capture_history_index(&history[..2], 0, 1, display),
            None
        );
        assert_eq!(get_capture_history_index(&[], 0, 1, display), None);
    }

    #[test]
    fn layers_panel_prefers_sides_and_stays_on_screen() {
        let screen = vec2(1440.0, 900.0);
        let bar_size = vec2(420.0, 32.0);
        let panel_size = vec2(176.0, 300.0);
        for (selection, right) in [
            (
                Rect {
                    pos: vec2(200.0, 150.0),
                    size: vec2(500.0, 400.0),
                },
                true,
            ),
            (
                Rect {
                    pos: vec2(850.0, 150.0),
                    size: vec2(550.0, 400.0),
                },
                false,
            ),
        ] {
            let bar = create_action_bar_rect(selection, bar_size, screen);
            let panel = create_layers_panel_rect(selection, bar, panel_size, screen);
            if right {
                assert!(panel.pos.x >= selection.end().x + GAP);
            } else {
                assert!(panel.end().x <= selection.pos.x - GAP);
            }
            assert!(panel.pos.cmpge(Vec2::splat(MARGIN)).all());
            assert!(panel.end().cmple(screen - MARGIN).all());
        }
        for screen in [screen, vec2(200.0, 150.0)] {
            let selection = Rect {
                pos: Vec2::ZERO,
                size: screen,
            };
            let bar = create_action_bar_rect(selection, bar_size, screen);
            let panel = create_layers_panel_rect(selection, bar, panel_size, screen);
            assert!(panel.pos.cmpge(Vec2::splat(MARGIN)).all());
            assert!(panel.end().cmple(screen - MARGIN).all());
            assert!(bar.pos.x >= MARGIN && bar.end().x <= screen.x - MARGIN);
        }
    }

    #[test]
    fn resize_hit_areas_cover_edges_with_slop_and_choose_nearest_corners() {
        let rect = Rect {
            pos: vec2(100.0, 100.0),
            size: vec2(300.0, 200.0),
        };
        for (point, expected) in [
            (vec2(170.0, 96.0), Some(ResizeHandle::Top)),
            (vec2(170.0, 104.0), Some(ResizeHandle::Top)),
            (vec2(170.0, 304.0), Some(ResizeHandle::Bottom)),
            (vec2(96.0, 170.0), Some(ResizeHandle::Left)),
            (vec2(404.0, 170.0), Some(ResizeHandle::Right)),
            (vec2(96.0, 96.0), Some(ResizeHandle::TopLeft)),
            (vec2(404.0, 304.0), Some(ResizeHandle::BottomRight)),
            (vec2(170.0, 95.0), None),
            (vec2(170.0, 105.0), None),
            (vec2(200.0, 200.0), None),
        ] {
            assert_eq!(get_resize_handle(point, rect), expected);
        }
        let tiny = Rect {
            pos: vec2(100.0, 100.0),
            size: vec2(8.0, 8.0),
        };
        assert_eq!(
            get_resize_handle(vec2(108.0, 100.0), tiny),
            Some(ResizeHandle::TopRight)
        );
        assert_eq!(
            get_resize_handle(vec2(108.0, 108.0), tiny),
            Some(ResizeHandle::BottomRight)
        );
    }

    #[test]
    fn resizing_crosses_edges_without_jumping() {
        let origin = Rect {
            pos: vec2(20.0, 30.0),
            size: vec2(100.0, 80.0),
        };
        let resized = create_resized_selection(origin, ResizeHandle::Top, vec2(70.0, 50.0));
        assert_eq!(resized.pos, vec2(20.0, 50.0));
        assert_eq!(resized.size, vec2(100.0, 60.0));
        let flipped = create_resized_selection(origin, ResizeHandle::Top, vec2(70.0, 140.0));
        assert_eq!(flipped.pos, vec2(20.0, 110.0));
        assert_eq!(flipped.size, vec2(100.0, 30.0));
        let corner = create_resized_selection(origin, ResizeHandle::TopLeft, vec2(0.0, 0.0));
        assert_eq!(corner, Rect::from_corners(Vec2::ZERO, origin.end()));
    }

    #[test]
    fn dimming_covers_everything_but_the_selection() {
        let screen = vec2(1440.0, 900.0);
        assert_eq!(
            get_dim_rects(None, screen),
            vec![Rect {
                pos: Vec2::ZERO,
                size: screen
            }]
        );
        let selection = Rect {
            pos: vec2(200.0, 100.0),
            size: vec2(300.0, 200.0),
        };
        let dim = get_dim_rects(Some(selection), screen);
        assert!(!dim.iter().any(|rect| rect.contains(vec2(350.0, 200.0))));
        assert_eq!(
            dim.iter()
                .map(|rect| rect.size.x * rect.size.y)
                .sum::<f32>(),
            screen.x * screen.y - 300.0 * 200.0
        );
    }
}
