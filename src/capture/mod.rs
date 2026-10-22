mod layout;
mod paint;
mod text;
pub mod toolbar;

use crate::{
    app::{self, AppState, content::PinContent, export, history, recording},
    editor::{Annotation, BlurMode, Editor, TextAttribute, Tool},
    geometry::{Color, Rect},
    pin, platform,
    raster::{self, Screenshot},
    ui::{self, View, magnifier},
};
use anyhow::{Result, anyhow};
use glam::{Vec2, vec2};
use gpui_kit::{
    component::{
        ActiveTheme,
        slider::{SliderEvent, SliderState},
    },
    prelude::*,
    *,
};
use image::RgbaImage;
use layout::ResizeHandle;
use std::{cell::Cell, rc::Rc, sync::Arc};
use text::TextCursor;

#[derive(Default)]
struct Captures {
    source: platform::CaptureSource,
    active: Option<View<CaptureView>>,
    cache: Option<View<CaptureView>>,
    pin_editor: Option<View<CaptureView>>,
}

impl Global for Captures {}

pub fn init(cx: &mut App) {
    cx.set_global(Captures::default());
}

pub fn start(cx: &mut App) {
    if let Some(view) = cx.global::<Captures>().active.clone() {
        if view
            .update(cx, |capture, window, cx| capture.activate(window, cx))
            .is_ok()
        {
            return;
        }
        cx.global_mut::<Captures>().active = None;
    }
    let bounds = match platform::get_desktop_bounds() {
        Ok(bounds) => bounds,
        Err(error) => return app::show_error(error),
    };
    let windows = platform::get_capture_windows(bounds);
    let view = match show_overlay(bounds, Mode::Capture, None, cx) {
        Ok(view) => view,
        Err(error) => return app::show_error(error),
    };
    let _ = view.update(cx, |view, window, cx| {
        view.windows = windows.clone();
        view.activate(window, cx);
        let (sender, receiver) = async_channel::bounded(1);
        let shows_cursor = cx.global::<AppState>().capture_cursor;
        let source = &cx.global::<Captures>().source;
        let overlay = platform::get_window_id(window);
        platform::capture_screen(source, overlay, bounds, shows_cursor, windows, sender);
        let capture = cx.background_executor().spawn(async move {
            let screenshot = receiver.recv().await??;
            let background = raster::create_render_image(&screenshot.image);
            return anyhow::Ok((screenshot, background));
        });
        view.load_capture(capture, window, cx)
    });
}

pub fn start_whiteboard(color: [u8; 4], cx: &mut App) {
    let bounds = match platform::get_desktop_bounds() {
        Ok(bounds) => bounds,
        Err(error) => return app::show_error(error),
    };
    let size = (bounds.size * platform::get_desktop_scale())
        .round()
        .max(Vec2::ONE);
    let screenshot = Screenshot {
        image: Arc::new(RgbaImage::from_pixel(
            size.x as u32,
            size.y as u32,
            image::Rgba(color),
        )),
        bounds,
        windows: Vec::new(),
    };
    match show_overlay(bounds, Mode::Whiteboard, Some(screenshot), cx) {
        Ok(view) => {
            let _ = view.update(cx, |capture, window, cx| capture.activate(window, cx));
        }
        Err(error) => app::show_error(error),
    }
}

/// Reuses the hidden capture window when possible: creating a window and its GPU
/// resources is the slowest part of starting a capture.
fn show_overlay(
    bounds: Rect,
    mode: Mode,
    screenshot: Option<Screenshot>,
    cx: &mut App,
) -> Result<View<CaptureView>> {
    if let Some(view) = cx.global_mut::<Captures>().active.take() {
        let _ = view.update(cx, |capture, window, cx| capture.dismiss(window, cx));
    }
    let pointer = platform::get_pointer_position();
    let cached = cx.global::<Captures>().cache.clone().filter(|view| {
        view.update(cx, |capture, window, cx| {
            platform::configure_overlay(window, bounds, false);
            capture.reset(
                bounds,
                mode.clone(),
                screenshot.clone(),
                pointer,
                window,
                cx,
            );
        })
        .is_ok()
    });
    let view = match cached {
        Some(view) => view,
        None => ui::open_overlay(bounds, false, cx, |window, cx| {
            cx.new(|cx| CaptureView::new(bounds, mode, screenshot, pointer, window, cx))
        })?,
    };
    let captures = cx.global_mut::<Captures>();
    captures.active = Some(view.clone());
    captures.cache = Some(view.clone());
    return Ok(view);
}

pub fn open_pin_editor(
    image: Arc<RgbaImage>,
    bounds: Rect,
    target: View<pin::Pin>,
    cx: &mut App,
) -> bool {
    if let Some(view) = cx.global::<Captures>().pin_editor.clone()
        && view
            .update(cx, |capture, window, cx| {
                let editing_target =
                    matches!(&capture.mode, Mode::PinEditor(editing) if editing.entity == target.entity);
                if editing_target {
                    capture.activate(window, cx);
                } else {
                    capture.dismiss(window, cx);
                }
                return editing_target;
            })
            .unwrap_or(false)
    {
        return false;
    }
    let desktop = match platform::get_desktop_bounds() {
        Ok(desktop) => desktop,
        Err(error) => {
            app::show_error(error);
            return false;
        }
    };
    let screenshot = Screenshot {
        image,
        bounds,
        windows: Vec::new(),
    };
    let pointer = platform::get_pointer_position();
    let view = ui::open_overlay(desktop, false, cx, |window, cx| {
        cx.new(|cx| {
            CaptureView::new(
                desktop,
                Mode::PinEditor(target),
                Some(screenshot),
                pointer,
                window,
                cx,
            )
        })
    });
    return match view {
        Ok(view) => {
            let _ = view.update(cx, |capture, window, cx| capture.activate(window, cx));
            cx.global_mut::<Captures>().pin_editor = Some(view);
            true
        }
        Err(error) => {
            app::show_error(error);
            false
        }
    };
}

#[derive(Clone)]
enum Mode {
    Capture,
    Whiteboard,
    PinEditor(View<pin::Pin>),
}

#[derive(Clone, Copy, PartialEq)]
enum DragMode {
    New,
    Move,
    Resize(ResizeHandle),
    Draw,
    MoveAnnotation(usize),
    SelectText(usize, usize),
    Erase,
}

struct Drag {
    mode: DragMode,
    start: Vec2,
    origin: Option<Rect>,
    annotation_origin: Vec<Vec2>,
    recorded_change: bool,
}

impl Drag {
    fn new(mode: DragMode, start: Vec2) -> Self {
        return Self {
            mode,
            start,
            origin: None,
            annotation_origin: Vec::new(),
            recorded_change: false,
        };
    }
}

#[derive(Clone, Copy, PartialEq)]
enum EditorAction {
    Tool(Tool),
    Variant(Tool),
    Filled(bool),
    BlurMode(BlurMode),
    TextStyle(TextAttribute),
    ColorPicker,
    Layers,
    Delete,
    Undo,
    Redo,
    Cancel,
    Record,
    Copy,
    Save,
    QuickSave,
    Pin,
    Search,
    Print,
    Apply,
}

/// Annotations up to the last blur are composited on the CPU, because blur reads the
/// pixels below it; everything above stays on GPU paths and redraws every frame.
#[derive(Default)]
struct RasterPreview {
    image: Option<Arc<RenderImage>>,
    scene: Vec<Annotation>,
    selection: Option<Rect>,
    task: Option<Task<()>>,
}

struct CaptureView {
    mode: Mode,
    bounds: Rect,
    screenshot: Option<Screenshot>,
    windows: Vec<Rect>,
    background: Option<Arc<RenderImage>>,
    raster_preview: RasterPreview,
    selection: Option<Rect>,
    editor: Editor,
    pointer: Vec2,
    drag: Option<Drag>,
    focus: FocusHandle,
    error: Option<SharedString>,
    text: TextCursor,
    pending_action: Option<EditorAction>,
    capture_task: Option<Task<()>>,
    color_task: Option<Task<()>>,
    custom_color: Option<Color>,
    show_layers: bool,
    show_toolbar: bool,
    confirming_discard: bool,
    layer_drop_target: Option<(u64, bool)>,
    capture_history: Vec<Screenshot>,
    capture_history_index: usize,
    intensity: Entity<SliderState>,
    toolbar_size: Rc<Cell<Option<Vec2>>>,
    _subscriptions: Vec<Subscription>,
}

impl CaptureView {
    fn new(
        bounds: Rect,
        mode: Mode,
        screenshot: Option<Screenshot>,
        pointer: Vec2,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let editor = Editor::new();
        let intensity = cx.new(|_| {
            SliderState::new()
                .min(1.0)
                .max(48.0)
                .step(1.0)
                .default_value(editor.intensity)
        });
        let subscription = cx.subscribe(&intensity, |capture, _, event: &SliderEvent, cx| {
            if let SliderEvent::Change(value) = event {
                capture.editor.set_intensity(value.start());
                cx.notify();
            }
        });
        let mut capture = Self {
            mode,
            bounds,
            screenshot: None,
            windows: Vec::new(),
            background: None,
            raster_preview: RasterPreview::default(),
            selection: None,
            editor,
            pointer: (pointer - bounds.pos).clamp(Vec2::ZERO, bounds.size),
            drag: None,
            focus,
            error: None,
            text: TextCursor::default(),
            pending_action: None,
            capture_task: None,
            color_task: None,
            custom_color: None,
            show_layers: false,
            show_toolbar: true,
            confirming_discard: false,
            layer_drop_target: None,
            capture_history: Vec::new(),
            capture_history_index: 0,
            intensity,
            toolbar_size: Rc::new(Cell::new(None)),
            _subscriptions: vec![subscription],
        };
        if let Some(screenshot) = screenshot {
            capture.set_screenshot(screenshot);
            capture.selection = Some(capture.get_canvas());
        }
        return capture;
    }

    fn get_canvas(&self) -> Rect {
        let bounds = self
            .screenshot
            .as_ref()
            .map_or(self.bounds, |screenshot| screenshot.bounds);
        return Rect {
            pos: bounds.pos - self.bounds.pos,
            size: bounds.size,
        };
    }

    fn set_screenshot(&mut self, screenshot: Screenshot) {
        self.windows = screenshot.windows.clone();
        self.background = Some(raster::create_render_image(&screenshot.image));
        self.screenshot = Some(screenshot);
    }

    fn reset(
        &mut self,
        bounds: Rect,
        mode: Mode,
        screenshot: Option<Screenshot>,
        pointer: Vec2,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for image in [self.background.take(), self.raster_preview.image.take()]
            .into_iter()
            .flatten()
        {
            cx.drop_image(image, Some(window));
        }
        *self = Self::new(bounds, mode, screenshot, pointer, window, cx);
        cx.notify();
    }

    fn activate(&self, window: &mut Window, cx: &mut Context<Self>) {
        platform::make_key_window(window);
        cx.activate(true);
        window.activate_window();
        window.focus(&self.focus, cx);
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editor.layers.is_empty() {
            return self.dismiss(window, cx);
        }
        self.confirming_discard = true;
        cx.notify();
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Mode::PinEditor(target) = &self.mode {
            let target = target.clone();
            let _ = target.update(cx, |pin, window, cx| {
                platform::set_window_visible(window, pin.get_is_visible(cx))
            });
            cx.global_mut::<Captures>().pin_editor = None;
            window.remove_window();
            return;
        }
        let captures = cx.global_mut::<Captures>();
        if captures
            .active
            .as_ref()
            .is_some_and(|view| view.window == window.window_handle())
        {
            captures.active = None;
        }
        platform::hide_overlay(window);
        self.reset(
            self.bounds,
            Mode::Capture,
            None,
            self.bounds.pos + self.pointer,
            window,
            cx,
        );
    }

    fn load_capture(
        &mut self,
        capture: Task<Result<(Screenshot, Arc<RenderImage>)>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.capture_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = capture.await;
            let _ = this.update_in(cx, |capture, window, cx| match result {
                Ok((screenshot, background)) => {
                    capture.complete_capture(screenshot, background, window, cx)
                }
                Err(error) => {
                    capture.dismiss(window, cx);
                    app::show_error(error);
                }
            });
        }));
    }

    fn complete_capture(
        &mut self,
        screenshot: Screenshot,
        background: Arc<RenderImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if screenshot.bounds != self.bounds {
            self.dismiss(window, cx);
            app::show_error(anyhow!("Display configuration changed; capture again"));
            return;
        }
        self.capture_history = std::iter::once(screenshot.clone())
            .chain(cx.global::<history::History>().captures.iter().cloned())
            .take(11)
            .collect();
        self.capture_history_index = 0;
        self.background = Some(background);
        self.screenshot = Some(screenshot);
        self.activate(window, cx);
        if let Some(action) = self.pending_action.take() {
            self.perform_action(action, window, cx);
        }
        cx.notify();
    }

    fn navigate_capture_history(
        &mut self,
        direction: i32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.screenshot.is_none() {
            return;
        }
        let bounds = self
            .capture_history
            .iter()
            .map(|screenshot| screenshot.bounds)
            .collect::<Vec<_>>();
        let Some(index) = layout::get_capture_history_index(
            &bounds,
            self.capture_history_index,
            direction,
            self.bounds,
        ) else {
            return;
        };
        let history = std::mem::take(&mut self.capture_history);
        self.reset(
            self.bounds,
            self.mode.clone(),
            None,
            self.bounds.pos + self.pointer,
            window,
            cx,
        );
        self.set_screenshot(history[index].clone());
        self.capture_history = history;
        self.capture_history_index = index;
    }

    fn get_pointer(&self, position: Point<Pixels>) -> Vec2 {
        return vec2(position.x.into(), position.y.into()).clamp(Vec2::ZERO, self.bounds.size);
    }

    fn get_drag_mode(&self) -> Option<DragMode> {
        return self.drag.as_ref().map(|drag| drag.mode);
    }

    fn get_hovered_window(&self) -> Option<Rect> {
        if self.selection.is_some() || self.drag.is_some() {
            return None;
        }
        return self
            .windows
            .iter()
            .find(|rect| rect.contains(self.pointer))
            .copied();
    }

    fn get_cursor(&self) -> CursorStyle {
        let tool = if self.get_text_draft().is_some() {
            Tool::Text
        } else {
            self.editor.tool
        };
        return get_cursor_style(
            self.get_drag_mode(),
            tool,
            self.pointer,
            self.selection,
            self.get_canvas(),
        );
    }

    fn get_shows_magnifier(&self) -> bool {
        return self.screenshot.is_some()
            && (self.selection.is_none() || self.get_drag_mode() == Some(DragMode::New))
            && self.get_cursor() == CursorStyle::Crosshair;
    }

    fn get_sample(&self) -> Option<magnifier::Sample> {
        let image = &self.screenshot.as_ref()?.image;
        let canvas = self.get_canvas();
        let scale = vec2(image.width() as f32, image.height() as f32) / canvas.size;
        let pixel = ((self.pointer - canvas.pos) * scale).floor();
        let max = vec2(image.width() as f32, image.height() as f32) - 1.0;
        let pixel = pixel.clamp(Vec2::ZERO, max.max(Vec2::ZERO));
        return magnifier::sample_pixel(image, pixel.x as u32, pixel.y as u32);
    }

    fn get_annotation_at(&self, point: Vec2, window: &Window) -> Option<usize> {
        return self.editor.layers.iter().rposition(|annotation| {
            if annotation.tool == Tool::Text {
                text::get_annotation_bounds(annotation, window).contains(point)
            } else {
                annotation.contains_point(point)
            }
        });
    }

    fn begin_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirming_discard {
            self.confirming_discard = false;
            cx.notify();
            return;
        }
        self.editor.finish_settings_change();
        self.text.insertion_format = None;
        window.focus(&self.focus, cx);
        self.pointer = self.get_pointer(event.position);
        cx.notify();
        if self
            .get_text_draft()
            .is_some_and(|draft| text::get_annotation_bounds(draft, window).contains(self.pointer))
        {
            self.begin_text_selection(event, window, cx);
            return;
        }
        let continue_line = self.editor.tool == Tool::Line
            && self
                .editor
                .draft
                .as_ref()
                .is_some_and(|draft| draft.tool == Tool::Line)
            && self
                .selection
                .is_some_and(|selection| selection.contains(self.pointer));
        if !continue_line {
            self.editor.commit_draft();
        }
        let pointer = self.pointer;
        let Some(selection) = self.selection.filter(|selection| {
            selection.contains(pointer) || layout::get_resize_handle(pointer, *selection).is_some()
        }) else {
            if !self.get_canvas().contains(pointer) {
                return;
            }
            let mut drag = Drag::new(DragMode::New, self.pointer);
            drag.origin = self.get_hovered_window();
            self.drag = Some(drag);
            self.selection = None;
            self.editor.selected = None;
            return;
        };
        if let Some(handle) = layout::get_resize_handle(self.pointer, selection) {
            let mut drag = Drag::new(DragMode::Resize(handle), self.pointer);
            drag.origin = Some(selection);
            self.drag = Some(drag);
            return;
        }
        let tool = self.editor.tool;
        let hit = if continue_line {
            None
        } else {
            self.get_annotation_at(self.pointer, window)
        };
        if tool != Tool::Eraser
            && (tool == Tool::Text || event.click_count >= 2)
            && let Some(index) = hit.filter(|index| self.editor.layers[*index].tool == Tool::Text)
        {
            self.editor.start_editing_text(index);
            self.begin_text_selection(event, window, cx);
            return;
        }
        let grabs_same_family = !matches!(tool, Tool::Text | Tool::Eraser)
            && hit.is_some_and(|index| {
                tool == Tool::Select
                    || get_tool_family(self.editor.layers[index].tool) == get_tool_family(tool)
            });
        if let (true, Some(index)) = (grabs_same_family, hit) {
            if tool == Tool::Select {
                self.editor.select_layer(index);
            } else {
                self.editor.selected = Some(index);
            }
            let mut drag = Drag::new(DragMode::MoveAnnotation(index), self.pointer);
            drag.annotation_origin = self.editor.layers[index].points.clone();
            self.drag = Some(drag);
            return;
        }
        match tool {
            Tool::Select => {
                self.editor.selected = None;
                let mut drag = Drag::new(DragMode::Move, self.pointer);
                drag.origin = Some(selection);
                self.drag = Some(drag);
            }
            Tool::Eraser => {
                self.drag = Some(Drag::new(DragMode::Erase, self.pointer));
                self.erase_at_pointer(window);
            }
            Tool::Line if continue_line => {
                self.editor
                    .draft
                    .as_mut()
                    .unwrap()
                    .points
                    .push(self.pointer);
                self.drag = Some(Drag::new(DragMode::Draw, self.pointer));
            }
            tool => {
                self.editor.selected = None;
                self.editor.draft = Some(self.editor.create_annotation(tool, self.pointer));
                if tool == Tool::Text {
                    self.text = TextCursor::default();
                } else {
                    self.drag = Some(Drag::new(DragMode::Draw, self.pointer));
                }
            }
        }
    }

    fn begin_text_selection(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(offset) = self.character_index_for_point(event.position, window, cx) else {
            return;
        };
        let text = &self.get_text_draft().unwrap().text;
        let range = text::create_clicked_text_range(text, offset, event.click_count);
        self.text.set(range.clone(), false);
        self.text.marked = None;
        self.drag = Some(Drag::new(
            DragMode::SelectText(range.start, range.end),
            self.pointer,
        ));
    }

    fn update_drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.get_cursor();
        let hovered = self.get_hovered_window();
        let previous = self.pointer;
        self.pointer = self.get_pointer(event.position);
        let Some(drag) = &self.drag else {
            if cursor != self.get_cursor()
                || hovered != self.get_hovered_window()
                || (self.get_shows_magnifier() && previous != self.pointer)
            {
                cx.notify();
            }
            return;
        };
        let (mode, start) = (drag.mode, drag.start);
        let mut point = self.pointer;
        if let Some(selection) = self.selection
            && matches!(mode, DragMode::Draw | DragMode::Erase)
        {
            point = point.clamp(selection.pos, selection.end());
        }
        let canvas = self.get_canvas();
        if matches!(mode, DragMode::New) {
            point = point.clamp(canvas.pos, canvas.end());
        }
        if event.modifiers.shift && mode == DragMode::Draw {
            point = constrain_point(self.editor.tool, start, point);
        }
        match mode {
            DragMode::New => {
                self.selection = Some(Rect::from_corners(start, point))
                    .filter(|rect| layout::get_is_valid_selection(*rect));
            }
            DragMode::Move => {
                let origin = drag.origin.unwrap();
                self.selection = Some(layout::create_moved_selection(
                    origin,
                    point - start,
                    canvas,
                ));
            }
            DragMode::Resize(handle) => {
                let origin = drag.origin.unwrap();
                let center = layout::get_resize_handle_centers(origin)
                    .into_iter()
                    .find(|(candidate, _)| *candidate == handle)
                    .unwrap()
                    .1;
                self.selection = Some(layout::create_resized_selection(
                    origin,
                    handle,
                    (center + point - start).clamp(canvas.pos, canvas.end()),
                ));
            }
            DragMode::MoveAnnotation(index) => {
                let Some(selection) = self.selection else {
                    return;
                };
                let delta = (point - start).clamp(selection.pos - start, selection.end() - start);
                let points = drag
                    .annotation_origin
                    .iter()
                    .map(|point| *point + delta)
                    .collect();
                if delta != Vec2::ZERO {
                    self.record_drag_change();
                }
                self.editor.layers[index].points = points;
            }
            DragMode::Draw => {
                if let Some(draft) = &mut self.editor.draft {
                    if matches!(draft.tool, Tool::Pen | Tool::Highlighter) {
                        if draft.points.last().unwrap().distance(point) >= 0.5 {
                            draft.points.push(point);
                        }
                    } else if draft.points.len() == 1 {
                        draft.points.push(point);
                    } else {
                        *draft.points.last_mut().unwrap() = point;
                    }
                }
            }
            DragMode::SelectText(start, end) => {
                if let Some(offset) = self.character_index_for_point(event.position, window, cx) {
                    self.text
                        .set(start.min(offset)..end.max(offset), offset < start);
                }
            }
            DragMode::Erase => self.erase_at_pointer(window),
        }
        cx.notify();
    }

    fn finish_drag(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        self.update_drag(
            &MouseMoveEvent {
                position: event.position,
                modifiers: event.modifiers,
                ..Default::default()
            },
            window,
            cx,
        );
        let drag = self.drag.take().unwrap();
        if drag.mode == DragMode::New && self.selection.is_none() {
            self.selection = drag.origin;
        }
        self.selection = self
            .selection
            .filter(|selection| layout::get_is_valid_selection(*selection));
        if drag.mode == DragMode::Draw && self.editor.tool == Tool::Line {
            let dragged = drag.start.distance(self.pointer) >= 1.0;
            if !dragged
                && let Some(draft) = &mut self.editor.draft
                && draft.points.len() > 1
                && draft.points[draft.points.len() - 2].distance(*draft.points.last().unwrap())
                    < 1.0
            {
                draft.points.pop();
            }
            if dragged || event.click_count >= 2 {
                self.editor.commit_draft();
            }
        } else if drag.mode == DragMode::Draw {
            self.editor.commit_draft();
        }
        cx.notify();
    }

    fn record_drag_change(&mut self) {
        if let Some(drag) = &mut self.drag
            && !drag.recorded_change
        {
            drag.recorded_change = true;
            self.editor.record_change();
        }
    }

    fn erase_at_pointer(&mut self, window: &Window) {
        if let Some(index) = self.get_annotation_at(self.pointer, window) {
            self.record_drag_change();
            self.editor.layers.remove(index);
            self.editor.selected = None;
        }
    }

    fn create_export_image(&mut self, cx: &App) -> Result<RgbaImage> {
        self.editor.commit_draft();
        let selection = self
            .selection
            .ok_or_else(|| anyhow!("Select an area first"))?;
        let screenshot = self
            .screenshot
            .as_ref()
            .ok_or_else(|| anyhow!("Capture is still loading"))?;
        return raster::render_selection(
            &screenshot.image,
            self.get_canvas(),
            selection,
            self.editor.get_visible_annotations(),
            &cx.global::<AppState>().fonts,
        );
    }

    fn get_screen_selection(&self) -> Option<Rect> {
        return self.selection.map(|selection| Rect {
            pos: self.bounds.pos + selection.pos,
            size: selection.size,
        });
    }

    fn perform_action(
        &mut self,
        action: EditorAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(
            action,
            EditorAction::Tool(_)
                | EditorAction::Variant(_)
                | EditorAction::Delete
                | EditorAction::Undo
                | EditorAction::Redo
        ) {
            self.drag = None;
        }
        if action == EditorAction::Record {
            let Some(bounds) = self.get_screen_selection() else {
                self.error = Some("Select an area first".into());
                cx.notify();
                return;
            };
            history::remember_selection(self.bounds, self.selection.unwrap(), cx);
            self.dismiss(window, cx);
            recording::start(bounds, cx);
            return;
        }
        self.error = None;
        let needs_image = matches!(
            action,
            EditorAction::Copy
                | EditorAction::Save
                | EditorAction::QuickSave
                | EditorAction::Pin
                | EditorAction::Search
                | EditorAction::Print
                | EditorAction::Apply
        );
        if needs_image && self.screenshot.is_none() && self.selection.is_some() {
            self.pending_action = Some(action);
            return;
        }
        match action {
            EditorAction::Tool(tool) => self.editor.select_tool(tool),
            EditorAction::Variant(tool) => self.editor.set_variant(tool),
            EditorAction::Filled(filled) => self.editor.set_filled(filled),
            EditorAction::BlurMode(mode) => self.editor.set_blur_mode(mode),
            EditorAction::TextStyle(attribute) => self.toggle_text_attribute(attribute),
            EditorAction::ColorPicker => self.pick_color(cx),
            EditorAction::Layers => self.show_layers = !self.show_layers,
            EditorAction::Delete => self.editor.delete_selected(),
            EditorAction::Undo => self.editor.undo_change(),
            EditorAction::Redo => self.editor.redo_change(),
            EditorAction::Cancel => return self.cancel(window, cx),
            _ => match self.export(action, window, cx) {
                Ok(true) => return,
                Ok(false) => {}
                Err(error) => self.error = Some(format!("{error:#}").into()),
            },
        }
        if self.editor.draft.is_none() {
            self.text.marked = None;
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn export(
        &mut self,
        action: EditorAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool> {
        let image = Arc::new(self.create_export_image(cx)?);
        let screenshot = self.screenshot.clone();
        if action == EditorAction::Apply {
            if let Mode::PinEditor(target) = &self.mode {
                let target = target.clone();
                // shortcut: editing an animation keeps only the selected frame, preserve frames once annotations can span a timeline.
                let _ = target.update(cx, |pin, window, cx| {
                    pin.replace_content(PinContent::from_image(image), window, cx)
                });
                self.dismiss(window, cx);
                return Ok(true);
            }
            return Ok(false);
        }
        history::remember_selection(self.bounds, self.selection.unwrap(), cx);
        match action {
            EditorAction::Save => {
                export::save_content(PinContent::from_image(image), screenshot, cx)
            }
            EditorAction::QuickSave => {
                export::quick_save_content(PinContent::from_image(image), screenshot, cx)
            }
            EditorAction::Print => export::print_image(image, cx),
            EditorAction::Copy => {
                export::copy_image(image.clone(), cx);
                history::remember(image, screenshot, cx);
                self.dismiss(window, cx);
                return Ok(true);
            }
            EditorAction::Pin => {
                history::remember(image.clone(), screenshot, cx);
                let bounds = self.get_screen_selection();
                self.dismiss(window, cx);
                cx.defer(move |cx| pin::open_image(image, bounds, cx));
                return Ok(true);
            }
            EditorAction::Search => {
                export::search_image(image.clone(), cx);
                history::remember(image, screenshot, cx);
                self.dismiss(window, cx);
                return Ok(true);
            }
            _ => unreachable!("not an export action"),
        }
        return Ok(false);
    }

    fn pick_color(&mut self, cx: &mut Context<Self>) {
        let color = self
            .editor
            .get_current_annotation()
            .map_or(self.editor.color, |annotation| annotation.color);
        let (picker, colors) = platform::pick_color(color);
        self.color_task = Some(cx.spawn(async move |this, cx| {
            let _picker = picker;
            while let Ok(color) = colors.recv().await {
                let updated = this.update(cx, |capture, cx| {
                    capture.editor.set_color(color);
                    capture.custom_color = Some(color);
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        }));
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if self.confirming_discard {
            match key {
                "enter" => self.dismiss(window, cx),
                "escape" => {
                    self.confirming_discard = false;
                    cx.notify();
                }
                _ => {}
            }
            return;
        }
        if key == "escape" {
            if self.editor.draft.take().is_some() {
                self.editor.editing_index = None;
                self.text.marked = None;
                cx.notify();
            } else {
                self.cancel(window, cx);
            }
            return;
        }
        if self.get_text_draft().is_some() {
            self.handle_text_key(event, window, cx);
            return;
        }
        if matches!(key, "left" | "right" | "up" | "down") {
            return self.handle_arrow(key, modifiers, cx);
        }
        if modifiers.platform {
            let action = match key {
                "a" => return self.select_all(cx),
                "c" => EditorAction::Copy,
                _ => match get_command_action(key, modifiers.shift) {
                    Some(action) => action,
                    None => return,
                },
            };
            return self.perform_action(action, window, cx);
        }
        if key == "`" && matches!(self.mode, Mode::Capture) {
            let state = cx.global_mut::<AppState>();
            state.capture_cursor = !state.capture_cursor;
            self.dismiss(window, cx);
            cx.defer(start);
            return;
        }
        if key == "c" && self.get_shows_magnifier() {
            if let Some(sample) = self.get_sample() {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    sample.color.format(modifiers.shift),
                ));
            }
            return;
        }
        if self.selection.is_none() {
            let direction = match key {
                "w" => Some(vec2(0.0, -1.0)),
                "a" => Some(vec2(-1.0, 0.0)),
                "s" => Some(vec2(0.0, 1.0)),
                "d" => Some(vec2(1.0, 0.0)),
                _ => None,
            };
            if let Some(direction) = direction {
                self.pointer = (self.pointer + direction).clamp(Vec2::ZERO, self.bounds.size);
                if let Err(error) = platform::move_pointer(self.bounds.pos + self.pointer) {
                    self.error = Some(format!("{error:#}").into());
                }
                return cx.notify();
            }
            if key == "r"
                && let Some(selection) = history::get_last_selection(self.bounds, cx)
            {
                self.selection = Some(selection);
                return cx.notify();
            }
            if key == "space" {
                return self.select_all(cx);
            }
        }
        if let Some(tool) = toolbar::TOOLS.iter().find(|tool| tool.key == key) {
            return self.perform_action(EditorAction::Tool(tool.tool), window, cx);
        }
        match key {
            "," => self.navigate_capture_history(1, window, cx),
            "." => self.navigate_capture_history(-1, window, cx),
            "enter" => self.perform_action(
                if matches!(self.mode, Mode::PinEditor(_)) {
                    EditorAction::Apply
                } else {
                    EditorAction::Copy
                },
                window,
                cx,
            ),
            "backspace" | "delete" => self.perform_action(EditorAction::Delete, window, cx),
            "[" | "]" => {
                self.editor.adjust_size(if key == "[" { -1.0 } else { 1.0 });
                cx.notify();
            }
            "space" => {
                self.show_toolbar = !self.show_toolbar;
                cx.notify();
            }
            _ => {}
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = Some(self.get_canvas());
        cx.notify();
    }

    fn handle_arrow(&mut self, key: &str, modifiers: Modifiers, cx: &mut Context<Self>) {
        let Some(selection) = self
            .selection
            .filter(|selection| layout::get_is_valid_selection(*selection))
        else {
            return;
        };
        let direction = match key {
            "left" => vec2(-1.0, 0.0),
            "right" => vec2(1.0, 0.0),
            "up" => vec2(0.0, -1.0),
            _ => vec2(0.0, 1.0),
        };
        if let Some(index) = self.editor.selected {
            let step = if modifiers.shift { 10.0 } else { 1.0 };
            self.editor.record_change();
            for point in &mut self.editor.layers[index].points {
                *point += direction * step;
            }
        } else if modifiers.platform || modifiers.shift {
            self.selection = Some(layout::create_keyboard_resized_selection(
                selection,
                key,
                modifiers.platform,
                self.get_canvas(),
            ));
        } else {
            self.selection = Some(layout::create_moved_selection(
                selection,
                direction,
                self.get_canvas(),
            ));
        }
        cx.notify();
    }

    fn update_raster_preview(
        &mut self,
        annotations: &[Annotation],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let split = annotations
            .iter()
            .rposition(|annotation| annotation.tool == Tool::Mosaic)
            .map_or(0, |index| index + 1);
        let scene = &annotations[..split];
        let bounds = self.get_canvas();
        let selection = self
            .selection
            .filter(|selection| layout::get_is_valid_selection(*selection));
        let preview = &mut self.raster_preview;
        if split == 0 || selection.is_none() {
            if let Some(image) = preview.image.take() {
                cx.drop_image(image, Some(window));
            }
            preview.scene.clear();
            preview.selection = None;
            return split;
        }
        if preview.task.is_some() || (preview.scene == scene && preview.selection == selection) {
            return split;
        }
        let (Some(selection), Some(screenshot)) = (selection, &self.screenshot) else {
            return split;
        };
        let source = screenshot.image.clone();
        let scene = scene.to_vec();
        let fonts = cx.global::<AppState>().fonts.clone();
        preview.task = Some(cx.spawn(async move |this, cx| {
            let (scene, result) = cx
                .background_executor()
                .spawn(async move {
                    let result =
                        raster::render_selection(&source, bounds, selection, scene.iter(), &fonts)
                            .map(|image| raster::create_render_image(&image));
                    (scene, result)
                })
                .await;
            let _ = this.update(cx, |capture, cx| {
                let preview = &mut capture.raster_preview;
                preview.task = None;
                if capture.selection == Some(selection) {
                    preview.scene = scene;
                    preview.selection = Some(selection);
                    let image = match result {
                        Ok(image) => Some(image),
                        Err(error) => {
                            capture.error = Some(format!("{error:#}").into());
                            None
                        }
                    };
                    if let Some(previous) =
                        std::mem::replace(&mut capture.raster_preview.image, image)
                    {
                        cx.drop_image(previous, None);
                    }
                }
                cx.notify();
            });
        }));
        return split;
    }

    fn sync_intensity_slider(&self, window: &mut Window, cx: &mut Context<Self>) {
        let intensity = self
            .editor
            .get_current_annotation()
            .map_or(self.editor.intensity, |annotation| annotation.intensity);
        if self.intensity.read(cx).value().start() != intensity {
            self.intensity
                .update(cx, |slider, cx| slider.set_value(intensity, window, cx));
        }
    }
}

fn get_command_action(key: &str, shift: bool) -> Option<EditorAction> {
    return Some(match key {
        "z" if shift => EditorAction::Redo,
        "z" => EditorAction::Undo,
        "y" => EditorAction::Redo,
        "s" if shift => EditorAction::QuickSave,
        "s" => EditorAction::Save,
        "p" => EditorAction::Print,
        "t" => EditorAction::Pin,
        _ => return None,
    });
}

fn constrain_point(tool: Tool, start: Vec2, point: Vec2) -> Vec2 {
    let delta = point - start;
    return match tool {
        Tool::Rectangle | Tool::Ellipse => start + delta.signum() * delta.abs().min_element(),
        Tool::Arrow | Tool::Line => {
            let step = std::f32::consts::FRAC_PI_4;
            let angle = (delta.y.atan2(delta.x) / step).round() * step;
            start + Vec2::from_angle(angle) * delta.length()
        }
        _ => point,
    };
}

fn get_tool_family(tool: Tool) -> Tool {
    return match tool {
        Tool::Rectangle | Tool::Ellipse => Tool::Rectangle,
        Tool::Arrow | Tool::Line => Tool::Arrow,
        other => other,
    };
}

fn get_cursor_style(
    drag: Option<DragMode>,
    tool: Tool,
    pointer: Vec2,
    selection: Option<Rect>,
    canvas: Rect,
) -> CursorStyle {
    let handle = match drag {
        Some(DragMode::New | DragMode::Draw | DragMode::Erase) => return CursorStyle::Crosshair,
        Some(DragMode::Move | DragMode::MoveAnnotation(_)) => return CursorStyle::ClosedHand,
        Some(DragMode::SelectText(_, _)) => return CursorStyle::IBeam,
        Some(DragMode::Resize(handle)) => Some(handle),
        None => selection.and_then(|selection| layout::get_resize_handle(pointer, selection)),
    };
    if let Some(handle) = handle {
        return match handle {
            ResizeHandle::Top | ResizeHandle::Bottom => CursorStyle::ResizeUpDown,
            ResizeHandle::Left | ResizeHandle::Right => CursorStyle::ResizeLeftRight,
            ResizeHandle::TopLeft | ResizeHandle::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            ResizeHandle::TopRight | ResizeHandle::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        };
    }
    if selection.is_some_and(|selection| selection.contains(pointer)) {
        return match tool {
            Tool::Select => CursorStyle::OpenHand,
            Tool::Text => CursorStyle::IBeam,
            _ => CursorStyle::Crosshair,
        };
    }
    if !canvas.contains(pointer) {
        return CursorStyle::Arrow;
    }
    return CursorStyle::Crosshair;
}

impl Render for CaptureView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut gpu_annotations: Vec<_> = self.editor.get_visible_annotations().cloned().collect();
        let split = self.update_raster_preview(&gpu_annotations, window, cx);
        self.sync_intensity_slider(window, cx);
        gpu_annotations.drain(..split);
        let selection = self.selection;
        let screen = self.bounds.size;
        let image_bounds = self.get_canvas();
        let border = selection.or(self.get_hovered_window()).or_else(|| {
            self.drag
                .as_ref()
                .filter(|drag| drag.mode == DragMode::New)
                .and_then(|drag| drag.origin)
        });
        let accepts_text = self.get_text_draft().is_some();
        let focus = self.focus.clone();
        let entity = cx.entity();
        let caret = self.get_text_bounds(self.text.get_head(), window);
        let highlights = self.get_text_highlights(window);
        let theme = cx.theme();
        let accent = theme.primary;
        let mut highlight_color = theme.selection;
        highlight_color.a = highlight_color.a.min(0.5);
        let raster_preview = self
            .raster_preview
            .image
            .clone()
            .filter(|_| self.raster_preview.selection == selection)
            .zip(selection);
        return div()
            .id("capture")
            .size_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .cursor(self.get_cursor())
            .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_drag))
            .on_mouse_move(cx.listener(Self::update_drag))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_drag))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::finish_drag))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|capture, _, window, cx| {
                    if capture.editor.draft.is_some() {
                        capture.editor.commit_draft();
                        capture.drag = None;
                        cx.notify();
                    } else {
                        capture.perform_action(EditorAction::Cancel, window, cx);
                    }
                }),
            )
            .on_key_down(cx.listener(Self::handle_key))
            .on_scroll_wheel(cx.listener(|capture, event: &ScrollWheelEvent, _, cx| {
                let delta: f32 = event.delta.pixel_delta(px(16.)).y.into();
                let pointer = capture.get_pointer(event.position);
                if delta == 0.0
                    || !capture
                        .selection
                        .is_some_and(|selection| selection.contains(pointer))
                {
                    return;
                }
                capture.editor.adjust_size(delta.signum());
                cx.notify();
            }))
            .on_modifiers_changed(cx.listener(|capture, _, _, cx| {
                if capture.get_shows_magnifier() {
                    cx.notify();
                }
            }))
            .when_some(self.background.clone(), |root, background| {
                root.child(
                    img(background)
                        .absolute()
                        .left(px(image_bounds.pos.x))
                        .top(px(image_bounds.pos.y))
                        .w(px(image_bounds.size.x))
                        .h(px(image_bounds.size.y)),
                )
            })
            .when_some(raster_preview, |root, (image, selection)| {
                root.child(
                    img(image)
                        .absolute()
                        .left(px(selection.pos.x))
                        .top(px(selection.pos.y))
                        .w(px(selection.size.x))
                        .h(px(selection.size.y)),
                )
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if accepts_text {
                            window.handle_input(
                                &focus,
                                ElementInputHandler::new(bounds, entity.clone()),
                                cx,
                            );
                        }
                        if let Some(selection) = selection {
                            window.with_content_mask(
                                Some(ContentMask {
                                    bounds: ui::to_bounds(selection),
                                }),
                                |window| {
                                    for annotation in &gpu_annotations {
                                        if let Err(error) =
                                            paint::paint_annotation(annotation, window, cx)
                                        {
                                            eprintln!("could not paint annotation: {error:#}");
                                        }
                                    }
                                },
                            );
                        }
                        for rect in layout::get_dim_rects(border, image_bounds) {
                            window.paint_quad(fill(ui::to_bounds(rect), rgba(0x00000066)));
                        }
                        for bounds in &highlights {
                            window.paint_quad(fill(*bounds, highlight_color));
                        }
                        let Some(border) = border else {
                            return;
                        };
                        window.paint_quad(outline(
                            ui::to_bounds(border),
                            accent,
                            BorderStyle::Solid,
                        ));
                        if selection.is_none() {
                            return;
                        }
                        for (_, center) in layout::get_resize_handle_centers(border) {
                            let radius = 3.5;
                            let mut quad = fill(
                                ui::to_bounds(Rect {
                                    pos: center - radius,
                                    size: Vec2::splat(radius * 2.0),
                                }),
                                accent,
                            );
                            quad.corner_radii = Corners::all(px(radius));
                            quad.border_widths = Edges::all(px(1.5));
                            quad.border_color = white();
                            window.paint_quad(quad);
                        }
                    },
                )
                .absolute()
                .size_full(),
            )
            .when_some(border.filter(|_| self.show_toolbar), |root, rect| {
                root.child(self.render_size_badge(rect, window, cx))
            })
            .when_some(selection.zip(self.editor.selected), |root, (_, index)| {
                let rect = text::get_annotation_bounds(&self.editor.layers[index], window);
                root.child(
                    div()
                        .absolute()
                        .left(px(rect.pos.x))
                        .top(px(rect.pos.y))
                        .w(px(rect.size.x))
                        .h(px(rect.size.y))
                        .border_1()
                        .border_dashed()
                        .border_color(accent),
                )
            })
            .when_some(caret.filter(|_| selection.is_some()), |root, caret| {
                root.child(
                    div()
                        .absolute()
                        .left(caret.origin.x)
                        .top(caret.origin.y)
                        .h(caret.size.height)
                        .border_l_2()
                        .border_color(rgb(self.editor.color.0)),
                )
            })
            .when_some(
                selection.filter(|_| self.show_toolbar),
                |root, selection| root.children(self.render_toolbars(selection, cx)),
            )
            .when(self.get_shows_magnifier(), |root| {
                root.when_some(
                    self.get_sample().zip(self.screenshot.as_ref()),
                    |root, (sample, screenshot)| {
                        root.child(magnifier::render(
                            screenshot.image.clone(),
                            sample,
                            magnifier::get_position(self.pointer, screen),
                            window.modifiers().shift,
                            cx,
                        ))
                    },
                )
            })
            .when(self.confirming_discard, |root| {
                root.child(self.render_discard_prompt(selection.unwrap_or(image_bounds), cx))
            })
            .when_some(self.error.clone(), |root, error| {
                root.child(
                    ui::panel(cx)
                        .absolute()
                        .bottom_4()
                        .left_4()
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
    use super::{CaptureView, DragMode, EditorAction, Mode, constrain_point, get_cursor_style};
    use crate::{
        capture::layout::ResizeHandle,
        editor::{Annotation, Editor, Tool},
        geometry::{Color, Rect},
        raster::Screenshot,
    };
    use glam::{Vec2, vec2};
    use gpui_kit::{
        AppContext, CursorStyle, InputEvent, KeyDownEvent, Keystroke, MouseDownEvent,
        MouseMoveEvent, TestAppContext, WindowHandle, point, px, test::TestWindowExt,
    };
    use image::RgbaImage;
    use std::sync::Arc;

    fn create_capture(cx: &mut TestAppContext) -> WindowHandle<CaptureView> {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_global(crate::app::config::Config::default());
        });
        return cx.add_window(|window, cx| {
            CaptureView::new(
                Rect {
                    pos: vec2(0.0, 0.0),
                    size: vec2(200.0, 100.0),
                },
                Mode::Capture,
                None,
                vec2(0.0, 0.0),
                window,
                cx,
            )
        });
    }

    #[gpui_kit::test]
    fn resetting_capture_moves_pointer_to_current_position(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                let bounds = Rect {
                    pos: vec2(100.0, 50.0),
                    size: vec2(200.0, 100.0),
                };
                capture.reset(bounds, Mode::Capture, None, vec2(130.0, 90.0), window, cx);
                assert_eq!(capture.pointer, vec2(30.0, 40.0));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn resetting_capture_cancels_pending_screenshot(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        let (sender, receiver) = async_channel::bounded(1);
        let load = cx.background_spawn(async move { receiver.recv().await.unwrap() });
        window
            .update(cx, |capture, window, cx| {
                capture.load_capture(load, window, cx)
            })
            .unwrap();
        cx.run_until_parked();
        let image = Arc::new(RgbaImage::new(200, 100));
        window
            .update(cx, |capture, window, cx| {
                capture.reset(
                    capture.bounds,
                    Mode::Whiteboard,
                    Some(Screenshot {
                        image: image.clone(),
                        bounds: capture.bounds,
                        windows: Vec::new(),
                    }),
                    vec2(0.0, 0.0),
                    window,
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        assert!(sender.is_closed());
        window
            .update(cx, |capture, _, _| {
                assert!(matches!(capture.mode, Mode::Whiteboard));
                assert!(Arc::ptr_eq(
                    &capture.screenshot.as_ref().unwrap().image,
                    &image
                ));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn command_arrows_grow_selection(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.selection = Some(Rect {
                    pos: vec2(10.0, 10.0),
                    size: vec2(20.0, 20.0),
                });
                for key in ["cmd-left", "cmd-up", "cmd-right", "cmd-down"] {
                    capture.handle_key(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse(key).unwrap(),
                            is_held: false,
                            prefer_character_input: false,
                        },
                        window,
                        cx,
                    );
                }
                assert_eq!(
                    capture.selection,
                    Some(Rect {
                        pos: vec2(9.0, 9.0),
                        size: vec2(22.0, 22.0),
                    })
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn picked_color_stays_in_swatches_after_choosing_a_preset(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, _, _| {
                capture.custom_color = Some(Color(0x123456));
                capture.editor.set_color(Color::WHITE);
                assert_eq!(capture.get_custom_swatch(Color::WHITE), Some(0x123456));
                assert_eq!(capture.get_custom_swatch(Color(0x654321)), Some(0x654321));
                capture.custom_color = None;
                assert_eq!(capture.get_custom_swatch(Color::WHITE), None);
                assert_eq!(capture.get_custom_swatch(Color(0x654321)), Some(0x654321));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn bracket_shortcuts_resize_text_without_changing_stroke_width(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.editor.draft = Some(Annotation {
                    tool: Tool::Text,
                    text: "Text".into(),
                    width: 24.0,
                    ..Default::default()
                });
                capture.editor.commit_draft();
                for (key, width) in [("]", 26.0), ("[", 24.0)] {
                    capture.handle_key(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse(key).unwrap(),
                            is_held: false,
                            prefer_character_input: false,
                        },
                        window,
                        cx,
                    );
                    assert_eq!(capture.editor.layers[0].width, width);
                    assert_eq!(capture.editor.width, 3.0);
                }
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn text_hit_areas_follow_shaped_glyphs(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.selection = Some(capture.bounds);
                capture.editor.tool = Tool::Text;
                capture.editor.draft = Some(Annotation {
                    tool: Tool::Text,
                    points: vec![vec2(20.0, 20.0)],
                    text: "😀😀😀😀😀".into(),
                    width: 24.0,
                    ..Default::default()
                });
                let draft = capture.editor.draft.as_ref().unwrap();
                let width: f32 = super::paint::shape_text_line(draft, &draft.text, 0, window)
                    .width
                    .into();
                let pointer = vec2(20.0 + width - 1.0, 30.0);
                let event = MouseDownEvent {
                    position: point(px(pointer.x), px(pointer.y)),
                    click_count: 1,
                    ..Default::default()
                };
                capture.begin_drag(&event, window, cx);
                assert!(matches!(
                    capture.get_drag_mode(),
                    Some(DragMode::SelectText(_, _))
                ));
                assert_eq!(capture.editor.draft.as_ref().unwrap().text, "😀😀😀😀😀");
                capture.drag = None;
                capture.editor.select_tool(Tool::Select);
                capture.begin_drag(&event, window, cx);
                assert!(matches!(
                    capture.get_drag_mode(),
                    Some(DragMode::MoveAnnotation(0))
                ));
                capture.drag = None;
                capture.editor.select_tool(Tool::Eraser);
                capture.begin_drag(&event, window, cx);
                assert!(capture.editor.layers.is_empty());
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn continuing_line_ignores_underlying_annotations(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.selection = Some(capture.bounds);
                capture.editor.draft = Some(Annotation {
                    tool: Tool::Line,
                    points: vec![vec2(100.0, 30.0), vec2(150.0, 30.0)],
                    ..Default::default()
                });
                capture.editor.commit_draft();
                capture.editor.select_tool(Tool::Line);
                capture.editor.draft = Some(
                    capture
                        .editor
                        .create_annotation(Tool::Line, vec2(20.0, 20.0)),
                );
                capture.begin_drag(
                    &MouseDownEvent {
                        position: point(px(120.0), px(30.0)),
                        click_count: 1,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                assert!(matches!(capture.get_drag_mode(), Some(DragMode::Draw)));
                assert_eq!(capture.editor.selected, None);
                assert_eq!(
                    capture.editor.draft.as_ref().unwrap().points,
                    vec![vec2(20.0, 20.0), vec2(120.0, 30.0)]
                );
                assert_eq!(
                    capture.editor.layers[0].points,
                    vec![vec2(100.0, 30.0), vec2(150.0, 30.0)]
                );
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn escape_with_annotations_asks_before_discarding(cx: &mut TestAppContext) {
        create_capture(cx);
        let mut capture = None;
        let window = cx.add_window(|window, cx| {
            let view = cx.new(|cx| {
                CaptureView::new(
                    Rect {
                        pos: vec2(0.0, 0.0),
                        size: vec2(200.0, 100.0),
                    },
                    Mode::Capture,
                    None,
                    vec2(0.0, 0.0),
                    window,
                    cx,
                )
            });
            capture = Some(view.clone());
            return gpui_kit::base::Root::new(view, window, cx);
        });
        let capture = capture.unwrap();
        cx.update(|cx| {
            capture.update(cx, |capture, _| {
                capture.editor.draft = Some(Annotation {
                    tool: Tool::Rectangle,
                    points: vec![vec2(10.0, 10.0), vec2(50.0, 50.0)],
                    ..Default::default()
                });
                capture.editor.commit_draft();
            })
        });
        let state = |cx: &mut TestAppContext| {
            cx.read(|cx| {
                let capture = capture.read(cx);
                (capture.confirming_discard, capture.editor.layers.len())
            })
        };
        cx.simulate_keystrokes(window.into(), "escape");
        assert_eq!(state(cx), (true, 1));
        cx.simulate_keystrokes(window.into(), "r");
        assert_eq!(state(cx), (true, 1));
        cx.simulate_keystrokes(window.into(), "escape");
        assert_eq!(state(cx), (false, 1));
    }

    #[gpui_kit::test]
    fn text_shortcuts_preserve_clipboard_without_selection(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.editor.draft = Some(Annotation {
                    tool: Tool::Text,
                    text: "Text".into(),
                    ..Default::default()
                });
                for key in ["cmd-c", "cmd-x"] {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("Keep me".into()));
                    capture.handle_key(
                        &KeyDownEvent {
                            keystroke: Keystroke::parse(key).unwrap(),
                            is_held: false,
                            prefer_character_input: false,
                        },
                        window,
                        cx,
                    );
                    assert_eq!(
                        cx.read_from_clipboard()
                            .and_then(|item| item.text())
                            .as_deref(),
                        Some("Keep me")
                    );
                    assert_eq!(capture.editor.draft.as_ref().unwrap().text, "Text");
                }
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn text_shortcuts_honor_quick_save(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.selection = Some(capture.bounds);
                capture.editor.draft = Some(Annotation {
                    tool: Tool::Text,
                    text: "Text".into(),
                    ..Default::default()
                });
                capture.handle_key(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("cmd-shift-s").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert!(matches!(
                    capture.pending_action,
                    Some(EditorAction::QuickSave)
                ));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn keyboard_resize_ignores_collapsed_mouse_drag(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.selection = Some(Rect {
                    pos: vec2(0.0, 20.0),
                    size: vec2(40.0, 40.0),
                });
                capture.begin_drag(
                    &MouseDownEvent {
                        position: point(px(40.0), px(40.0)),
                        click_count: 1,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                capture.update_drag(
                    &MouseMoveEvent {
                        position: point(px(0.0), px(40.0)),
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                let collapsed = capture.selection;
                assert_eq!(collapsed.unwrap().size.x, 0.0);
                capture.handle_key(
                    &KeyDownEvent {
                        keystroke: Keystroke::parse("cmd-left").unwrap(),
                        is_held: false,
                        prefer_character_input: false,
                    },
                    window,
                    cx,
                );
                assert_eq!(capture.selection, collapsed);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn collapsed_selection_skips_blur_preview(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                cx.set_global(crate::app::AppState::default());
                capture.reset(
                    capture.bounds,
                    Mode::Whiteboard,
                    Some(Screenshot {
                        image: Arc::new(RgbaImage::new(200, 100)),
                        bounds: capture.bounds,
                        windows: Vec::new(),
                    }),
                    vec2(0.0, 0.0),
                    window,
                    cx,
                );
                capture.selection = Some(Rect {
                    pos: vec2(10.0, 10.0),
                    size: vec2(0.0, 40.0),
                });
                let blur = Annotation {
                    tool: Tool::Mosaic,
                    points: vec![vec2(10.0, 10.0), vec2(30.0, 30.0)],
                    ..Default::default()
                };
                capture.update_raster_preview(&[blur], window, cx);
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |capture, _, _| assert_eq!(capture.error, None))
            .unwrap();
    }

    #[gpui_kit::test]
    fn editing_actions_cancel_annotation_drags(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, window, cx| {
                capture.selection = Some(capture.bounds);
                for action in [
                    EditorAction::Undo,
                    EditorAction::Delete,
                    EditorAction::Redo,
                    EditorAction::Tool(Tool::Eraser),
                ] {
                    capture.editor = Editor::new();
                    capture.editor.draft = Some(Annotation {
                        tool: Tool::Rectangle,
                        points: vec![vec2(20.0, 20.0), vec2(80.0, 80.0)],
                        ..Default::default()
                    });
                    capture.editor.commit_draft();
                    capture.begin_drag(
                        &MouseDownEvent {
                            position: point(px(20.0), px(20.0)),
                            click_count: 1,
                            ..Default::default()
                        },
                        window,
                        cx,
                    );
                    assert!(matches!(
                        capture.drag.as_ref().map(|drag| drag.mode),
                        Some(DragMode::MoveAnnotation(0))
                    ));
                    capture.perform_action(action, window, cx);
                    capture.update_drag(
                        &MouseMoveEvent {
                            position: point(px(25.0), px(25.0)),
                            ..Default::default()
                        },
                        window,
                        cx,
                    );
                    assert!(capture.drag.is_none());
                }
            })
            .unwrap();
    }

    #[test]
    fn active_drag_cursor_wins_over_hovered_resize_handles() {
        let rect = Rect {
            pos: vec2(20.0, 20.0),
            size: vec2(100.0, 100.0),
        };
        let canvas = Rect {
            pos: Vec2::ZERO,
            size: vec2(200.0, 200.0),
        };
        let cursor = |drag, tool| get_cursor_style(drag, tool, rect.pos, Some(rect), canvas);
        assert_eq!(
            cursor(Some(DragMode::Move), Tool::Select),
            CursorStyle::ClosedHand
        );
        assert_eq!(
            cursor(Some(DragMode::New), Tool::Select),
            CursorStyle::Crosshair
        );
        assert_eq!(
            cursor(Some(DragMode::Draw), Tool::Pen),
            CursorStyle::Crosshair
        );
        assert_eq!(
            cursor(Some(DragMode::Resize(ResizeHandle::Top)), Tool::Select),
            CursorStyle::ResizeUpDown
        );
        assert_eq!(
            cursor(None, Tool::Select),
            CursorStyle::ResizeUpLeftDownRight
        );
        assert_eq!(
            get_cursor_style(None, Tool::Pen, vec2(150.0, 150.0), Some(rect), canvas),
            CursorStyle::Crosshair
        );
        assert_eq!(
            get_cursor_style(None, Tool::Pen, vec2(250.0, 150.0), Some(rect), canvas),
            CursorStyle::Arrow
        );
    }

    #[gpui_kit::test]
    fn windows_are_hoverable_before_the_screenshot_arrives(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, _, _| {
                let terminal = Rect {
                    pos: vec2(10.0, 10.0),
                    size: vec2(80.0, 40.0),
                };
                capture.windows = vec![terminal];
                capture.pointer = vec2(30.0, 30.0);
                assert!(capture.screenshot.is_none());
                assert_eq!(capture.get_hovered_window(), Some(terminal));
                capture.pointer = vec2(150.0, 80.0);
                assert_eq!(capture.get_hovered_window(), None);
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn toolbar_is_measured_while_hidden_on_the_first_frame(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        window
            .update(cx, |capture, _, cx| {
                cx.set_global(crate::app::AppState::default());
                capture.selection = Some(Rect {
                    pos: vec2(20.0, 20.0),
                    size: vec2(100.0, 50.0),
                });
                assert!(capture.toolbar_size.get().is_none());
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
            .unwrap();
        window
            .update(cx, |capture, _, _| {
                assert!(capture.toolbar_size.get().is_some_and(|size| size.x > 0.0));
            })
            .unwrap();
    }

    #[gpui_kit::test]
    fn scrolling_inside_the_selection_changes_stroke_width(cx: &mut TestAppContext) {
        let window = create_capture(cx);
        let width = window
            .update(cx, |capture, _, cx| {
                cx.set_global(crate::app::AppState::default());
                capture.selection = Some(Rect {
                    pos: vec2(20.0, 20.0),
                    size: vec2(100.0, 50.0),
                });
                capture.editor.select_tool(Tool::Pen);
                capture.show_toolbar = false;
                capture.editor.get_current_width()
            })
            .unwrap();
        let scroll = |cx: &mut TestAppContext, x: f32, delta: f32| {
            cx.update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                window.dispatch_event(
                    gpui_kit::ScrollWheelEvent {
                        position: point(px(x), px(40.0)),
                        delta: gpui_kit::ScrollDelta::Pixels(point(px(0.0), px(delta))),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        };
        scroll(cx, 60.0, 10.0);
        scroll(cx, 60.0, 10.0);
        scroll(cx, 180.0, 10.0);
        let current = |cx: &mut TestAppContext| {
            window
                .update(cx, |capture, _, _| capture.editor.get_current_width())
                .unwrap()
        };
        assert_eq!(current(cx), width + 2.0);
        scroll(cx, 60.0, -10.0);
        assert_eq!(current(cx), width + 1.0);
    }

    #[gpui_kit::test]
    fn image_placed_inside_overlay_limits_selection_and_exports_at_full_resolution(
        cx: &mut TestAppContext,
    ) {
        let window = create_capture(cx);
        let image_bounds = Rect {
            pos: vec2(40.0, 20.0),
            size: vec2(50.0, 25.0),
        };
        let canvas = Rect {
            pos: image_bounds.pos,
            size: image_bounds.size,
        };
        window
            .update(cx, |capture, window, cx| {
                cx.set_global(crate::app::AppState::default());
                capture.reset(
                    capture.bounds,
                    Mode::Whiteboard,
                    Some(Screenshot {
                        image: Arc::new(RgbaImage::new(100, 50)),
                        bounds: image_bounds,
                        windows: Vec::new(),
                    }),
                    vec2(0.0, 0.0),
                    window,
                    cx,
                );
                assert_eq!(capture.selection, Some(canvas));
                capture.begin_drag(
                    &MouseDownEvent {
                        button: gpui_kit::MouseButton::Left,
                        position: point(px(150.0), px(80.0)),
                        click_count: 1,
                        ..Default::default()
                    },
                    window,
                    cx,
                );
                assert!(capture.drag.is_none());
                assert_eq!(capture.selection, Some(canvas));
                capture.select_all(cx);
                assert_eq!(capture.selection, Some(canvas));
                assert_eq!(
                    capture.create_export_image(cx).unwrap().dimensions(),
                    (100, 50)
                );
            })
            .unwrap();
    }

    #[test]
    fn shift_constrains_shapes_to_squares_and_lines_to_45_degrees() {
        let start = vec2(10.0, 10.0);
        assert_eq!(
            constrain_point(Tool::Rectangle, start, vec2(40.0, 20.0)),
            vec2(20.0, 20.0)
        );
        let line = constrain_point(Tool::Line, start, vec2(50.0, 12.0));
        assert!((line.y - 10.0).abs() < 0.001);
        assert_eq!(
            constrain_point(Tool::Pen, start, vec2(3.0, 4.0)),
            vec2(3.0, 4.0)
        );
    }
}
