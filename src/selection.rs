use std::cell::Cell;
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSBezierPath, NSColor, NSCursor, NSCursorFrameResizeDirections,
    NSCursorFrameResizePosition, NSEvent, NSFloatingWindowLevel, NSLineCapStyle, NSScreen, NSView,
    NSWindingRule, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::lens;

const ACTION_BAR_WIDTH: f64 = 80.0;
const ACTION_BAR_HEIGHT: f64 = 40.0;
const ACTION_BAR_GAP: f64 = 8.0;
const SCREEN_MARGIN: f64 = 8.0;
const HANDLE_RADIUS: f64 = 5.0;
const HANDLE_HIT_RADIUS: f64 = 9.0;

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

struct SelectionViewIvars {
    screen_frame: NSRect,
    primary_screen_height: f64,
    start: Cell<NSPoint>,
    current: Cell<NSPoint>,
    selection: Cell<Option<NSRect>>,
    resize_origin: Cell<Option<NSRect>>,
    drag_mode: Cell<Option<DragMode>>,
}

define_class!(
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[ivars = SelectionViewIvars]
    struct SelectionView;

    impl SelectionView {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty_rect: NSRect) {
            let bounds = self.bounds();
            let drag_mode = self.ivars().drag_mode.get();
            let selection = if drag_mode == Some(DragMode::New) {
                Some(get_selection_rect(
                    self.ivars().start.get(),
                    self.ivars().current.get(),
                ))
            } else {
                self.ivars().selection.get()
            };

            NSColor::colorWithCalibratedWhite_alpha(0.0, 0.38).setFill();
            let dimmed_area = NSBezierPath::bezierPathWithRect(bounds);
            if let Some(selection) = selection {
                dimmed_area.appendBezierPathWithRect(selection);
                dimmed_area.setWindingRule(NSWindingRule::EvenOdd);
            }
            dimmed_area.fill();

            let Some(selection) = selection else {
                return;
            };
            NSColor::colorWithCalibratedRed_green_blue_alpha(0.12, 0.46, 1.0, 1.0).setStroke();
            let border = NSBezierPath::bezierPathWithRect(selection);
            border.setLineWidth(2.0);
            border.stroke();

            if drag_mode != Some(DragMode::New) {
                draw_resize_handles(selection);
            }
            if drag_mode.is_none() {
                draw_action_bar(get_action_bar_rect(selection, bounds));
            }
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let point = event.locationInWindow();
            if let Some(selection) = self.ivars().selection.get() {
                let action_bar = get_action_bar_rect(selection, self.bounds());
                if is_point_in_rect(point, get_search_button_rect(action_bar)) {
                    self.confirm_selection(selection);
                    return;
                }
                if is_point_in_rect(point, get_cancel_button_rect(action_bar)) {
                    self.cancel_selection();
                    return;
                }
                if let Some(handle) = get_resize_handle(point, selection) {
                    self.ivars().start.set(point);
                    self.ivars().resize_origin.set(Some(selection));
                    self.ivars()
                        .drag_mode
                        .set(Some(DragMode::Resize(handle)));
                    get_resize_cursor(handle).set();
                    return;
                }
                if is_point_in_rect(point, selection) {
                    self.ivars().start.set(point);
                    self.ivars().resize_origin.set(Some(selection));
                    self.ivars().drag_mode.set(Some(DragMode::Move));
                    NSCursor::closedHandCursor().set();
                    return;
                }
            }

            let point = clamp_point(point, self.bounds());
            self.ivars().start.set(point);
            self.ivars().current.set(point);
            self.ivars().selection.set(None);
            self.ivars().resize_origin.set(None);
            self.ivars().drag_mode.set(Some(DragMode::New));
            self.invalidate_cursor_rects();
            NSCursor::crosshairCursor().set();
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let point = clamp_point(event.locationInWindow(), self.bounds());
            match self.ivars().drag_mode.get() {
                Some(DragMode::New) => self.ivars().current.set(point),
                Some(DragMode::Move) => {
                    if let Some(origin) = self.ivars().resize_origin.get() {
                        self.ivars().selection.set(Some(get_moved_selection(
                            origin,
                            self.ivars().start.get(),
                            point,
                            self.bounds(),
                        )));
                    }
                }
                Some(DragMode::Resize(handle)) => {
                    if let Some(origin) = self.ivars().resize_origin.get() {
                        self.ivars()
                            .selection
                            .set(Some(get_resized_selection(origin, handle, point)));
                    }
                }
                None => return,
            }
            match self.ivars().drag_mode.get() {
                Some(DragMode::Resize(handle)) => get_resize_cursor(handle).set(),
                Some(DragMode::Move) => NSCursor::closedHandCursor().set(),
                _ => NSCursor::crosshairCursor().set(),
            }
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            let point = clamp_point(event.locationInWindow(), self.bounds());
            let selection = match self.ivars().drag_mode.take() {
                Some(DragMode::New) => get_selection_rect(self.ivars().start.get(), point),
                Some(DragMode::Move) => {
                    let Some(origin) = self.ivars().resize_origin.get() else {
                        return;
                    };
                    get_moved_selection(
                        origin,
                        self.ivars().start.get(),
                        point,
                        self.bounds(),
                    )
                }
                Some(DragMode::Resize(handle)) => {
                    let Some(origin) = self.ivars().resize_origin.get() else {
                        return;
                    };
                    get_resized_selection(origin, handle, point)
                }
                None => return,
            };
            self.ivars().selection.set(
                (selection.size.width >= 2.0 && selection.size.height >= 2.0)
                    .then_some(selection),
            );
            self.ivars().resize_origin.set(None);
            self.invalidate_cursor_rects();
            self.set_cursor_for_point(point);
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            self.set_cursor_for_point(event.locationInWindow());
        }

        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            self.addCursorRect_cursor(self.bounds(), &NSCursor::crosshairCursor());
            let Some(selection) = self.ivars().selection.get() else {
                return;
            };

            self.addCursorRect_cursor(selection, &NSCursor::openHandCursor());
            for (handle, center) in get_resize_handle_centers(selection) {
                self.addCursorRect_cursor(get_handle_hit_rect(center), &get_resize_cursor(handle));
            }
            self.addCursorRect_cursor(
                get_action_bar_rect(selection, self.bounds()),
                &NSCursor::pointingHandCursor(),
            );
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if event.keyCode() == 53 {
                self.cancel_selection();
            }
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }
    }
);

define_class!(
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    struct SelectionWindow;

    impl SelectionWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }
    }
);

impl SelectionView {
    fn new(
        screen_frame: NSRect,
        primary_screen_height: f64,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let bounds = NSRect::new(NSPoint::new(0.0, 0.0), screen_frame.size);
        let this = Self::alloc(mtm).set_ivars(SelectionViewIvars {
            screen_frame,
            primary_screen_height,
            start: Cell::new(NSPoint::new(0.0, 0.0)),
            current: Cell::new(NSPoint::new(0.0, 0.0)),
            selection: Cell::new(None),
            resize_origin: Cell::new(None),
            drag_mode: Cell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: bounds] }
    }

    fn confirm_selection(&self, selection: NSRect) {
        let screen_frame = self.ivars().screen_frame;
        let capture_x = (screen_frame.origin.x + selection.origin.x).floor() as i64;
        let capture_y = (self.ivars().primary_screen_height
            - screen_frame.origin.y
            - selection.origin.y
            - selection.size.height)
            .floor() as i64;
        let capture_width = selection.size.width.ceil() as i64;
        let capture_height = selection.size.height.ceil() as i64;

        if let Some(window) = self.window() {
            window.orderOut(None);
        }
        NSCursor::arrowCursor().set();

        std::thread::spawn(move || {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let image_path =
                std::env::temp_dir().join(format!("snipsy-{}-{timestamp}.png", std::process::id()));
            let capture_rect =
                format!("-R{capture_x},{capture_y},{capture_width},{capture_height}");

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

    fn cancel_selection(&self) {
        if let Some(window) = self.window() {
            window.orderOut(None);
        }
        NSCursor::arrowCursor().set();
    }

    fn invalidate_cursor_rects(&self) {
        if let Some(window) = self.window() {
            window.invalidateCursorRectsForView(self);
        }
    }

    fn set_cursor_for_point(&self, point: NSPoint) {
        if let Some(selection) = self.ivars().selection.get() {
            if is_point_in_rect(point, get_action_bar_rect(selection, self.bounds())) {
                NSCursor::pointingHandCursor().set();
                return;
            }
            if let Some(handle) = get_resize_handle(point, selection) {
                get_resize_cursor(handle).set();
                return;
            }
            if is_point_in_rect(point, selection) {
                NSCursor::openHandCursor().set();
                return;
            }
        }
        NSCursor::crosshairCursor().set();
    }
}

impl SelectionWindow {
    fn new(screen_frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let window: Retained<Self> = unsafe { msg_send![super(this), init] };
        window.setStyleMask(NSWindowStyleMask::Borderless);
        window.setFrame_display(screen_frame, true);
        window
    }
}

pub fn begin_selection(mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let mouse_location = NSEvent::mouseLocation();
    let screen = NSScreen::screens(mtm)
        .into_iter()
        .find(|screen| is_point_in_rect(mouse_location, screen.frame()))
        .or_else(|| NSScreen::mainScreen(mtm))?;
    let primary_screen_height = NSScreen::mainScreen(mtm)?.frame().size.height;
    let screen_frame = screen.frame();
    let view = SelectionView::new(screen_frame, primary_screen_height, mtm);
    let window = SelectionWindow::new(screen_frame, mtm);

    unsafe { window.setReleasedWhenClosed(false) };
    window.setLevel(NSFloatingWindowLevel);
    window.setOpaque(false);
    window.setHasShadow(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setIgnoresMouseEvents(false);
    window.setAcceptsMouseMovedEvents(true);
    window.setContentView(Some(&view));

    let application = NSApplication::sharedApplication(mtm);
    application.activate();
    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&view));
    window.invalidateCursorRectsForView(&view);
    window.resetCursorRects();
    NSCursor::crosshairCursor().set();
    Some(window.into_super())
}

fn get_selection_rect(start: NSPoint, current: NSPoint) -> NSRect {
    NSRect::new(
        NSPoint::new(start.x.min(current.x), start.y.min(current.y)),
        NSSize::new((start.x - current.x).abs(), (start.y - current.y).abs()),
    )
}

fn get_resized_selection(origin: NSRect, handle: ResizeHandle, point: NSPoint) -> NSRect {
    let left = origin.origin.x;
    let right = origin.origin.x + origin.size.width;
    let bottom = origin.origin.y;
    let top = origin.origin.y + origin.size.height;

    match handle {
        ResizeHandle::TopLeft => get_selection_rect(NSPoint::new(right, bottom), point),
        ResizeHandle::Top => NSRect::new(
            NSPoint::new(left, bottom.min(point.y)),
            NSSize::new(origin.size.width, (point.y - bottom).abs()),
        ),
        ResizeHandle::TopRight => get_selection_rect(NSPoint::new(left, bottom), point),
        ResizeHandle::Right => NSRect::new(
            NSPoint::new(left.min(point.x), bottom),
            NSSize::new((point.x - left).abs(), origin.size.height),
        ),
        ResizeHandle::BottomRight => get_selection_rect(NSPoint::new(left, top), point),
        ResizeHandle::Bottom => NSRect::new(
            NSPoint::new(left, top.min(point.y)),
            NSSize::new(origin.size.width, (point.y - top).abs()),
        ),
        ResizeHandle::BottomLeft => get_selection_rect(NSPoint::new(right, top), point),
        ResizeHandle::Left => NSRect::new(
            NSPoint::new(right.min(point.x), bottom),
            NSSize::new((point.x - right).abs(), origin.size.height),
        ),
    }
}

fn get_moved_selection(origin: NSRect, start: NSPoint, current: NSPoint, bounds: NSRect) -> NSRect {
    NSRect::new(
        NSPoint::new(
            (origin.origin.x + current.x - start.x)
                .clamp(0.0, bounds.size.width - origin.size.width),
            (origin.origin.y + current.y - start.y)
                .clamp(0.0, bounds.size.height - origin.size.height),
        ),
        origin.size,
    )
}

fn get_resize_handle_centers(selection: NSRect) -> [(ResizeHandle, NSPoint); 8] {
    let left = selection.origin.x;
    let center_x = selection.origin.x + selection.size.width / 2.0;
    let right = selection.origin.x + selection.size.width;
    let bottom = selection.origin.y;
    let center_y = selection.origin.y + selection.size.height / 2.0;
    let top = selection.origin.y + selection.size.height;

    [
        (ResizeHandle::TopLeft, NSPoint::new(left, top)),
        (ResizeHandle::Top, NSPoint::new(center_x, top)),
        (ResizeHandle::TopRight, NSPoint::new(right, top)),
        (ResizeHandle::Right, NSPoint::new(right, center_y)),
        (ResizeHandle::BottomRight, NSPoint::new(right, bottom)),
        (ResizeHandle::Bottom, NSPoint::new(center_x, bottom)),
        (ResizeHandle::BottomLeft, NSPoint::new(left, bottom)),
        (ResizeHandle::Left, NSPoint::new(left, center_y)),
    ]
}

fn get_resize_handle(point: NSPoint, selection: NSRect) -> Option<ResizeHandle> {
    get_resize_handle_centers(selection)
        .into_iter()
        .find_map(|(handle, center)| {
            is_point_in_rect(point, get_handle_hit_rect(center)).then_some(handle)
        })
}

fn get_resize_cursor(handle: ResizeHandle) -> Retained<NSCursor> {
    let position = match handle {
        ResizeHandle::TopLeft => NSCursorFrameResizePosition::TopLeft,
        ResizeHandle::Top => NSCursorFrameResizePosition::Top,
        ResizeHandle::TopRight => NSCursorFrameResizePosition::TopRight,
        ResizeHandle::Right => NSCursorFrameResizePosition::Right,
        ResizeHandle::BottomRight => NSCursorFrameResizePosition::BottomRight,
        ResizeHandle::Bottom => NSCursorFrameResizePosition::Bottom,
        ResizeHandle::BottomLeft => NSCursorFrameResizePosition::BottomLeft,
        ResizeHandle::Left => NSCursorFrameResizePosition::Left,
    };
    NSCursor::frameResizeCursorFromPosition_inDirections(
        position,
        NSCursorFrameResizeDirections::All,
    )
}

fn get_handle_hit_rect(center: NSPoint) -> NSRect {
    NSRect::new(
        NSPoint::new(center.x - HANDLE_HIT_RADIUS, center.y - HANDLE_HIT_RADIUS),
        NSSize::new(HANDLE_HIT_RADIUS * 2.0, HANDLE_HIT_RADIUS * 2.0),
    )
}

fn draw_resize_handles(selection: NSRect) {
    for (_, center) in get_resize_handle_centers(selection) {
        let handle = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
            NSPoint::new(center.x - HANDLE_RADIUS, center.y - HANDLE_RADIUS),
            NSSize::new(HANDLE_RADIUS * 2.0, HANDLE_RADIUS * 2.0),
        ));
        NSColor::whiteColor().setFill();
        handle.fill();
        NSColor::colorWithCalibratedRed_green_blue_alpha(0.12, 0.46, 1.0, 1.0).setStroke();
        handle.setLineWidth(2.0);
        handle.stroke();
    }
}

fn get_action_bar_rect(selection: NSRect, bounds: NSRect) -> NSRect {
    let x = (selection.origin.x + selection.size.width - ACTION_BAR_WIDTH).clamp(
        SCREEN_MARGIN,
        bounds.size.width - ACTION_BAR_WIDTH - SCREEN_MARGIN,
    );
    let below_y = selection.origin.y - ACTION_BAR_HEIGHT - ACTION_BAR_GAP;
    if below_y >= SCREEN_MARGIN {
        return NSRect::new(
            NSPoint::new(x, below_y),
            NSSize::new(ACTION_BAR_WIDTH, ACTION_BAR_HEIGHT),
        );
    }

    let above_y = selection.origin.y + selection.size.height + ACTION_BAR_GAP;
    if above_y + ACTION_BAR_HEIGHT <= bounds.size.height - SCREEN_MARGIN {
        return NSRect::new(
            NSPoint::new(x, above_y),
            NSSize::new(ACTION_BAR_WIDTH, ACTION_BAR_HEIGHT),
        );
    }

    NSRect::new(
        NSPoint::new(
            (selection.origin.x + selection.size.width - ACTION_BAR_WIDTH - ACTION_BAR_GAP).clamp(
                SCREEN_MARGIN,
                bounds.size.width - ACTION_BAR_WIDTH - SCREEN_MARGIN,
            ),
            (selection.origin.y + ACTION_BAR_GAP).clamp(
                SCREEN_MARGIN,
                bounds.size.height - ACTION_BAR_HEIGHT - SCREEN_MARGIN,
            ),
        ),
        NSSize::new(ACTION_BAR_WIDTH, ACTION_BAR_HEIGHT),
    )
}

fn get_search_button_rect(action_bar: NSRect) -> NSRect {
    NSRect::new(
        action_bar.origin,
        NSSize::new(ACTION_BAR_WIDTH / 2.0, ACTION_BAR_HEIGHT),
    )
}

fn get_cancel_button_rect(action_bar: NSRect) -> NSRect {
    NSRect::new(
        NSPoint::new(
            action_bar.origin.x + ACTION_BAR_WIDTH / 2.0,
            action_bar.origin.y,
        ),
        NSSize::new(ACTION_BAR_WIDTH / 2.0, ACTION_BAR_HEIGHT),
    )
}

fn draw_action_bar(action_bar: NSRect) {
    let background =
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(action_bar, 10.0, 10.0);
    NSColor::colorWithCalibratedWhite_alpha(0.13, 0.98).setFill();
    background.fill();
    NSColor::colorWithCalibratedWhite_alpha(1.0, 0.1).setStroke();
    background.setLineWidth(1.0);
    background.stroke();

    let divider = NSBezierPath::bezierPath();
    divider.moveToPoint(NSPoint::new(
        action_bar.origin.x + ACTION_BAR_WIDTH / 2.0,
        action_bar.origin.y + 10.0,
    ));
    divider.lineToPoint(NSPoint::new(
        action_bar.origin.x + ACTION_BAR_WIDTH / 2.0,
        action_bar.origin.y + ACTION_BAR_HEIGHT - 10.0,
    ));
    NSColor::colorWithCalibratedWhite_alpha(1.0, 0.18).setStroke();
    divider.setLineWidth(1.0);
    divider.stroke();

    NSColor::whiteColor().setStroke();
    let search_button = get_search_button_rect(action_bar);
    let search_icon = NSBezierPath::bezierPathWithOvalInRect(NSRect::new(
        NSPoint::new(search_button.origin.x + 12.0, search_button.origin.y + 13.0),
        NSSize::new(11.0, 11.0),
    ));
    search_icon.setLineWidth(2.0);
    search_icon.stroke();
    let search_handle = NSBezierPath::bezierPath();
    search_handle.moveToPoint(NSPoint::new(
        search_button.origin.x + 21.0,
        search_button.origin.y + 14.0,
    ));
    search_handle.lineToPoint(NSPoint::new(
        search_button.origin.x + 28.0,
        search_button.origin.y + 7.0,
    ));
    search_handle.setLineWidth(2.0);
    search_handle.setLineCapStyle(NSLineCapStyle::Round);
    search_handle.stroke();

    let cancel_button = get_cancel_button_rect(action_bar);
    NSColor::colorWithCalibratedWhite_alpha(0.92, 1.0).setStroke();
    let cancel_icon = NSBezierPath::bezierPath();
    cancel_icon.moveToPoint(NSPoint::new(
        cancel_button.origin.x + 13.0,
        cancel_button.origin.y + 13.0,
    ));
    cancel_icon.lineToPoint(NSPoint::new(
        cancel_button.origin.x + 27.0,
        cancel_button.origin.y + 27.0,
    ));
    cancel_icon.moveToPoint(NSPoint::new(
        cancel_button.origin.x + 27.0,
        cancel_button.origin.y + 13.0,
    ));
    cancel_icon.lineToPoint(NSPoint::new(
        cancel_button.origin.x + 13.0,
        cancel_button.origin.y + 27.0,
    ));
    cancel_icon.setLineWidth(2.0);
    cancel_icon.setLineCapStyle(NSLineCapStyle::Round);
    cancel_icon.stroke();
}

fn clamp_point(point: NSPoint, bounds: NSRect) -> NSPoint {
    NSPoint::new(
        point.x.clamp(0.0, bounds.size.width),
        point.y.clamp(0.0, bounds.size.height),
    )
}

fn is_point_in_rect(point: NSPoint, rect: NSRect) -> bool {
    point.x >= rect.origin.x
        && point.x <= rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y <= rect.origin.y + rect.size.height
}
