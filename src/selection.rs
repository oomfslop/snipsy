use std::time::{SystemTime, UNIX_EPOCH};

use glam::{Vec2, vec2};
use nekoframe::prelude::*;

use crate::{app::App, lens};

const ACTION_BAR_WIDTH: f32 = 88.0;
const ACTION_BAR_HEIGHT: f32 = 44.0;
const ACTION_BAR_GAP: f32 = 8.0;
const SCREEN_MARGIN: f32 = 8.0;
const HANDLE_RADIUS: f32 = 5.0;
const HANDLE_HIT_RADIUS: f32 = 10.0;

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    New,
    Move,
    Resize(ResizeHandle),
}

#[derive(Clone, Copy, PartialEq)]
enum ResizeHandle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

pub struct SelectionState {
    size: Vec2,
    scale_factor: f64,
    monitor: Option<Monitor>,
    pointer: Vec2,
    start: Vec2,
    selection: Option<Rect>,
    drag_origin: Option<Rect>,
    drag_mode: Option<DragMode>,
    pending_action: Option<SelectionAction>,
    revision: u64,
}

impl SelectionState {
    pub fn new() -> Self {
        return Self {
            size: vec2(800.0, 600.0),
            scale_factor: 1.0,
            monitor: None,
            pointer: Vec2::ZERO,
            start: Vec2::ZERO,
            selection: None,
            drag_origin: None,
            drag_mode: None,
            pending_action: None,
            revision: 0,
        };
    }

    fn reset_for_activation(&mut self, monitor: Monitor) {
        self.size = vec2(monitor.size.width as f32, monitor.size.height as f32);
        self.monitor = Some(monitor);
        self.pointer = Vec2::ZERO;
        self.selection = None;
        self.drag_origin = None;
        self.drag_mode = None;
        self.pending_action = None;
        self.revision += 1;
    }

    fn begin_drag(&mut self) {
        if let Some(selection) = self.selection {
            let action_bar = create_action_bar_rect(selection, self.size);
            if create_search_button_rect(action_bar).intersects(self.pointer) {
                self.pending_action = Some(SelectionAction::Search(selection));
                return;
            }
            if create_cancel_button_rect(action_bar).intersects(self.pointer) {
                self.pending_action = Some(SelectionAction::Cancel);
                return;
            }
            if let Some(handle) = get_resize_handle(self.pointer, selection) {
                self.start = self.pointer;
                self.drag_origin = Some(selection);
                self.drag_mode = Some(DragMode::Resize(handle));
                return;
            }
            if selection.intersects(self.pointer) {
                self.start = self.pointer;
                self.drag_origin = Some(selection);
                self.drag_mode = Some(DragMode::Move);
                return;
            }
        }

        self.pointer = clamp_point(self.pointer, self.size);
        self.start = self.pointer;
        self.selection = None;
        self.drag_origin = None;
        self.drag_mode = Some(DragMode::New);
    }

    fn update_drag(&mut self) {
        let point = clamp_point(self.pointer, self.size);
        match self.drag_mode {
            Some(DragMode::New) => self.selection = Some(create_selection_rect(self.start, point)),
            Some(DragMode::Move) => {
                if let Some(origin) = self.drag_origin {
                    self.selection = Some(create_moved_selection(origin, self.start, point, self.size));
                }
            }
            Some(DragMode::Resize(handle)) => {
                if let Some(origin) = self.drag_origin {
                    self.selection = Some(create_resized_selection(origin, handle, point));
                }
            }
            None => {}
        }
    }

    fn finish_drag(&mut self) {
        self.update_drag();
        self.drag_mode = None;
        self.drag_origin = None;
        self.selection = self
            .selection
            .filter(|selection| selection.size.x >= 2.0 && selection.size.y >= 2.0);
    }

    fn get_cursor(&self) -> CursorIcon {
        match self.drag_mode {
            Some(DragMode::New) => return CursorIcon::Crosshair,
            Some(DragMode::Move) => return CursorIcon::Grabbing,
            Some(DragMode::Resize(handle)) => return get_resize_cursor(handle),
            None => {}
        }
        let Some(selection) = self.selection else {
            return CursorIcon::Crosshair;
        };
        if create_action_bar_rect(selection, self.size).intersects(self.pointer) {
            return CursorIcon::Pointer;
        }
        if let Some(handle) = get_resize_handle(self.pointer, selection) {
            return get_resize_cursor(handle);
        }
        if selection.intersects(self.pointer) {
            return CursorIcon::Grab;
        }
        return CursorIcon::Crosshair;
    }
}

enum SelectionAction {
    Search(Rect),
    Cancel,
}

pub struct SelectionDelegate;

impl WindowDelegate<SelectionState> for SelectionDelegate {
    type AppCtx = App;

    fn created(&mut self, ctx: DelegateContext<'_, Self::AppCtx, SelectionState>) {
        ctx.env.scale_factor = ctx.window.scale_factor();
        let size = ctx.window.inner_size().to_logical(ctx.env.scale_factor);
        ctx.env.size = vec2(size.width as f32, size.height as f32);
        *ctx.shared.shared.window_id.lock().unwrap() = Some(ctx.window.id());
    }

    fn destroyed(&mut self, ctx: &Self::AppCtx) {
        *ctx.shared.window_id.lock().unwrap() = None;
    }

    fn before_frame(&mut self, ctx: DelegateContext<'_, Self::AppCtx, SelectionState>) {
        let Some(monitor) = ctx.shared.shared.pending_overlay.lock().unwrap().take() else {
            return;
        };
        ctx.window.set_outer_position(monitor.position);
        ctx.window
            .set_inner_size(LogicalSize::new(monitor.size.width, monitor.size.height));
        ctx.env.reset_for_activation(monitor);
        ctx.window.set_visible(true);
        ctx.window.focus();
    }

    fn event(&mut self, mut ctx: DelegateContext<'_, Self::AppCtx, SelectionState>, event: WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                let logical = size.to_logical(ctx.window.scale_factor());
                ctx.env.size = vec2(logical.width as f32, logical.height as f32);
            }
            WindowEvent::ScaleFactorChanged { scale_factor } => {
                ctx.env.scale_factor = scale_factor;
            }
            WindowEvent::KeyboardInput(key)
                if key.state == ElementState::Pressed && key.code == KeyCode::Escape =>
            {
                finish_action(ctx.reborrow(), SelectionAction::Cancel);
            }
            WindowEvent::RedrawRequested => {
                if let Some(action) = ctx.env.pending_action.take() {
                    finish_action(ctx.reborrow(), action);
                }
            }
            _ => {}
        }
    }
}

pub fn build_root(alloc: &'static IdAlloc) -> Box<dyn Component<SelectionState>> {
    return PaintArea::new()
        .size_fn(|state: &SelectionState| state.size)
        .revision(|state: &SelectionState| state.revision)
        .paint(paint_selection)
        .cursor(|state, _local| state.get_cursor())
        .on_pointer_down(|state, pointer| {
            state.pointer = pointer.local;
            state.begin_drag();
            return true;
        })
        .on_pointer_move(|state, pointer| {
            state.pointer = pointer.local;
            if state.drag_mode.is_some() {
                state.update_drag();
                return true;
            }
            // A standing selection draws pointer-dependent chrome (action-bar
            // hover, handle cursors); without one, moves change nothing.
            return state.selection.is_some();
        })
        .on_pointer_up(|state, pointer| {
            state.pointer = pointer.local;
            if state.drag_mode.is_none() {
                return false;
            }
            state.finish_drag();
            return true;
        })
        .into_component(alloc);
}

fn paint_selection(state: &SelectionState, painter: &mut Painter, area: Rect) {
    let origin = area.pos;
    let at = |rect: Rect| Rect { pos: origin + rect.pos, size: rect.size };

    let dim = Color::new(0.0, 0.0, 0.0, 0.38);
    let Some(selection) = state.selection else {
        painter.fill_rect(at(create_rect(Vec2::ZERO, state.size)), dim);
        return;
    };

    painter.fill_rect(at(create_rect(Vec2::ZERO, vec2(state.size.x, selection.pos.y))), dim);
    painter.fill_rect(
        at(create_rect(
            vec2(0.0, selection.pos.y + selection.size.y),
            vec2(state.size.x, state.size.y - selection.pos.y - selection.size.y),
        )),
        dim,
    );
    painter.fill_rect(
        at(create_rect(vec2(0.0, selection.pos.y), vec2(selection.pos.x, selection.size.y))),
        dim,
    );
    painter.fill_rect(
        at(create_rect(
            vec2(selection.pos.x + selection.size.x, selection.pos.y),
            vec2(state.size.x - selection.pos.x - selection.size.x, selection.size.y),
        )),
        dim,
    );
    painter.stroke_rect(at(selection), Color::from_rgb(47, 128, 237), 2.0);

    if state.drag_mode != Some(DragMode::New) {
        for (_, center) in get_resize_handle_centers(selection) {
            painter.queue().push_primitive(Primitive::Rect(RectanglePrimitive {
                rect: at(create_rect(
                    center - Vec2::splat(HANDLE_RADIUS),
                    Vec2::splat(HANDLE_RADIUS * 2.0),
                )),
                color: Color::WHITE,
                outline: Stroke {
                    width: 2.0,
                    color: Color::from_rgb(47, 128, 237),
                    kind: StrokeKind::Inside,
                    dash: None,
                },
                corners: Corners::same(HANDLE_RADIUS),
            }));
        }
    }
    if state.drag_mode.is_none() {
        draw_action_bar(
            painter,
            at(create_action_bar_rect(selection, state.size)),
            origin + state.pointer,
        );
    }
}

fn finish_action(ctx: DelegateContext<'_, App, SelectionState>, action: SelectionAction) {
    // Off screen BEFORE the capture thread starts: set_visible is the
    // synchronous half of teardown, while close_window is queued and races
    // the capture otherwise.
    ctx.window.set_visible(false);
    ctx.window.set_cursor(CursorIcon::Default);
    if let SelectionAction::Search(selection) = action {
        let monitor = ctx.env.monitor.clone().expect("active selection has a monitor");
        spawn_capture(&monitor, selection);
    }
    ctx.env.selection = None;
    ctx.env.drag_mode = None;
    ctx.env.pending_action = None;
    ctx.app.close_window(ctx.window.id());
}

/// `screencapture -R` speaks the CoreGraphics global space: top-left origin
/// at the primary display, logical points — exactly [`Monitor::position`]'s
/// space, so monitor + selection add directly.
fn spawn_capture(monitor: &Monitor, selection: Rect) {
    let capture_x = (monitor.position.x + selection.pos.x as f64).floor() as i64;
    let capture_y = (monitor.position.y + selection.pos.y as f64).floor() as i64;
    let capture_width = selection.size.x.ceil() as i64;
    let capture_height = selection.size.y.ceil() as i64;

    std::thread::spawn(move || {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let image_path =
            std::env::temp_dir().join(format!("snipsy-{}-{timestamp}.png", std::process::id()));
        let capture_rect = format!("-R{capture_x},{capture_y},{capture_width},{capture_height}");

        match std::process::Command::new("/usr/sbin/screencapture")
            .args(["-x", &capture_rect])
            .arg(&image_path)
            .status()
        {
            Ok(status) if status.success() && image_path.is_file() => {
                lens::upload_image_and_open_results(&image_path);
            }
            Ok(status) => eprintln!("screen capture failed with status {status}"),
            Err(error) => eprintln!("could not start screen capture: {error}"),
        }

        let _ = std::fs::remove_file(image_path);
    });
}

fn create_selection_rect(start: Vec2, current: Vec2) -> Rect {
    return create_rect(start.min(current), (start - current).abs());
}

fn create_resized_selection(origin: Rect, handle: ResizeHandle, point: Vec2) -> Rect {
    let left = origin.pos.x;
    let right = origin.pos.x + origin.size.x;
    let top = origin.pos.y;
    let bottom = origin.pos.y + origin.size.y;
    return match handle {
        ResizeHandle::TopLeft => create_selection_rect(vec2(right, bottom), point),
        ResizeHandle::Top => create_rect(
            vec2(left, top.min(point.y)),
            vec2(origin.size.x, (point.y - bottom).abs()),
        ),
        ResizeHandle::TopRight => create_selection_rect(vec2(left, bottom), point),
        ResizeHandle::Right => create_rect(
            vec2(left.min(point.x), top),
            vec2((point.x - left).abs(), origin.size.y),
        ),
        ResizeHandle::BottomRight => create_selection_rect(vec2(left, top), point),
        ResizeHandle::Bottom => create_rect(
            vec2(left, top.min(point.y)),
            vec2(origin.size.x, (point.y - top).abs()),
        ),
        ResizeHandle::BottomLeft => create_selection_rect(vec2(right, top), point),
        ResizeHandle::Left => create_rect(
            vec2(right.min(point.x), top),
            vec2((point.x - right).abs(), origin.size.y),
        ),
    };
}

fn create_moved_selection(origin: Rect, start: Vec2, current: Vec2, bounds: Vec2) -> Rect {
    return create_rect(
        vec2(
            (origin.pos.x + current.x - start.x).clamp(0.0, bounds.x - origin.size.x),
            (origin.pos.y + current.y - start.y).clamp(0.0, bounds.y - origin.size.y),
        ),
        origin.size,
    );
}

fn get_resize_handle_centers(selection: Rect) -> [(ResizeHandle, Vec2); 8] {
    let left = selection.pos.x;
    let center_x = selection.pos.x + selection.size.x / 2.0;
    let right = selection.pos.x + selection.size.x;
    let top = selection.pos.y;
    let center_y = selection.pos.y + selection.size.y / 2.0;
    let bottom = selection.pos.y + selection.size.y;
    return [
        (ResizeHandle::TopLeft, vec2(left, top)),
        (ResizeHandle::Top, vec2(center_x, top)),
        (ResizeHandle::TopRight, vec2(right, top)),
        (ResizeHandle::Right, vec2(right, center_y)),
        (ResizeHandle::BottomRight, vec2(right, bottom)),
        (ResizeHandle::Bottom, vec2(center_x, bottom)),
        (ResizeHandle::BottomLeft, vec2(left, bottom)),
        (ResizeHandle::Left, vec2(left, center_y)),
    ];
}

fn get_resize_handle(point: Vec2, selection: Rect) -> Option<ResizeHandle> {
    return get_resize_handle_centers(selection)
        .into_iter()
        .find_map(|(handle, center)| {
            create_rect(
                center - Vec2::splat(HANDLE_HIT_RADIUS),
                Vec2::splat(HANDLE_HIT_RADIUS * 2.0),
            )
            .intersects(point)
            .then_some(handle)
        });
}

fn get_resize_cursor(handle: ResizeHandle) -> CursorIcon {
    return match handle {
        ResizeHandle::TopLeft | ResizeHandle::BottomRight => CursorIcon::ResizeNW,
        ResizeHandle::Top | ResizeHandle::Bottom => CursorIcon::ResizeNS,
        ResizeHandle::TopRight | ResizeHandle::BottomLeft => CursorIcon::ResizeNE,
        ResizeHandle::Right | ResizeHandle::Left => CursorIcon::ResizeEW,
    };
}

fn create_action_bar_rect(selection: Rect, bounds: Vec2) -> Rect {
    let x = (selection.pos.x + selection.size.x - ACTION_BAR_WIDTH)
        .clamp(SCREEN_MARGIN, bounds.x - ACTION_BAR_WIDTH - SCREEN_MARGIN);
    let below_y = selection.pos.y + selection.size.y + ACTION_BAR_GAP;
    if below_y + ACTION_BAR_HEIGHT <= bounds.y - SCREEN_MARGIN {
        return create_rect(vec2(x, below_y), vec2(ACTION_BAR_WIDTH, ACTION_BAR_HEIGHT));
    }

    let above_y = selection.pos.y - ACTION_BAR_HEIGHT - ACTION_BAR_GAP;
    if above_y >= SCREEN_MARGIN {
        return create_rect(vec2(x, above_y), vec2(ACTION_BAR_WIDTH, ACTION_BAR_HEIGHT));
    }

    return create_rect(
        vec2(
            (selection.pos.x + selection.size.x - ACTION_BAR_WIDTH - ACTION_BAR_GAP)
                .clamp(SCREEN_MARGIN, bounds.x - ACTION_BAR_WIDTH - SCREEN_MARGIN),
            (selection.pos.y + selection.size.y - ACTION_BAR_HEIGHT - ACTION_BAR_GAP)
                .clamp(SCREEN_MARGIN, bounds.y - ACTION_BAR_HEIGHT - SCREEN_MARGIN),
        ),
        vec2(ACTION_BAR_WIDTH, ACTION_BAR_HEIGHT),
    );
}

fn create_search_button_rect(action_bar: Rect) -> Rect {
    return create_rect(action_bar.pos, vec2(ACTION_BAR_WIDTH / 2.0, ACTION_BAR_HEIGHT));
}

fn create_cancel_button_rect(action_bar: Rect) -> Rect {
    return create_rect(
        action_bar.pos + vec2(ACTION_BAR_WIDTH / 2.0, 0.0),
        vec2(ACTION_BAR_WIDTH / 2.0, ACTION_BAR_HEIGHT),
    );
}

fn draw_action_bar(painter: &mut Painter, action_bar: Rect, pointer: Vec2) {
    painter.queue().push_primitive(Primitive::Rect(RectanglePrimitive {
        rect: action_bar,
        color: Color::from_rgba(31, 32, 35, 250),
        outline: Stroke {
            width: 1.0,
            color: Color::from_rgba(255, 255, 255, 24),
            kind: StrokeKind::Inside,
            dash: None,
        },
        corners: Corners::same(12.0),
    }));

    let search = create_search_button_rect(action_bar);
    let cancel = create_cancel_button_rect(action_bar);
    if search.intersects(pointer) {
        painter.queue().push_primitive(Primitive::Rect(RectanglePrimitive {
            rect: create_rect(search.pos + Vec2::splat(4.0), search.size - Vec2::splat(8.0)),
            color: Color::from_rgba(47, 128, 237, 220),
            outline: Stroke::NONE,
            corners: Corners::same(9.0),
        }));
    } else if cancel.intersects(pointer) {
        painter.queue().push_primitive(Primitive::Rect(RectanglePrimitive {
            rect: create_rect(cancel.pos + Vec2::splat(4.0), cancel.size - Vec2::splat(8.0)),
            color: Color::from_rgba(235, 87, 87, 72),
            outline: Stroke::NONE,
            corners: Corners::same(9.0),
        }));
    }

    painter.line(
        action_bar.pos + vec2(ACTION_BAR_WIDTH / 2.0, 11.0),
        action_bar.pos + vec2(ACTION_BAR_WIDTH / 2.0, ACTION_BAR_HEIGHT - 11.0),
        Color::from_rgba(255, 255, 255, 40),
        1.0,
    );
    painter.queue().push_primitive(Primitive::Rect(RectanglePrimitive {
        rect: create_rect(search.pos + vec2(13.0, 12.0), Vec2::splat(14.0)),
        color: Color::TRANSPARENT,
        outline: Stroke {
            width: 2.5,
            color: Color::WHITE,
            kind: StrokeKind::Inside,
            dash: None,
        },
        corners: Corners::same(7.0),
    }));
    painter.line(
        search.pos + vec2(25.0, 25.0),
        search.pos + vec2(31.0, 31.0),
        Color::WHITE,
        2.5,
    );
    painter.line(
        cancel.pos + vec2(15.0, 15.0),
        cancel.pos + vec2(29.0, 29.0),
        Color::from_rgb(238, 238, 238),
        2.5,
    );
    painter.line(
        cancel.pos + vec2(29.0, 15.0),
        cancel.pos + vec2(15.0, 29.0),
        Color::from_rgb(238, 238, 238),
        2.5,
    );
}

fn create_rect(pos: Vec2, size: Vec2) -> Rect {
    return Rect { pos, size };
}

fn clamp_point(point: Vec2, bounds: Vec2) -> Vec2 {
    return point.clamp(Vec2::ZERO, bounds);
}
