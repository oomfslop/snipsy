use crate::geometry::Rect;
use glam::{Vec2, vec2};

pub const MARGIN: f32 = 8.0;
pub const GAP: f32 = 4.0;
const HANDLE_HIT_RADIUS: f32 = 4.0;
pub const MIN_SELECTION: f32 = 2.0;

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

pub fn create_moved_selection(origin: Rect, delta: Vec2, bounds: Rect) -> Rect {
    return Rect {
        pos: (origin.pos + delta).clamp(bounds.pos, (bounds.end() - origin.size).max(bounds.pos)),
        size: origin.size,
    };
}

pub fn create_keyboard_resized_selection(
    mut selection: Rect,
    key: &str,
    grow: bool,
    bounds: Rect,
) -> Rect {
    let amount = if grow { 1.0 } else { -1.0 };
    match key {
        "left" => {
            let next =
                (selection.pos.x - amount).clamp(bounds.pos.x, selection.end().x - MIN_SELECTION);
            selection.size.x += selection.pos.x - next;
            selection.pos.x = next;
        }
        "right" => {
            selection.size.x = (selection.size.x + amount).clamp(
                MIN_SELECTION,
                (bounds.end().x - selection.pos.x).max(MIN_SELECTION),
            );
        }
        "up" => {
            let next =
                (selection.pos.y - amount).clamp(bounds.pos.y, selection.end().y - MIN_SELECTION);
            selection.size.y += selection.pos.y - next;
            selection.pos.y = next;
        }
        _ => {
            selection.size.y = (selection.size.y + amount).clamp(
                MIN_SELECTION,
                (bounds.end().y - selection.pos.y).max(MIN_SELECTION),
            );
        }
    }
    return selection;
}

pub fn get_is_valid_selection(selection: Rect) -> bool {
    return selection.size.min_element() >= MIN_SELECTION;
}

pub fn get_dim_rects(selection: Option<Rect>, bounds: Rect) -> Vec<Rect> {
    let Some(rect) = selection else {
        return vec![bounds];
    };
    let (start, screen) = (bounds.pos, bounds.end());
    let end = rect.end();
    return vec![
        Rect::from_corners(start, vec2(screen.x, rect.pos.y)),
        Rect::from_corners(vec2(start.x, end.y), screen.max(end)),
        Rect::from_corners(vec2(start.x, rect.pos.y), vec2(rect.pos.x, end.y)),
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

pub fn create_centered_rect(target: Rect, size: Vec2, screen: Vec2) -> Rect {
    let pos = (target.pos + (target.size - size) / 2.0).clamp(
        Vec2::splat(MARGIN),
        (screen - size - MARGIN).max(Vec2::splat(MARGIN)),
    );
    return Rect { pos, size };
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
