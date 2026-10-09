use crate::{
    app::{self, AppState},
    editor::{Annotation, BlurMode, Editor, Screenshot, TextAttribute, Tool, get_byte_offset},
    geometry::{Color, Rect},
    lens, pin, raster,
};
use glam::{Vec2, vec2};
use gpui::{prelude::*, *};
use image::RgbaImage;
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

const HANDLE_RADIUS: f32 = 5.0;
const HANDLE_HIT_RADIUS: f32 = 4.0;
const BAR_WIDTH: f32 = 16.0 * 28.0 + 16.0 + 7.0 + 2.0 * (4.0 + 1.0);
const BAR_HEIGHT: f32 = 36.0;
const PALETTE_HEIGHT: f32 = 38.0;
const MAGNIFIER_CELL: f32 = 8.0;
const MAGNIFIER_COLUMNS: i32 = 17;
const MAGNIFIER_ROWS: i32 = 13;
const ACCENT: u32 = 0x327fff;
const COLORS: [u32; 8] = [
    0xff4b4b,
    0xffa62b,
    0xffdf38,
    0x45ce88,
    0x327fff,
    0xbf78ff,
    Color::WHITE.0,
    0x19191c,
];

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
#[derive(Clone, Copy, Debug, PartialEq)]
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
#[derive(Clone, Copy)]
enum Action {
    Tool(Tool),
    Undo,
    Redo,
    Search,
    Cancel,
    Pin,
    Save,
    Copy,
    Variant(Tool),
    Filled(bool),
    BlurMode(BlurMode),
    TextStyle(TextAttribute),
    ColorPicker,
    Layers,
    Delete,
}

pub struct SelectionState {
    screenshot: Option<Screenshot>,
    bounds: Rect,
    background: Option<Arc<RenderImage>>,
    preview: Option<Arc<RenderImage>>,
    preview_scene: Vec<Annotation>,
    preview_selection: Option<Rect>,
    preview_task: Option<Task<()>>,
    selection: Option<Rect>,
    editor: Editor,
    pointer: Vec2,
    start: Vec2,
    drag_origin: Option<Rect>,
    annotation_origin: Vec<Vec2>,
    drag_mode: Option<DragMode>,
    drag_recorded: bool,
    focus: FocusHandle,
    error: Option<String>,
    text_range: Range<usize>,
    text_reversed: bool,
    insertion_format: Option<crate::editor::TextFormat>,
    marked_range: Option<Range<usize>>,
    pending_action: Option<Action>,
    color_task: Option<Task<()>>,
    show_layers: bool,
    layer_drop_target: Option<(u64, bool)>,
    intensity_drag: bool,
    capture_history_index: usize,
    capture_history: Vec<Screenshot>,
}

impl SelectionState {
    pub fn new(bounds: Rect, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        return Self {
            screenshot: None,
            bounds,
            background: None,
            preview: None,
            preview_scene: Vec::new(),
            preview_selection: None,
            preview_task: None,
            selection: None,
            editor: Editor::new(),
            pointer: vec2(
                window.mouse_position().x.into(),
                window.mouse_position().y.into(),
            )
            .clamp(Vec2::ZERO, bounds.size),
            start: Vec2::ZERO,
            drag_origin: None,
            annotation_origin: Vec::new(),
            drag_mode: None,
            drag_recorded: false,
            focus,
            error: None,
            text_range: 0..0,
            text_reversed: false,
            insertion_format: None,
            marked_range: None,
            pending_action: None,
            color_task: None,
            show_layers: false,
            layer_drop_target: None,
            intensity_drag: false,
            capture_history_index: 0,
            capture_history: Vec::new(),
        };
    }

    pub fn activate(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screenshot.is_none() {
            return;
        }
        cx.activate(true);
        window.activate_window();
        window.focus(&self.focus);
    }

    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if cx.global::<AppState>().selection.map(AnyWindowHandle::from)
            == Some(window.window_handle())
        {
            cx.global_mut::<AppState>().selection = None;
        }
        crate::platform::hide_overlay(window);
        self.reset_for_capture(self.bounds, window, cx);
    }

    pub fn reset_for_capture(&mut self, bounds: Rect, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(image) = self.background.take() {
            cx.drop_image(image, Some(window));
        }
        if let Some(image) = self.preview.take() {
            cx.drop_image(image, Some(window));
        }
        *self = Self::new(bounds, window, cx);
        cx.notify();
    }

    pub fn complete_capture(
        &mut self,
        screenshot: Screenshot,
        background: Arc<RenderImage>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if screenshot.bounds != self.bounds {
            self.dismiss(window, cx);
            app::show_error("Display configuration changed; capture again".into(), cx);
            return;
        }
        self.capture_history = std::iter::once(screenshot.clone())
            .chain(cx.global::<AppState>().capture_history.iter().cloned())
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
        let Some(index) =
            get_capture_history_index(&bounds, self.capture_history_index, direction, self.bounds)
        else {
            return;
        };
        let screenshot = self.capture_history[index].clone();
        let background = raster::create_render_image(&screenshot.image);
        if let Some(image) = self.background.take() {
            cx.drop_image(image, Some(window));
        }
        if let Some(image) = self.preview.take() {
            cx.drop_image(image, Some(window));
        }
        let bounds = self.bounds;
        let history = std::mem::take(&mut self.capture_history);
        *self = Self::new(bounds, window, cx);
        self.capture_history = history;
        self.capture_history_index = index;
        self.screenshot = Some(screenshot);
        self.background = Some(background);
        cx.notify();
    }

    fn begin_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.finish_settings_change();
        self.insertion_format = None;
        window.focus(&self.focus);
        self.pointer = vec2(event.position.x.into(), event.position.y.into())
            .clamp(Vec2::ZERO, self.bounds.size);
        self.start = self.pointer;
        self.drag_recorded = false;
        if self.editor.draft.as_ref().is_some_and(|draft| {
            draft.tool == Tool::Text && draft.get_bounds().intersects(self.pointer)
        }) {
            self.begin_text_selection(event, window, cx);
            return;
        }
        self.editor.commit_draft();
        if let Some(selection) = self.selection {
            if let Some(handle) = get_resize_handle(self.pointer, selection) {
                self.drag_origin = Some(selection);
                self.drag_mode = Some(DragMode::Resize(handle));
                cx.notify();
                return;
            }
            if selection.intersects(self.pointer) {
                if self.editor.tool != Tool::Eraser
                    && (self.editor.tool == Tool::Text || event.click_count >= 2)
                {
                    if let Some(index) = self
                        .editor
                        .get_annotation_at(self.pointer)
                        .filter(|index| self.editor.layers[*index].tool == Tool::Text)
                    {
                        self.editor.start_editing_text(index);
                        self.begin_text_selection(event, window, cx);
                        return;
                    }
                }
                if self.editor.tool != Tool::Select
                    && self.editor.tool != Tool::Text
                    && self.editor.tool != Tool::Eraser
                {
                    if let Some(index) = self.editor.get_annotation_at(self.pointer) {
                        if get_tool_family(self.editor.layers[index].tool)
                            == get_tool_family(self.editor.tool)
                        {
                            self.editor.selected = Some(index);
                            self.annotation_origin = self.editor.layers[index].points.clone();
                            self.drag_mode = Some(DragMode::MoveAnnotation(index));
                            cx.notify();
                            return;
                        }
                    }
                }
                match self.editor.tool {
                    Tool::Select => {
                        if let Some(index) = self.editor.get_annotation_at(self.pointer) {
                            self.editor.selected = Some(index);
                            let annotation = &self.editor.layers[index];
                            self.editor.color = annotation.color;
                            match annotation.tool {
                                Tool::Text => self.editor.text_size = annotation.width,
                                Tool::Highlighter => {
                                    self.editor.highlighter_width = annotation.width
                                }
                                _ => self.editor.width = annotation.width,
                            }
                            self.annotation_origin = self.editor.layers[index].points.clone();
                            self.drag_mode = Some(DragMode::MoveAnnotation(index));
                        } else {
                            self.editor.selected = None;
                            self.drag_origin = Some(selection);
                            self.drag_mode = Some(DragMode::Move);
                        }
                    }
                    Tool::Eraser => {
                        self.drag_mode = Some(DragMode::Erase);
                        self.erase_at_pointer();
                    }
                    tool => {
                        self.editor.selected = None;
                        if tool == Tool::Text {
                            self.text_range = 0..0;
                            self.text_reversed = false;
                            self.marked_range = None;
                        }
                        self.editor.draft = Some(Annotation {
                            tool,
                            points: vec![self.pointer],
                            color: self.editor.color,
                            width: if tool == Tool::Text {
                                self.editor.text_size
                            } else if tool == Tool::Highlighter {
                                self.editor.highlighter_width
                            } else {
                                self.editor.width
                            },
                            text: String::new(),
                            filled: self.editor.filled,
                            blur_mode: self.editor.blur_mode,
                            intensity: self.editor.intensity,
                            text_format: self.editor.text_format,
                            ..Default::default()
                        });
                        if tool != Tool::Text {
                            self.drag_mode = Some(DragMode::Draw);
                        }
                    }
                }
                cx.notify();
                return;
            }
        }
        self.drag_origin = self.get_hovered_window();
        self.drag_mode = Some(DragMode::New);
        self.selection = None;
        self.editor.selected = None;
        cx.notify();
    }

    fn begin_text_selection(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(offset) = self.character_index_for_point(event.position, window, cx) {
            let text = &self.editor.draft.as_ref().unwrap().text;
            self.text_range = create_clicked_text_range(text, offset, event.click_count);
            self.text_reversed = false;
            self.marked_range = None;
            self.drag_mode = Some(DragMode::SelectText(
                self.text_range.start,
                self.text_range.end,
            ));
        }
        cx.notify();
    }

    fn update_drag(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.get_cursor();
        let hovered = self.get_hovered_window();
        let previous_pointer = self.pointer;
        self.pointer = vec2(event.position.x.into(), event.position.y.into())
            .clamp(Vec2::ZERO, self.bounds.size);
        if self.drag_mode.is_none() {
            if cursor != self.get_cursor()
                || hovered != self.get_hovered_window()
                || (self.get_should_show_magnifier() && previous_pointer != self.pointer)
            {
                cx.notify();
            }
            return;
        }
        let mut point = self.pointer;
        if let Some(selection) = self.selection {
            if matches!(self.drag_mode, Some(DragMode::Draw | DragMode::Erase)) {
                point = point.clamp(selection.pos, selection.pos + selection.size);
            }
        }
        if event.modifiers.shift && self.drag_mode == Some(DragMode::Draw) {
            let delta = point - self.start;
            match self.editor.tool {
                Tool::Rectangle | Tool::Ellipse => {
                    point = self.start + delta.signum() * delta.abs().min_element();
                }
                Tool::Arrow | Tool::Line => {
                    let angle = (delta.y.atan2(delta.x) / std::f32::consts::FRAC_PI_4).round()
                        * std::f32::consts::FRAC_PI_4;
                    point = self.start + vec2(angle.cos(), angle.sin()) * delta.length();
                }
                _ => {}
            }
        }
        match self.drag_mode {
            Some(DragMode::New) => {
                self.selection = Some(create_selection_rect(self.start, point))
                    .filter(|rect| rect.size.min_element() >= 2.0);
            }
            Some(DragMode::Move) => {
                if let Some(origin) = self.drag_origin {
                    self.selection = Some(create_moved_selection(
                        origin,
                        self.start,
                        point,
                        self.bounds.size,
                    ));
                }
            }
            Some(DragMode::Resize(handle)) => {
                if let Some(origin) = self.drag_origin {
                    let center = get_resize_handle_centers(origin)
                        .into_iter()
                        .find(|(candidate, _)| *candidate == handle)
                        .unwrap()
                        .1;
                    self.selection = Some(create_resized_selection(
                        origin,
                        handle,
                        (center + point - self.start).clamp(Vec2::ZERO, self.bounds.size),
                    ));
                }
            }
            Some(DragMode::MoveAnnotation(index)) => {
                if let Some(selection) = self.selection {
                    let delta = (point - self.start).clamp(
                        selection.pos - self.start,
                        selection.pos + selection.size - self.start,
                    );
                    if delta != Vec2::ZERO && !self.drag_recorded {
                        self.editor.record_change();
                        self.drag_recorded = true;
                    }
                    self.editor.layers[index].points = self
                        .annotation_origin
                        .iter()
                        .map(|point| *point + delta)
                        .collect();
                }
            }
            Some(DragMode::Draw) => {
                if let Some(draft) = &mut self.editor.draft {
                    if matches!(draft.tool, Tool::Pen | Tool::Highlighter) {
                        if draft.points.last().unwrap().distance(point) >= 0.5 {
                            draft.points.push(point);
                        }
                    } else if draft.points.len() == 1 {
                        draft.points.push(point);
                    } else {
                        draft.points[1] = point;
                    }
                }
            }
            Some(DragMode::SelectText(start, end)) => {
                self.insertion_format = None;
                if let Some(offset) = self.character_index_for_point(event.position, window, cx) {
                    self.text_range = start.min(offset)..end.max(offset);
                    self.text_reversed = offset < start;
                }
            }
            Some(DragMode::Erase) => self.erase_at_pointer(),
            None => {}
        }
        cx.notify();
    }

    fn finish_drag(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag_mode.is_some() {
            self.update_drag(
                &MouseMoveEvent {
                    position: event.position,
                    modifiers: event.modifiers,
                    ..Default::default()
                },
                window,
                cx,
            );
        }
        if self.drag_mode == Some(DragMode::New)
            && self
                .selection
                .is_none_or(|selection| selection.size.min_element() < 2.0)
        {
            self.selection = self.drag_origin;
        }
        self.selection = self
            .selection
            .filter(|selection| selection.size.min_element() >= 2.0);
        if self.drag_mode == Some(DragMode::Draw) {
            self.editor.commit_draft();
        }
        self.drag_mode = None;
        self.drag_origin = None;
        cx.notify();
    }

    fn erase_at_pointer(&mut self) {
        if let Some(index) = self.editor.get_annotation_at(self.pointer) {
            if !self.drag_recorded {
                self.editor.record_change();
                self.drag_recorded = true;
            }
            self.editor.layers.remove(index);
            self.editor.selected = None;
        }
    }

    fn get_image(&mut self, cx: &App) -> Result<RgbaImage, String> {
        self.editor.commit_draft();
        let selection = self.selection.ok_or("Select an area first")?;
        let screenshot = self.screenshot.as_ref().ok_or("Capture is still loading")?;
        return raster::render_selection(
            &screenshot.image,
            self.bounds,
            selection,
            self.editor.get_visible_annotations(),
            &cx.global::<AppState>().fonts,
        );
    }

    fn perform_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        if self.screenshot.is_none()
            && self.selection.is_some()
            && matches!(
                action,
                Action::Copy | Action::Save | Action::Pin | Action::Search
            )
        {
            self.pending_action = Some(action);
            return;
        }
        match action {
            Action::Variant(tool) => self.editor.set_variant(tool),
            Action::Filled(filled) => self.editor.set_filled(filled),
            Action::BlurMode(mode) => self.editor.set_blur_mode(mode),
            Action::TextStyle(attribute) => self.toggle_text_attribute(attribute),
            Action::ColorPicker => {
                self.color_task = None;
                let color = self
                    .editor
                    .get_current_annotation()
                    .map_or(self.editor.color, |annotation| annotation.color);
                let (picker, receiver) = crate::platform::pick_color(color);
                self.color_task = Some(cx.spawn(async move |this, cx| {
                    let _picker = picker;
                    while let Ok(color) = receiver.recv().await {
                        if this
                            .update(cx, |state, cx| {
                                state.editor.set_color(color);
                                cx.notify();
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            Action::Layers => self.show_layers = !self.show_layers,
            Action::Delete => self.editor.delete_selected(),
            Action::Tool(tool) => {
                self.editor.select_tool(tool);
                if tool == Tool::Highlighter {
                    self.editor.color = Color(0xffdf38);
                }
            }
            Action::Undo => self.editor.undo_change(),
            Action::Redo => self.editor.redo_change(),
            Action::Cancel => {
                self.dismiss(window, cx);
                return;
            }
            Action::Copy | Action::Save | Action::Pin | Action::Search => {
                match self.get_image(cx) {
                    Ok(image) => {
                        let image = Arc::new(image);
                        match action {
                            Action::Copy => {
                                if let Err(error) = app::copy_image(&image, cx) {
                                    self.error = Some(error);
                                    cx.notify();
                                    return;
                                }
                                if let Some(screenshot) = self.screenshot.clone() {
                                    app::remember_capture_frame(screenshot, cx);
                                }
                                app::remember_capture(image, cx);
                                self.dismiss(window, cx);
                            }
                            Action::Save => app::save_image(image, self.screenshot.clone(), cx),
                            Action::Pin => {
                                if let Some(screenshot) = self.screenshot.clone() {
                                    app::remember_capture_frame(screenshot, cx);
                                }
                                app::remember_capture(image.clone(), cx);
                                let selection = self.selection.unwrap();
                                let bounds = Rect {
                                    pos: self.bounds.pos + selection.pos,
                                    size: selection.size,
                                };
                                cx.spawn(async move |_, cx| {
                                    let _ = cx.update(|cx| pin::open_pin(image, Some(bounds), cx));
                                })
                                .detach();
                                self.dismiss(window, cx);
                            }
                            Action::Search => match raster::encode_png(&image)
                                .and_then(|image| lens::create_handoff(&image))
                            {
                                Ok(path) => {
                                    if let Some(screenshot) = self.screenshot.clone() {
                                        app::remember_capture_frame(screenshot, cx);
                                    }
                                    app::remember_capture(image, cx);
                                    cx.open_with_system(&path);
                                    lens::schedule_cleanup(path);
                                    self.dismiss(window, cx);
                                }
                                Err(error) => self.error = Some(error),
                            },
                            _ => {}
                        }
                    }
                    Err(error) => self.error = Some(error),
                }
            }
        }
        if self.editor.draft.is_none() {
            self.marked_range = None;
        }
        window.focus(&self.focus);
        cx.notify();
    }

    fn handle_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if key == "escape" {
            if self.editor.draft.take().is_some() {
                self.editor.editing_index = None;
                self.marked_range = None;
                cx.notify();
            } else {
                self.dismiss(window, cx);
            }
            return;
        }
        if self
            .editor
            .draft
            .as_ref()
            .is_some_and(|draft| draft.tool == Tool::Text)
        {
            self.handle_text_key(event, window, cx);
            return;
        }
        if modifiers.platform {
            match key {
                "z" => self.perform_action(
                    if modifiers.shift {
                        Action::Redo
                    } else {
                        Action::Undo
                    },
                    window,
                    cx,
                ),
                "y" => self.perform_action(Action::Redo, window, cx),
                "c" => self.perform_action(Action::Copy, window, cx),
                "s" => self.perform_action(Action::Save, window, cx),
                "p" => self.perform_action(Action::Pin, window, cx),
                "a" => {
                    self.selection = Some(Rect {
                        pos: Vec2::ZERO,
                        size: self.bounds.size,
                    });
                    cx.notify();
                }
                _ => {}
            }
            return;
        }
        if key == "c" && self.get_should_show_magnifier() {
            if let Some((_, _, color)) = self.screenshot.as_ref().and_then(|screenshot| {
                get_sampled_pixel(&screenshot.image, self.bounds.size, self.pointer)
            }) {
                cx.write_to_clipboard(ClipboardItem::new_string(create_color_label(
                    color,
                    modifiers.shift,
                )));
                cx.stop_propagation();
            }
            return;
        }
        if self.selection.is_none() && matches!(key, "w" | "a" | "s" | "d") {
            let delta = match key {
                "w" => vec2(0.0, -1.0),
                "a" => vec2(-1.0, 0.0),
                "s" => vec2(0.0, 1.0),
                _ => vec2(1.0, 0.0),
            };
            self.pointer = create_moved_pointer(self.pointer, delta, self.bounds.size);
            if let Err(error) = crate::platform::move_pointer(self.bounds.pos + self.pointer) {
                self.error = Some(error);
            }
            cx.notify();
            return;
        }
        let tool = match key {
            "v" => Some(Tool::Select),
            "r" => Some(Tool::Rectangle),
            "o" => Some(Tool::Ellipse),
            "a" => Some(Tool::Arrow),
            "l" => Some(Tool::Line),
            "b" => Some(Tool::Pen),
            "h" => Some(Tool::Highlighter),
            "t" => Some(Tool::Text),
            "m" => Some(Tool::Mosaic),
            "e" => Some(Tool::Eraser),
            _ => None,
        };
        if let Some(tool) = tool {
            self.perform_action(Action::Tool(tool), window, cx);
            return;
        }
        match key {
            "," => self.navigate_capture_history(1, window, cx),
            "." => self.navigate_capture_history(-1, window, cx),
            "enter" => self.perform_action(Action::Copy, window, cx),
            "backspace" | "delete" => {
                self.editor.delete_selected();
                cx.notify();
            }
            "[" => {
                self.editor.set_width(self.editor.get_current_width() - 1.0);
                cx.notify();
            }
            "]" => {
                self.editor.set_width(self.editor.get_current_width() + 1.0);
                cx.notify();
            }
            "space" if self.selection.is_none() => {
                self.selection = Some(Rect {
                    pos: Vec2::ZERO,
                    size: self.bounds.size,
                });
                cx.notify();
            }
            "left" | "right" | "up" | "down" => {
                if let Some(selection) = self.selection {
                    let step = if modifiers.shift { 10.0 } else { 1.0 };
                    let delta = match key {
                        "left" => vec2(-step, 0.0),
                        "right" => vec2(step, 0.0),
                        "up" => vec2(0.0, -step),
                        _ => vec2(0.0, step),
                    };
                    if let Some(index) = self.editor.selected {
                        self.editor.record_change();
                        for point in &mut self.editor.layers[index].points {
                            *point += delta;
                        }
                    } else {
                        self.selection = Some(create_moved_selection(
                            selection,
                            Vec2::ZERO,
                            delta,
                            self.bounds.size,
                        ));
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn build_button(
        &self,
        icon: &'static str,
        label: &'static str,
        action: Action,
        selected: bool,
        enabled: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(icon)
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.0))
            .flex_shrink_0()
            .rounded_sm()
            .cursor_pointer()
            .bg(if selected { rgb(ACCENT) } else { rgba(0) })
            .text_color(if enabled {
                rgb(0xf2f2f2)
            } else {
                rgb(0x66666d)
            })
            .hover(move |s| s.bg(rgb(if selected { 0x448cff } else { 0x414148 })))
            .child(svg().path(icon).size(px(18.0)).text_color(if enabled {
                rgb(0xf2f2f2)
            } else {
                rgb(0x66666d)
            }))
            .tooltip(move |_, cx| cx.new(|_| TooltipLabel(label.into())).into())
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |state, _, window, cx| {
                cx.stop_propagation();
                if enabled {
                    state.perform_action(action, window, cx);
                }
            }))
    }

    fn handle_text_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let text = self.editor.draft.as_ref().unwrap().text.clone();
        if modifiers.platform {
            match key {
                "b" => self.toggle_text_attribute(TextAttribute::Bold),
                "i" => self.toggle_text_attribute(TextAttribute::Italic),
                "u" => self.toggle_text_attribute(TextAttribute::Underline),
                "a" => {
                    self.insertion_format = None;
                    self.text_range = 0..text.encode_utf16().count();
                    self.text_reversed = false;
                }
                "c" | "x" => {
                    let start = get_byte_offset(&text, self.text_range.start);
                    let end = get_byte_offset(&text, self.text_range.end);
                    cx.write_to_clipboard(ClipboardItem::new_string(text[start..end].into()));
                    if key == "x" {
                        self.replace_text_in_range(None, "", window, cx);
                    }
                }
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.replace_text_in_range(None, &text, window, cx);
                    }
                }
                "z" | "y" => self.perform_action(
                    if key == "y" || modifiers.shift {
                        Action::Redo
                    } else {
                        Action::Undo
                    },
                    window,
                    cx,
                ),
                "s" => self.perform_action(Action::Save, window, cx),
                "p" => self.perform_action(Action::Pin, window, cx),
                _ => return,
            }
        } else {
            match key {
                "enter" if modifiers.shift => self.replace_text_in_range(None, "\n", window, cx),
                "enter" if self.marked_range.is_some() => return,
                "enter" => {
                    self.editor.commit_draft();
                    self.marked_range = None;
                }
                "backspace" | "delete" => {
                    let mut range = self.text_range.clone();
                    if range.is_empty() {
                        let byte = get_byte_offset(&text, range.start);
                        if key == "backspace" {
                            if let Some((start, _)) = text[..byte].grapheme_indices(true).last() {
                                range.start = text[..start].encode_utf16().count();
                            }
                        } else if let Some(grapheme) = text[byte..].graphemes(true).next() {
                            range.end += grapheme.encode_utf16().count();
                        }
                    }
                    self.replace_text_in_range(Some(range), "", window, cx);
                }
                "left" | "right" => {
                    self.insertion_format = None;
                    let offset = if self.text_reversed {
                        self.text_range.start
                    } else {
                        self.text_range.end
                    };
                    let byte = get_byte_offset(&text, offset);
                    let next = if !modifiers.shift && !self.text_range.is_empty() {
                        if key == "left" {
                            self.text_range.start
                        } else {
                            self.text_range.end
                        }
                    } else if key == "left" {
                        text[..byte]
                            .grapheme_indices(true)
                            .last()
                            .map_or(0, |(start, _)| text[..start].encode_utf16().count())
                    } else {
                        offset
                            + text[byte..]
                                .graphemes(true)
                                .next()
                                .map_or(0, |g| g.encode_utf16().count())
                    };
                    if modifiers.shift {
                        let anchor = if self.text_reversed {
                            self.text_range.end
                        } else {
                            self.text_range.start
                        };
                        self.text_range = anchor.min(next)..anchor.max(next);
                        self.text_reversed = next < anchor;
                    } else {
                        self.text_range = next..next;
                        self.text_reversed = false;
                    }
                }
                "home" => {
                    self.insertion_format = None;
                    self.text_range = 0..0;
                    self.text_reversed = false;
                }
                "end" => {
                    self.insertion_format = None;
                    let end = text.encode_utf16().count();
                    self.text_range = end..end;
                    self.text_reversed = false;
                }
                _ => return,
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn get_text_bounds(&self, offset: usize, window: &mut Window) -> Option<Bounds<Pixels>> {
        let draft = self
            .editor
            .draft
            .as_ref()
            .filter(|draft| draft.tool == Tool::Text)?;
        let byte = get_byte_offset(&draft.text, offset);
        let prefix = &draft.text[..byte];
        let line = prefix.rsplit('\n').next().unwrap_or("");
        let shaped = window.text_system().shape_line(
            line.to_string().into(),
            px(draft.width),
            &create_text_runs(draft, byte - line.len()..byte),
            None,
        );
        return Some(Bounds::new(
            point(
                px(draft.points[0].x) + shaped.width,
                px(draft.points[0].y
                    + prefix.bytes().filter(|byte| *byte == b'\n').count() as f32
                        * draft.width
                        * 1.3),
            ),
            size(px(1.0), px(draft.width * 1.3)),
        ));
    }

    fn get_current_text_format(&self) -> crate::editor::TextFormat {
        let Some(annotation) = self.editor.get_current_annotation() else {
            return self.editor.text_format;
        };
        if self.editor.draft.is_some() && self.text_range.is_empty() {
            if let Some(format) = self.insertion_format {
                return format;
            }
            let byte = get_byte_offset(&annotation.text, self.text_range.start);
            return annotation
                .get_format(annotation.text[..byte].chars().count().saturating_sub(1));
        }
        let range = if self.editor.draft.is_some() {
            get_byte_offset(&annotation.text, self.text_range.start)
                ..get_byte_offset(&annotation.text, self.text_range.end)
        } else {
            0..annotation.text.len()
        };
        let runs = annotation.get_format_runs(range);
        if runs.is_empty() {
            return annotation.text_format;
        }
        return crate::editor::TextFormat {
            bold: runs.iter().all(|(_, format)| format.bold),
            italic: runs.iter().all(|(_, format)| format.italic),
            underline: runs.iter().all(|(_, format)| format.underline),
        };
    }

    fn toggle_text_attribute(&mut self, attribute: TextAttribute) {
        let mut format = self.get_current_text_format();
        let enabled = !format.get_attribute(attribute);
        format.set_attribute(attribute, enabled);
        self.editor.text_format = format;
        if let Some(draft) = self
            .editor
            .draft
            .as_mut()
            .filter(|draft| draft.tool == Tool::Text)
        {
            if self.text_range.is_empty() {
                self.insertion_format = Some(format);
                if draft.text.is_empty() {
                    draft.text_format = format;
                }
                return;
            }
            let start = get_byte_offset(&draft.text, self.text_range.start);
            let end = get_byte_offset(&draft.text, self.text_range.end);
            draft.set_text_attribute(start..end, attribute, enabled);
            self.insertion_format = None;
        } else {
            self.editor
                .update_current_annotation("text-format", |annotation| {
                    if annotation.tool == Tool::Text {
                        annotation.set_text_attribute(0..annotation.text.len(), attribute, enabled);
                    }
                });
        }
    }

    fn build_intensity_slider(&self, cx: &Context<Self>) -> impl IntoElement {
        let value = self
            .editor
            .get_current_annotation()
            .map_or(self.editor.intensity, |annotation| annotation.intensity);
        let bounds = std::rc::Rc::new(std::cell::Cell::new(None::<Bounds<Pixels>>));
        let paint_bounds = bounds.clone();
        let down_bounds = bounds.clone();
        let move_bounds = bounds.clone();
        div()
            .id("intensity")
            .w(px(146.0))
            .h(px(28.0))
            .flex_shrink_0()
            .cursor(CursorStyle::Arrow)
            .child(
                canvas(
                    move |area, _, _| {
                        paint_bounds.set(Some(area));
                    },
                    move |area, _, window, _| {
                        let left = area.origin.x + px(8.0);
                        let y = area.origin.y + px(14.0);
                        let width = px(96.0);
                        let x = left + width * ((value - 1.0) / 47.0);
                        window.paint_quad(fill(
                            Bounds::new(point(left, y - px(2.0)), size(width, px(4.0))),
                            rgb(0x55555d),
                        ));
                        window.paint_quad(fill(
                            Bounds::new(point(left, y - px(2.0)), size(x - left, px(4.0))),
                            rgb(ACCENT),
                        ));
                        window.paint_quad(fill(
                            Bounds::new(point(x - px(4.0), y - px(6.0)), size(px(8.0), px(12.0))),
                            rgb(0xffffff),
                        ));
                    },
                )
                .size_full(),
            )
            .child(
                div()
                    .absolute()
                    .right_1()
                    .top_1()
                    .text_sm()
                    .text_color(rgb(0xffffff))
                    .child(format!("{}", value.round() as u32)),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |state, event: &MouseDownEvent, _, cx| {
                    state.intensity_drag = true;
                    if let Some(area) = down_bounds.get() {
                        let x: f32 = (event.position.x - area.origin.x).into();
                        state.editor.set_intensity(1.0 + (x - 8.0) / 96.0 * 47.0);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |state, event: &MouseMoveEvent, _, cx| {
                if state.intensity_drag {
                    if let Some(area) = move_bounds.get() {
                        let x: f32 = (event.position.x - area.origin.x).into();
                        state.editor.set_intensity(1.0 + (x - 8.0) / 96.0 * 47.0);
                    }
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|state, _, _, cx| {
                    state.intensity_drag = false;
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|state, _, _, _| {
                    state.intensity_drag = false;
                }),
            )
    }

    fn get_cursor(&self) -> CursorStyle {
        let tool = if self
            .editor
            .draft
            .as_ref()
            .is_some_and(|draft| draft.tool == Tool::Text)
        {
            Tool::Text
        } else {
            self.editor.tool
        };
        return get_selection_cursor(self.drag_mode, tool, self.pointer, self.selection);
    }

    fn get_hovered_window(&self) -> Option<Rect> {
        return get_hovered_capture_window(
            self.selection,
            self.drag_mode,
            self.screenshot
                .as_ref()
                .map_or(&[], |screenshot| screenshot.windows.as_slice()),
            self.pointer,
        );
    }

    fn get_should_show_magnifier(&self) -> bool {
        return self.screenshot.is_some()
            && (self.selection.is_none() || self.drag_mode == Some(DragMode::New))
            && self.get_cursor() == CursorStyle::Crosshair;
    }
}

impl EntityInputHandler for SelectionState {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = &self.editor.draft.as_ref()?.text;
        let start = get_byte_offset(text, range.start);
        let end = get_byte_offset(text, range.end).max(start);
        *adjusted = Some(text[..start].encode_utf16().count()..text[..end].encode_utf16().count());
        return Some(text[start..end].into());
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        return self
            .editor
            .draft
            .as_ref()
            .filter(|draft| draft.tool == Tool::Text)
            .map(|_| UTF16Selection {
                range: self.text_range.clone(),
                reversed: self.text_reversed,
            });
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        return self.marked_range.clone();
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let insertion_format = self.get_current_text_format();
        let Some(draft) = self
            .editor
            .draft
            .as_mut()
            .filter(|draft| draft.tool == Tool::Text)
        else {
            return;
        };
        let range = range
            .or(self.marked_range.take())
            .unwrap_or(self.text_range.clone());
        let start = get_byte_offset(&draft.text, range.start);
        let end = get_byte_offset(&draft.text, range.end).max(start);
        let cursor = draft.text[..start].encode_utf16().count() + text.encode_utf16().count();
        draft.replace_text(start..end, text, insertion_format);
        self.insertion_format = Some(insertion_format);
        self.text_range = cursor..cursor;
        self.text_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range, text, window, cx);
        let length = text.encode_utf16().count();
        let end = self.text_range.end;
        let start = end.saturating_sub(length);
        self.marked_range = (!text.is_empty()).then_some(start..end);
        if let Some(selected) = selected {
            self.text_range = start + selected.start.min(length)..start + selected.end.min(length);
        }
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _: Bounds<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        return self.get_text_bounds(range.start, window);
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        window: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let draft = self
            .editor
            .draft
            .as_ref()
            .filter(|draft| draft.tool == Tool::Text)?;
        let y: f32 = point.y.into();
        let row = (((y - draft.points[0].y) / (draft.width * 1.3))
            .floor()
            .max(0.0) as usize)
            .min(draft.text.split('\n').count() - 1);
        let mut offset = 0;
        let mut byte_offset = 0;
        for (index, line) in draft.text.split('\n').enumerate() {
            if index == row {
                let shaped = window.text_system().shape_line(
                    line.to_string().into(),
                    px(draft.width),
                    &create_text_runs(draft, byte_offset..byte_offset + line.len()),
                    None,
                );
                let byte = shaped.closest_index_for_x(point.x - px(draft.points[0].x));
                return Some(offset + line[..byte].encode_utf16().count());
            }
            offset += line.encode_utf16().count() + 1;
            byte_offset += line.len() + 1;
        }
        return Some(offset);
    }
}

impl Render for SelectionState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let annotations: Vec<_> = self.editor.get_visible_annotations().cloned().collect();
        let split = annotations
            .iter()
            .rposition(|annotation| annotation.tool == Tool::Mosaic)
            .map_or(0, |index| index + 1);
        let raster_scene = &annotations[..split];
        if split == 0 || self.selection.is_none() {
            if let Some(previous) = self.preview.take() {
                cx.drop_image(previous, Some(window));
            }
            self.preview_scene.clear();
            self.preview_selection = None;
        // ponytail: mosaic prefixes stay rasterized; one background job coalesces drag updates.
        } else if self.preview_task.is_none()
            && (self.preview_scene != raster_scene || self.preview_selection != self.selection)
        {
            if let (Some(selection), Some(screenshot)) = (
                self.selection
                    .filter(|selection| selection.size.min_element() >= 2.0),
                self.screenshot.as_ref(),
            ) {
                let source = screenshot.image.clone();
                let scene = raster_scene.to_vec();
                let bounds = self.bounds;
                let fonts = cx.global::<AppState>().fonts.clone();
                self.preview_task = Some(cx.spawn(async move |this, cx| {
                    let (scene, result) = cx
                        .background_executor()
                        .spawn(async move {
                            let result = raster::render_selection(
                                &source,
                                bounds,
                                selection,
                                scene.iter(),
                                &fonts,
                            )
                            .map(|image| raster::create_render_image(&image));
                            (scene, result)
                        })
                        .await;
                    let _ = this.update(cx, |state, cx| {
                        state.preview_task = None;
                        if state.selection == Some(selection) {
                            state.preview_scene = scene;
                            state.preview_selection = Some(selection);
                            match result {
                                Ok(image) => {
                                    if let Some(previous) = state.preview.replace(image) {
                                        cx.drop_image(previous, None);
                                    }
                                }
                                Err(error) => {
                                    if let Some(previous) = state.preview.take() {
                                        cx.drop_image(previous, None);
                                    }
                                    state.error = Some(error);
                                }
                            }
                        }
                        cx.notify();
                    });
                }));
            }
        }
        let gpu_annotations: Vec<_> = annotations.into_iter().skip(split).collect();
        let selection = self.selection;
        let ready = self.background.is_some();
        let screen_size = self.bounds.size;
        let hovered = self.get_hovered_window();
        let border = selection.or(hovered).or_else(|| {
            (self.drag_mode == Some(DragMode::New))
                .then_some(self.drag_origin)
                .flatten()
        });
        let input = self
            .editor
            .draft
            .as_ref()
            .is_some_and(|draft| draft.tool == Tool::Text);
        let input_focus = self.focus.clone();
        let input_entity = cx.entity();
        let caret = self.get_text_bounds(
            if self.text_reversed {
                self.text_range.start
            } else {
                self.text_range.end
            },
            window,
        );
        let mut text_highlights = Vec::new();
        if input && !self.text_range.is_empty() {
            let text = &self.editor.draft.as_ref().unwrap().text;
            let mut offset = 0;
            for line in text.split('\n') {
                let end = offset + line.encode_utf16().count();
                if self.text_range.start <= end && self.text_range.end > offset {
                    if let (Some(start), Some(finish)) = (
                        self.get_text_bounds(self.text_range.start.max(offset), window),
                        self.get_text_bounds(self.text_range.end.min(end), window),
                    ) {
                        text_highlights.push(Bounds::new(
                            start.origin,
                            size(
                                (finish.origin.x - start.origin.x).max(px(1.0)),
                                start.size.height,
                            ),
                        ));
                    }
                }
                offset = end + 1;
            }
        }
        let cursor = self.get_cursor();
        let mut root = div()
            .id("selection")
            .size_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .font_family("Arial")
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::begin_drag))
            .on_mouse_move(cx.listener(Self::update_drag))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::finish_drag))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::finish_drag))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|state, _, window, cx| {
                    state.perform_action(Action::Cancel, window, cx)
                }),
            )
            .on_key_down(cx.listener(Self::handle_key))
            .on_modifiers_changed(cx.listener(|state, _, _, cx| {
                if state.get_should_show_magnifier() {
                    cx.notify();
                }
            }))
            .when_some(self.background.clone(), |el, background| {
                el.child(img(background).absolute().size_full())
            })
            .when_some(
                selection.zip(
                    self.preview
                        .clone()
                        .filter(|_| self.preview_selection == selection),
                ),
                |el, (selection, preview)| {
                    el.child(
                        img(preview)
                            .absolute()
                            .left(px(selection.pos.x))
                            .top(px(selection.pos.y))
                            .w(px(selection.size.x))
                            .h(px(selection.size.y)),
                    )
                },
            )
            .child(
                canvas(
                    move |_, _, _| (),
                    move |bounds, _, window, cx| {
                        if input {
                            window.handle_input(
                                &input_focus,
                                ElementInputHandler::new(bounds, input_entity.clone()),
                                cx,
                            );
                        }
                        if let Some(selection) = selection {
                            window.with_content_mask(
                                Some(ContentMask {
                                    bounds: get_bounds(selection),
                                }),
                                |window| {
                                    for annotation in &gpu_annotations {
                                        if let Err(error) = paint_annotation(annotation, window, cx)
                                        {
                                            let entity = input_entity.clone();
                                            cx.defer(move |cx| {
                                                entity.update(cx, |state, cx| {
                                                    if state.error.as_ref() != Some(&error) {
                                                        state.error = Some(error);
                                                        cx.notify();
                                                    }
                                                })
                                            });
                                        }
                                    }
                                },
                            );
                        }
                        if ready {
                            for rect in get_dim_rects(border, screen_size) {
                                window.paint_quad(fill(get_bounds(rect), rgba(0x00000060)));
                            }
                        }
                        for bounds in &text_highlights {
                            window.paint_quad(fill(*bounds, rgba(0x327fff66)));
                        }
                        if let Some(rect) = border {
                            window.paint_quad(outline(
                                get_bounds(rect),
                                rgb(ACCENT),
                                BorderStyle::Solid,
                            ));
                            for (_, center) in get_resize_handle_centers(rect) {
                                let mut quad = fill(
                                    get_bounds(Rect {
                                        pos: center - Vec2::splat(HANDLE_RADIUS),
                                        size: Vec2::splat(HANDLE_RADIUS * 2.0),
                                    }),
                                    rgb(ACCENT),
                                );
                                quad.corner_radii = Corners::all(px(HANDLE_RADIUS));
                                quad.border_widths = Edges::all(px(1.5));
                                quad.border_color = rgb(0xffffff).into();
                                window.paint_quad(quad);
                            }
                        }
                    },
                )
                .absolute()
                .size_full(),
            );
        if let Some(rect) = border {
            let scale = self
                .screenshot
                .as_ref()
                .map_or(window.scale_factor(), |screenshot| {
                    screenshot.image.width() as f32 / screen_size.x
                });
            root = root.child(
                div()
                    .absolute()
                    .left(px(rect.pos.x.clamp(8.0, (screen_size.x - 150.0).max(8.0))))
                    .top(px((rect.pos.y - 30.0).max(8.0)))
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded_sm()
                    .bg(rgb(0x242428))
                    .border_1()
                    .border_color(rgb(0x55555d))
                    .text_color(rgb(0xffffff))
                    .text_sm()
                    .child(format!(
                        "{} × {}",
                        (rect.size.x * scale).round() as u32,
                        (rect.size.y * scale).round() as u32
                    )),
            );
        }
        if let Some(selection) = selection {
            if let Some(index) = self.editor.selected {
                let rect = self.editor.layers[index].get_bounds();
                root = root.child(
                    div()
                        .absolute()
                        .left(px(rect.pos.x))
                        .top(px(rect.pos.y))
                        .w(px(rect.size.x))
                        .h(px(rect.size.y))
                        .border_1()
                        .border_dashed()
                        .border_color(rgb(0xffffff)),
                );
            }
            if let Some(caret) = caret {
                root = root.child(
                    div()
                        .absolute()
                        .left(caret.origin.x)
                        .top(caret.origin.y)
                        .h(caret.size.height)
                        .border_l_2()
                        .border_color(rgb(self.editor.color.0)),
                );
            }

            let bar = create_action_bar_rect(selection, screen_size);
            let layers_panel = self.show_layers.then(|| {
                create_layers_panel_rect(selection, bar, screen_size, self.editor.layers.len())
            });
            let tools = [
                ("select", "Select / move (V)", Tool::Select),
                ("rectangle", "Rectangle (R)", Tool::Rectangle),
                ("arrow", "Arrow (A)", Tool::Arrow),
                ("pen", "Pen (B)", Tool::Pen),
                ("highlight", "Highlighter (H)", Tool::Highlighter),
                ("text", "Text (T)", Tool::Text),
                ("mosaic", "Mosaic (M)", Tool::Mosaic),
                ("eraser", "Eraser (E)", Tool::Eraser),
            ];
            root = root.child(
                div()
                    .id("action-bar")
                    .cursor(CursorStyle::Arrow)
                    .absolute()
                    .left(px(bar.pos.x))
                    .top(px(bar.pos.y))
                    .w(px(bar.size.x))
                    .h(px(BAR_HEIGHT))
                    .flex()
                    .items_center()
                    .gap(px(1.0))
                    .px(px(4.0))
                    .rounded_md()
                    .bg(rgb(0x28282d))
                    .border_1()
                    .border_color(rgb(0x44444c))
                    .overflow_x_scroll()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(tools.into_iter().map(|(icon, label, tool)| {
                        self.build_button(
                            icon,
                            label,
                            Action::Tool(tool),
                            get_tool_family(self.editor.tool) == get_tool_family(tool),
                            true,
                            cx,
                        )
                    }))
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(18.0))
                            .mx(px(3.0))
                            .bg(rgb(0x55555d))
                            .flex_shrink_0(),
                    )
                    .child(self.build_button(
                        "undo",
                        "Undo (⌘Z)",
                        Action::Undo,
                        false,
                        !self.editor.undo.is_empty() || self.editor.draft.is_some(),
                        cx,
                    ))
                    .child(self.build_button(
                        "redo",
                        "Redo (⌘⇧Z)",
                        Action::Redo,
                        false,
                        !self.editor.redo.is_empty(),
                        cx,
                    ))
                    .child(self.build_button(
                        "search",
                        "Google Lens",
                        Action::Search,
                        false,
                        true,
                        cx,
                    ))
                    .child(self.build_button(
                        "cancel",
                        "Cancel (Esc)",
                        Action::Cancel,
                        false,
                        true,
                        cx,
                    ))
                    .child(self.build_button("pin", "Pin (⌘P)", Action::Pin, false, true, cx))
                    .child(self.build_button("save", "Save (⌘S)", Action::Save, false, true, cx))
                    .child(self.build_button(
                        "layers",
                        "Layers",
                        Action::Layers,
                        self.show_layers,
                        true,
                        cx,
                    ))
                    .child(self.build_button(
                        "copy",
                        "Copy (Enter / ⌘C)",
                        Action::Copy,
                        false,
                        true,
                        cx,
                    )),
            );
            let tool = self.editor.get_current_tool();
            if tool != Tool::Select && tool != Tool::Eraser {
                let y = if bar.pos.y + BAR_HEIGHT + PALETTE_HEIGHT + 4.0 <= screen_size.y - 8.0 {
                    bar.pos.y + BAR_HEIGHT + 4.0
                } else {
                    (bar.pos.y - PALETTE_HEIGHT - 4.0).max(8.0)
                };
                let current = self.editor.get_current_annotation();
                let filled = current.map_or(self.editor.filled, |annotation| annotation.filled);
                let mode = current.map_or(self.editor.blur_mode, |annotation| annotation.blur_mode);
                let color = current.map_or(self.editor.color, |annotation| annotation.color);
                let width = self.editor.get_current_width();
                let format = self.get_current_text_format();
                let mut options_width = (screen_size.x - bar.pos.x - 8.0).max(32.0);
                if let Some(panel) = layers_panel {
                    if panel.pos.x > bar.pos.x
                        && y < panel.pos.y + panel.size.y
                        && y + PALETTE_HEIGHT > panel.pos.y
                    {
                        options_width =
                            options_width.min((panel.pos.x - bar.pos.x - 8.0).max(32.0));
                    }
                }
                let mut options = div()
                    .id("tool-options")
                    .absolute()
                    .left(px(bar.pos.x))
                    .top(px(y))
                    .flex()
                    .items_center()
                    .gap_1()
                    .p_1()
                    .rounded_md()
                    .bg(rgb(0x28282d))
                    .border_1()
                    .border_color(rgb(0x44444c))
                    .cursor(CursorStyle::Arrow)
                    .max_w(px(options_width))
                    .overflow_x_scroll()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
                match tool {
                    Tool::Rectangle | Tool::Ellipse => {
                        options = options
                            .child(self.build_button(
                                "outline",
                                "Outline",
                                Action::Filled(false),
                                !filled,
                                true,
                                cx,
                            ))
                            .child(self.build_button(
                                "filled",
                                "Filled",
                                Action::Filled(true),
                                filled,
                                true,
                                cx,
                            ))
                            .child(self.build_button(
                                "rectangle",
                                "Rectangle",
                                Action::Variant(Tool::Rectangle),
                                tool == Tool::Rectangle,
                                true,
                                cx,
                            ))
                            .child(self.build_button(
                                "ellipse",
                                "Oval",
                                Action::Variant(Tool::Ellipse),
                                tool == Tool::Ellipse,
                                true,
                                cx,
                            ));
                    }
                    Tool::Arrow | Tool::Line => {
                        options = options
                            .child(self.build_button(
                                "arrow",
                                "Arrow",
                                Action::Variant(Tool::Arrow),
                                tool == Tool::Arrow,
                                true,
                                cx,
                            ))
                            .child(self.build_button(
                                "line",
                                "Straight line",
                                Action::Variant(Tool::Line),
                                tool == Tool::Line,
                                true,
                                cx,
                            ));
                    }
                    Tool::Mosaic => {
                        options = options
                            .child(self.build_button(
                                "mosaic",
                                "Pixelated",
                                Action::BlurMode(BlurMode::Pixelated),
                                mode == BlurMode::Pixelated,
                                true,
                                cx,
                            ))
                            .child(self.build_button(
                                "blur",
                                "Gaussian blur",
                                Action::BlurMode(BlurMode::Gaussian),
                                mode == BlurMode::Gaussian,
                                true,
                                cx,
                            ))
                            .child(self.build_intensity_slider(cx));
                    }
                    Tool::Text => {
                        options = options
                            .child(self.build_button(
                                "bold",
                                "Bold (⌘B)",
                                Action::TextStyle(TextAttribute::Bold),
                                format.bold,
                                true,
                                cx,
                            ))
                            .child(self.build_button(
                                "italic",
                                "Italic (⌘I)",
                                Action::TextStyle(TextAttribute::Italic),
                                format.italic,
                                true,
                                cx,
                            ))
                            .child(self.build_button(
                                "underline",
                                "Underline (⌘U)",
                                Action::TextStyle(TextAttribute::Underline),
                                format.underline,
                                true,
                                cx,
                            ));
                    }
                    _ => {}
                }
                if tool != Tool::Mosaic {
                    options = options
                        .child(
                            div()
                                .id("size")
                                .h(px(28.0))
                                .min_w(px(44.0))
                                .px_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(rgb(0xeeeeee))
                                .rounded_sm()
                                .bg(rgb(0x38383f))
                                .child(format!(
                                    "{}{}",
                                    width.round() as u32,
                                    if tool == Tool::Text { "" } else { "px" }
                                ))
                                .tooltip(move |_, cx| {
                                    cx.new(|_| TooltipLabel("Scroll to adjust size".into()))
                                        .into()
                                })
                                .on_scroll_wheel(cx.listener(
                                    move |state, event: &ScrollWheelEvent, _, cx| {
                                        let delta: f32 = event.delta.pixel_delta(px(16.0)).y.into();
                                        if delta == 0.0 {
                                            return;
                                        }
                                        let width = state.editor.get_current_width();
                                        if tool == Tool::Text {
                                            state
                                                .editor
                                                .set_text_size(width + delta.signum() * 2.0);
                                        } else {
                                            state.editor.set_width(width + delta.signum());
                                        }
                                        cx.stop_propagation();
                                        cx.notify();
                                    },
                                )),
                        )
                        .child(self.build_button(
                            "palette",
                            "Color picker",
                            Action::ColorPicker,
                            false,
                            true,
                            cx,
                        ))
                        .children(COLORS.into_iter().map(|swatch| {
                            div()
                                .id(("color", swatch as usize))
                                .size(px(18.0))
                                .flex_shrink_0()
                                .rounded_full()
                                .bg(rgb(swatch))
                                .border_2()
                                .border_color(if color.0 == swatch {
                                    rgb(0xffffff)
                                } else {
                                    rgb(0x55555d)
                                })
                                .cursor_pointer()
                                .on_click(cx.listener(move |state, _, _, cx| {
                                    state.editor.set_color(Color(swatch));
                                    cx.notify();
                                }))
                        }));
                }
                if self.editor.selected.is_some() {
                    options = options.child(self.build_button(
                        "delete",
                        "Delete layer",
                        Action::Delete,
                        false,
                        true,
                        cx,
                    ));
                }
                root = root.child(options);
            }
            if let Some(panel) = layers_panel {
                let layers = self
                    .editor
                    .layers
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(index, layer)| {
                        let id = layer.id;
                        let label: SharedString =
                            format!("{} {}", get_layer_label(layer.tool), id).into();
                        let drag_label = label.clone();
                        let drop_target = self.layer_drop_target;
                        let drag_entity = cx.entity();
                        div()
                            .id(("layer", id as usize))
                            .relative()
                            .flex()
                            .items_center()
                            .h(px(28.0))
                            .flex_shrink_0()
                            .px_2()
                            .rounded_sm()
                            .cursor_move()
                            .bg(if self.editor.selected == Some(index) {
                                rgb(0x327fff)
                            } else {
                                rgba(0)
                            })
                            .hover(|style| style.bg(rgb(0x414148)))
                            .text_sm()
                            .text_color(rgb(0xffffff))
                            .child(label)
                            .on_drag(id, move |dragged: &u64, _, window, cx| {
                                drag_entity.update(cx, |state, cx| {
                                    state.layer_drop_target = None;
                                    state.editor.commit_draft();
                                    state.editor.selected = state
                                        .editor
                                        .layers
                                        .iter()
                                        .position(|layer| layer.id == *dragged);
                                    state.editor.tool = Tool::Select;
                                    window.focus(&state.focus);
                                    cx.notify();
                                });
                                cx.new(|_| TooltipLabel(drag_label.clone()))
                            })
                            .on_drag_move(cx.listener(
                                move |state, event: &DragMoveEvent<u64>, _, cx| {
                                    if event.bounds.contains(&event.event.position) {
                                        let above = event.event.position.y
                                            < event.bounds.origin.y
                                                + event.bounds.size.height / 2.0;
                                        let target = Some((id, above));
                                        if state.layer_drop_target != target {
                                            state.layer_drop_target = target;
                                            cx.notify();
                                        }
                                    }
                                },
                            ))
                            .drag_over::<u64>(move |style, dragged, _, _| {
                                if *dragged == id {
                                    return style;
                                }
                                match drop_target {
                                    Some((target, true)) if target == id => {
                                        style.border_t_2().border_color(rgb(ACCENT))
                                    }
                                    Some((target, false)) if target == id => {
                                        style.border_b_2().border_color(rgb(ACCENT))
                                    }
                                    _ => style,
                                }
                            })
                            .on_drop(cx.listener(move |state, dragged: &u64, window, cx| {
                                let above = state
                                    .layer_drop_target
                                    .filter(|(target, _)| *target == id)
                                    .is_none_or(|(_, above)| above);
                                state.layer_drop_target = None;
                                state.editor.move_layer(*dragged, id, above);
                                state.editor.selected = state
                                    .editor
                                    .layers
                                    .iter()
                                    .position(|layer| layer.id == *dragged);
                                state.editor.tool = Tool::Select;
                                window.focus(&state.focus);
                                cx.notify();
                            }))
                            .on_click(cx.listener(move |state, _, window, cx| {
                                state.editor.commit_draft();
                                state.editor.selected =
                                    state.editor.layers.iter().position(|layer| layer.id == id);
                                state.editor.tool = Tool::Select;
                                window.focus(&state.focus);
                                cx.notify();
                            }))
                    });
                root = root.child(
                    div()
                        .id("layers-panel")
                        .flex()
                        .flex_col()
                        .absolute()
                        .left(px(panel.pos.x))
                        .top(px(panel.pos.y))
                        .w(px(panel.size.x))
                        .h(px(panel.size.y))
                        .rounded_md()
                        .bg(rgb(0x28282d))
                        .border_1()
                        .border_color(rgb(0x44444c))
                        .cursor(CursorStyle::Arrow)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_drag_move(cx.listener(|state, event: &DragMoveEvent<u64>, _, cx| {
                            if !event.bounds.contains(&event.event.position)
                                && state.layer_drop_target.take().is_some()
                            {
                                cx.notify();
                            }
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_1()
                                .h(px(30.0))
                                .flex_shrink_0()
                                .text_sm()
                                .text_color(rgb(0xffffff))
                                .child("Layers")
                                .child(self.build_button(
                                    "cancel",
                                    "Close layers",
                                    Action::Layers,
                                    false,
                                    true,
                                    cx,
                                )),
                        )
                        .child(
                            div()
                                .id("layer-list")
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .flex_1()
                                .min_h(px(0.0))
                                .overflow_y_scroll()
                                .p_1()
                                .children(layers),
                        ),
                );
            }
        }

        if self.get_should_show_magnifier() {
            if let Some(screenshot) = self.screenshot.as_ref() {
                if let Some((x, y, color)) =
                    get_sampled_pixel(&screenshot.image, screen_size, self.pointer)
                {
                    let rect = create_magnifier_rect(self.pointer, screen_size);
                    let source = screenshot.image.clone();
                    let color_label = create_color_label(color, window.modifiers().shift);
                    root = root.child(
                        div()
                            .absolute()
                            .left(px(rect.pos.x))
                            .top(px(rect.pos.y))
                            .w(px(rect.size.x))
                            .h(px(rect.size.y))
                            .overflow_hidden()
                            .bg(rgb(0x17171b))
                            .border_1()
                            .border_color(rgb(0x55555d))
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, window, _| {
                                        for row in 0..MAGNIFIER_ROWS {
                                            for column in 0..MAGNIFIER_COLUMNS {
                                                let sx = x as i64
                                                    + (column - MAGNIFIER_COLUMNS / 2) as i64;
                                                let sy =
                                                    y as i64 + (row - MAGNIFIER_ROWS / 2) as i64;
                                                if sx < 0
                                                    || sy < 0
                                                    || sx >= source.width() as i64
                                                    || sy >= source.height() as i64
                                                {
                                                    continue;
                                                }
                                                let pixel = source.get_pixel(sx as u32, sy as u32);
                                                window.paint_quad(fill(
                                                    Bounds::new(
                                                        bounds.origin
                                                            + point(
                                                                px(column as f32 * MAGNIFIER_CELL),
                                                                px(row as f32 * MAGNIFIER_CELL),
                                                            ),
                                                        size(
                                                            px(MAGNIFIER_CELL),
                                                            px(MAGNIFIER_CELL),
                                                        ),
                                                    ),
                                                    rgb(crate::geometry::Color::from_rgb(
                                                        pixel[0], pixel[1], pixel[2],
                                                    )
                                                    .0),
                                                ));
                                            }
                                        }
                                        let center = bounds.origin
                                            + point(
                                                px((MAGNIFIER_COLUMNS / 2) as f32 * MAGNIFIER_CELL),
                                                px((MAGNIFIER_ROWS / 2) as f32 * MAGNIFIER_CELL),
                                            );
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                point(center.x, bounds.origin.y),
                                                size(px(MAGNIFIER_CELL), bounds.size.height),
                                            ),
                                            rgba(0x327fff44),
                                        ));
                                        window.paint_quad(fill(
                                            Bounds::new(
                                                point(bounds.origin.x, center.y),
                                                size(bounds.size.width, px(MAGNIFIER_CELL)),
                                            ),
                                            rgba(0x327fff44),
                                        ));
                                        let center_bounds = Bounds::new(
                                            center,
                                            size(px(MAGNIFIER_CELL), px(MAGNIFIER_CELL)),
                                        );
                                        window.paint_quad(fill(center_bounds, rgb(color.0)));
                                        window.paint_quad(outline(
                                            center_bounds,
                                            rgb(0x000000),
                                            BorderStyle::Solid,
                                        ));
                                        window.paint_quad(outline(
                                            Bounds::new(
                                                center + point(px(1.0), px(1.0)),
                                                size(
                                                    px(MAGNIFIER_CELL - 2.0),
                                                    px(MAGNIFIER_CELL - 2.0),
                                                ),
                                            ),
                                            rgb(0xffffff),
                                            BorderStyle::Solid,
                                        ));
                                    },
                                )
                                .w(px(MAGNIFIER_COLUMNS as f32 * MAGNIFIER_CELL))
                                .h(px(MAGNIFIER_ROWS as f32 * MAGNIFIER_CELL)),
                            )
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .items_center()
                                    .gap(px(2.0))
                                    .py(px(4.0))
                                    .text_color(rgb(0xffffff))
                                    .text_size(px(11.0))
                                    .font_family("Menlo")
                                    .child(format!("({x}, {y})"))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.0))
                                            .child(
                                                div()
                                                    .size(px(12.0))
                                                    .bg(rgb(color.0))
                                                    .border_1()
                                                    .border_color(rgb(0xcccccc)),
                                            )
                                            .child(color_label),
                                    ),
                            ),
                    );
                }
            }
        }
        root.when_some(self.error.clone(), |el, error| {
            el.child(
                div()
                    .absolute()
                    .bottom_4()
                    .left_4()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(0x28282d))
                    .text_color(rgb(0xff8686))
                    .child(error),
            )
        })
    }
}

struct TooltipLabel(SharedString);
impl Render for TooltipLabel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(6.0))
            .py(px(3.0))
            .rounded_sm()
            .bg(rgb(0x19191c))
            .text_color(rgb(0xffffff))
            .text_sm()
            .child(self.0.clone())
    }
}
fn get_hovered_capture_window(
    selection: Option<Rect>,
    drag: Option<DragMode>,
    windows: &[Rect],
    pointer: Vec2,
) -> Option<Rect> {
    if selection.is_some() || drag.is_some() {
        return None;
    }
    return windows
        .iter()
        .find(|rect| rect.intersects(pointer))
        .copied();
}

fn get_dim_rects(selection: Option<Rect>, size: Vec2) -> Vec<Rect> {
    let Some(rect) = selection else {
        return vec![Rect {
            pos: Vec2::ZERO,
            size,
        }];
    };
    return vec![
        Rect {
            pos: Vec2::ZERO,
            size: vec2(size.x, rect.pos.y),
        },
        Rect {
            pos: vec2(0.0, rect.pos.y + rect.size.y),
            size: vec2(size.x, (size.y - rect.pos.y - rect.size.y).max(0.0)),
        },
        Rect {
            pos: vec2(0.0, rect.pos.y),
            size: vec2(rect.pos.x, rect.size.y),
        },
        Rect {
            pos: vec2(rect.pos.x + rect.size.x, rect.pos.y),
            size: vec2((size.x - rect.pos.x - rect.size.x).max(0.0), rect.size.y),
        },
    ];
}

fn get_sampled_pixel(
    image: &RgbaImage,
    screen_size: Vec2,
    pointer: Vec2,
) -> Option<(u32, u32, crate::geometry::Color)> {
    if image.width() == 0 || image.height() == 0 || screen_size.min_element() <= 0.0 {
        return None;
    }
    let x = (pointer.x * image.width() as f32 / screen_size.x)
        .floor()
        .clamp(0.0, (image.width() - 1) as f32) as u32;
    let y = (pointer.y * image.height() as f32 / screen_size.y)
        .floor()
        .clamp(0.0, (image.height() - 1) as f32) as u32;
    let pixel = image.get_pixel(x, y);
    return Some((
        x,
        y,
        crate::geometry::Color::from_rgb(pixel[0], pixel[1], pixel[2]),
    ));
}

fn create_color_label(color: crate::geometry::Color, rgb_format: bool) -> String {
    return if rgb_format {
        format!(
            "{}, {}, {}",
            color.0 >> 16 & 255,
            color.0 >> 8 & 255,
            color.0 & 255
        )
    } else {
        format!("#{:06x}", color.0)
    };
}

fn create_magnifier_rect(pointer: Vec2, screen_size: Vec2) -> Rect {
    let size = vec2(
        MAGNIFIER_COLUMNS as f32 * MAGNIFIER_CELL + 2.0,
        MAGNIFIER_ROWS as f32 * MAGNIFIER_CELL + 50.0,
    )
    .min((screen_size - Vec2::splat(16.0)).max(Vec2::ZERO));
    let mut pos = pointer + Vec2::splat(24.0);
    if pos.x + size.x > screen_size.x - 8.0 {
        pos.x = pointer.x - size.x - 24.0;
    }
    if pos.y + size.y > screen_size.y - 8.0 {
        pos.y = pointer.y - size.y - 24.0;
    }
    return Rect {
        pos: pos.clamp(Vec2::ZERO, (screen_size - size).max(Vec2::ZERO)),
        size,
    };
}

fn create_clicked_text_range(text: &str, offset: usize, click_count: usize) -> Range<usize> {
    if click_count < 2 {
        return offset..offset;
    }
    let byte = get_byte_offset(text, offset);
    let range = text
        .split_word_bound_indices()
        .find(|(start, word)| *start <= byte && byte < start + word.len())
        .or_else(|| text.split_word_bound_indices().last());
    let Some((start, word)) = range else {
        return 0..0;
    };
    let start = text[..start].encode_utf16().count();
    return start..start + word.encode_utf16().count();
}

fn get_tool_family(tool: Tool) -> Tool {
    return match tool {
        Tool::Rectangle | Tool::Ellipse => Tool::Rectangle,
        Tool::Arrow | Tool::Line => Tool::Arrow,
        other => other,
    };
}

fn get_layer_label(tool: Tool) -> &'static str {
    return match tool {
        Tool::Rectangle => "Rectangle",
        Tool::Ellipse => "Oval",
        Tool::Arrow => "Arrow",
        Tool::Line => "Line",
        Tool::Pen => "Pen",
        Tool::Highlighter => "Highlighter",
        Tool::Text => "Text",
        Tool::Mosaic => "Blur",
        Tool::Select => "Selection",
        Tool::Eraser => "Eraser",
    };
}

fn create_layers_panel_rect(selection: Rect, bar: Rect, screen: Vec2, layers: usize) -> Rect {
    let size = vec2(188.0, (40.0 + layers.max(1) as f32 * 30.0).min(320.0))
        .min((screen - Vec2::splat(16.0)).max(Vec2::ZERO));
    let right = (selection.pos.x + selection.size.x).max(bar.pos.x + bar.size.x) + 8.0;
    let left = selection.pos.x.min(bar.pos.x) - size.x - 8.0;
    let below = selection.pos.y + selection.size.y + 8.0;
    let above = selection.pos.y.min(bar.pos.y - PALETTE_HEIGHT - 4.0) - size.y - 8.0;
    let pos = if right + size.x <= screen.x - 8.0 {
        vec2(right, selection.pos.y)
    } else if left >= 8.0 {
        vec2(left, selection.pos.y)
    } else if below + size.y <= screen.y - 8.0 && bar.pos.x >= size.x + 16.0 {
        vec2(bar.pos.x - size.x - 8.0, below)
    } else if above >= 8.0 {
        vec2(selection.pos.x, above)
    } else if bar.pos.x >= size.x + 16.0 {
        vec2(bar.pos.x - size.x - 8.0, bar.pos.y)
    } else if bar.pos.x + bar.size.x + size.x + 16.0 <= screen.x {
        vec2(bar.pos.x + bar.size.x + 8.0, bar.pos.y)
    } else {
        vec2(selection.pos.x, bar.pos.y - size.y - 8.0)
    };
    return Rect {
        pos: pos.clamp(
            Vec2::splat(8.0),
            (screen - size - Vec2::splat(8.0)).max(Vec2::splat(8.0)),
        ),
        size,
    };
}

fn create_text_runs(annotation: &Annotation, range: Range<usize>) -> Vec<TextRun> {
    return annotation
        .get_format_runs(range)
        .into_iter()
        .map(|(length, format)| {
            let mut face = font("Arial");
            if format.bold {
                face = face.bold();
            }
            if format.italic {
                face = face.italic();
            }
            TextRun {
                len: length,
                font: face,
                color: rgb(annotation.color.0).into(),
                background_color: None,
                underline: format.underline.then_some(UnderlineStyle {
                    color: Some(rgb(annotation.color.0).into()),
                    thickness: px((annotation.width / 16.0).max(1.0)),
                    wavy: false,
                }),
                strikethrough: None,
            }
        })
        .collect();
}

fn get_selection_cursor(
    drag: Option<DragMode>,
    tool: Tool,
    pointer: Vec2,
    selection: Option<Rect>,
) -> CursorStyle {
    match drag {
        Some(DragMode::New | DragMode::Draw | DragMode::Erase) => return CursorStyle::Crosshair,
        Some(DragMode::Move | DragMode::MoveAnnotation(_)) => return CursorStyle::ClosedHand,
        Some(DragMode::SelectText(_, _)) => return CursorStyle::IBeam,
        _ => {}
    }
    let handle = match drag {
        Some(DragMode::Resize(handle)) => Some(handle),
        _ => selection.and_then(|selection| get_resize_handle(pointer, selection)),
    };
    if let Some(handle) = handle {
        return match handle {
            ResizeHandle::Top | ResizeHandle::Bottom => CursorStyle::ResizeUpDown,
            ResizeHandle::Left | ResizeHandle::Right => CursorStyle::ResizeLeftRight,
            ResizeHandle::TopLeft | ResizeHandle::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            _ => CursorStyle::ResizeUpRightDownLeft,
        };
    }
    if selection.is_some_and(|selection| selection.intersects(pointer)) {
        return match tool {
            Tool::Select => CursorStyle::OpenHand,
            Tool::Text => CursorStyle::IBeam,
            _ => CursorStyle::Crosshair,
        };
    }
    return CursorStyle::Crosshair;
}

fn build_annotation_path(annotation: &Annotation) -> Result<Option<Path<Pixels>>, String> {
    let start = annotation.points[0];
    let end = *annotation.points.last().unwrap();
    let min = start.min(end);
    let max = start.max(end);
    let at = |point: Vec2| gpui::point(px(point.x), px(point.y));
    let mut path = PathBuilder::stroke(px(annotation.width)).with_style(PathStyle::Stroke(
        StrokeOptions::default()
            .with_line_width(annotation.width)
            .with_line_cap(lyon::path::LineCap::Round)
            .with_line_join(lyon::path::LineJoin::Round),
    ));
    if annotation.filled && matches!(annotation.tool, Tool::Rectangle | Tool::Ellipse) {
        path = path.with_style(PathStyle::Fill(FillOptions::default()));
    }
    match annotation.tool {
        Tool::Rectangle => {
            if (max - min).min_element() <= 0.0 {
                return Ok(None);
            }
            path.move_to(at(min));
            path.line_to(at(vec2(max.x, min.y)));
            path.line_to(at(max));
            path.line_to(at(vec2(min.x, max.y)));
            path.close();
        }
        Tool::Ellipse => {
            let radius = (max - min) / 2.0;
            if radius.min_element() <= 0.0 {
                return Ok(None);
            }
            let center = (min + max) / 2.0;
            path.move_to(at(center + vec2(radius.x, 0.0)));
            path.arc_to(
                at(radius),
                px(0.0),
                false,
                true,
                at(center - vec2(radius.x, 0.0)),
            );
            path.arc_to(
                at(radius),
                px(0.0),
                false,
                true,
                at(center + vec2(radius.x, 0.0)),
            );
            path.close();
        }
        Tool::Arrow | Tool::Line => {
            if start == end {
                return Ok(None);
            }
            let [left, right] = crate::editor::get_arrow_head(start, end, annotation.width);
            path.move_to(at(start));
            path.line_to(at(end));
            if annotation.tool == Tool::Arrow {
                path.move_to(at(left));
                path.line_to(at(end));
                path.line_to(at(right));
            }
        }
        Tool::Pen | Tool::Highlighter => {
            if annotation.points.len() < 2 {
                return Ok(None);
            }
            path.move_to(at(start));
            for point in &annotation.points[1..] {
                path.line_to(at(*point));
            }
        }
        _ => return Ok(None),
    }
    return path.build().map(Some).map_err(|error| error.to_string());
}

fn paint_annotation(
    annotation: &Annotation,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    let mut color = rgb(annotation.color.0);
    if annotation.tool == Tool::Highlighter {
        color.a = 0.35;
    }
    if let Some(path) = build_annotation_path(annotation)? {
        window.paint_path(path, color);
    }
    if matches!(annotation.tool, Tool::Pen | Tool::Highlighter) && annotation.points.len() == 1 {
        let radius = annotation.width / 2.0;
        let mut dot = fill(
            get_bounds(Rect {
                pos: annotation.points[0] - Vec2::splat(radius),
                size: Vec2::splat(annotation.width),
            }),
            color,
        );
        dot.corner_radii = Corners::all(px(radius));
        window.paint_quad(dot);
    }
    if annotation.tool == Tool::Text {
        let mut offset = 0;
        for (index, text) in annotation.text.split('\n').enumerate() {
            let line = window.text_system().shape_line(
                text.to_string().into(),
                px(annotation.width),
                &create_text_runs(annotation, offset..offset + text.len()),
                None,
            );
            let height = px(annotation.width * 1.3);
            let baseline =
                px(annotation.points[0].y + annotation.width * (1.0 + index as f32 * 1.3));
            let y = baseline - line.ascent - (height - line.ascent - line.descent) / 2.0;
            line.paint(point(px(annotation.points[0].x), y), height, window, cx)
                .map_err(|error| error.to_string())?;
            offset += text.len() + 1;
        }
    }
    return Ok(());
}

fn get_bounds(rect: Rect) -> Bounds<Pixels> {
    return Bounds::new(
        point(px(rect.pos.x), px(rect.pos.y)),
        size(px(rect.size.x), px(rect.size.y)),
    );
}
fn create_selection_rect(start: Vec2, current: Vec2) -> Rect {
    return Rect {
        pos: start.min(current),
        size: (start - current).abs(),
    };
}
fn create_resized_selection(origin: Rect, handle: ResizeHandle, point: Vec2) -> Rect {
    let left = origin.pos.x;
    let right = origin.pos.x + origin.size.x;
    let top = origin.pos.y;
    let bottom = origin.pos.y + origin.size.y;
    return match handle {
        ResizeHandle::TopLeft => create_selection_rect(vec2(right, bottom), point),
        ResizeHandle::Top => Rect {
            pos: vec2(left, bottom.min(point.y)),
            size: vec2(origin.size.x, (point.y - bottom).abs()),
        },
        ResizeHandle::TopRight => create_selection_rect(vec2(left, bottom), point),
        ResizeHandle::Right => Rect {
            pos: vec2(left.min(point.x), top),
            size: vec2((point.x - left).abs(), origin.size.y),
        },
        ResizeHandle::BottomRight => create_selection_rect(vec2(left, top), point),
        ResizeHandle::Bottom => Rect {
            pos: vec2(left, top.min(point.y)),
            size: vec2(origin.size.x, (point.y - top).abs()),
        },
        ResizeHandle::BottomLeft => create_selection_rect(vec2(right, top), point),
        ResizeHandle::Left => Rect {
            pos: vec2(right.min(point.x), top),
            size: vec2((point.x - right).abs(), origin.size.y),
        },
    };
}
fn create_moved_selection(origin: Rect, start: Vec2, current: Vec2, bounds: Vec2) -> Rect {
    return Rect {
        pos: (origin.pos + current - start)
            .clamp(Vec2::ZERO, (bounds - origin.size).max(Vec2::ZERO)),
        size: origin.size,
    };
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
    let end = selection.pos + selection.size;
    if !point
        .cmpge(selection.pos - Vec2::splat(HANDLE_HIT_RADIUS))
        .all()
        || !point.cmple(end + Vec2::splat(HANDLE_HIT_RADIUS)).all()
    {
        return None;
    }
    let start_distance = (point - selection.pos).abs();
    let end_distance = (point - end).abs();
    let horizontal = (start_distance.x.min(end_distance.x) <= HANDLE_HIT_RADIUS)
        .then_some(start_distance.x <= end_distance.x);
    let vertical = (start_distance.y.min(end_distance.y) <= HANDLE_HIT_RADIUS)
        .then_some(start_distance.y <= end_distance.y);
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
fn create_action_bar_rect(selection: Rect, bounds: Vec2) -> Rect {
    let width = BAR_WIDTH.min((bounds.x - 16.0).max(1.0));
    let x =
        (selection.pos.x + selection.size.x - width).clamp(8.0, (bounds.x - width - 8.0).max(8.0));
    let below = selection.pos.y + selection.size.y + 6.0;
    let above = selection.pos.y - BAR_HEIGHT - 6.0;
    let y = if below + BAR_HEIGHT <= bounds.y - 8.0 {
        below
    } else if above >= 8.0 {
        above
    } else {
        (selection.pos.y + selection.size.y - BAR_HEIGHT - 6.0)
            .clamp(8.0, (bounds.y - BAR_HEIGHT - 8.0).max(8.0))
    };
    return Rect {
        pos: vec2(x, y),
        size: vec2(width, BAR_HEIGHT),
    };
}

fn get_capture_history_index(
    history: &[Rect],
    current: usize,
    direction: i32,
    bounds: Rect,
) -> Option<usize> {
    for offset in 1..history.len() {
        let index =
            (current as i32 + direction * offset as i32).rem_euclid(history.len() as i32) as usize;
        if history[index] == bounds {
            return Some(index);
        }
    }
    return None;
}

fn create_moved_pointer(pointer: Vec2, delta: Vec2, bounds: Vec2) -> Vec2 {
    return (pointer + delta).clamp(Vec2::ZERO, bounds.max(Vec2::ZERO));
}

#[cfg(test)]
mod tests {
    #[test]
    fn keyboard_pointer_movement_stays_on_screen() {
        let bounds = vec2(100.0, 80.0);
        assert_eq!(
            super::create_moved_pointer(vec2(0.0, 0.0), vec2(-1.0, -1.0), bounds),
            vec2(0.0, 0.0)
        );
        assert_eq!(
            super::create_moved_pointer(bounds, vec2(1.0, 1.0), bounds),
            bounds
        );
    }

    #[test]
    fn capture_history_wraps_and_skips_other_displays() {
        let display = Rect {
            pos: vec2(0.0, 0.0),
            size: vec2(1440.0, 900.0),
        };
        let other = Rect {
            pos: vec2(1440.0, 0.0),
            size: vec2(1920.0, 1080.0),
        };
        let history = [display, other, display];
        assert_eq!(
            super::get_capture_history_index(&history, 0, 1, display),
            Some(2)
        );
        assert_eq!(
            super::get_capture_history_index(&history, 2, -1, display),
            Some(0)
        );
        assert_eq!(
            super::get_capture_history_index(&history, 2, 1, display),
            Some(0)
        );
        assert_eq!(
            super::get_capture_history_index(&history[..2], 0, 1, display),
            None
        );
    }

    #[test]
    fn layers_panel_prefers_sides_and_stays_on_screen() {
        let screen = vec2(1440.0, 900.0);
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
            let bar = create_action_bar_rect(selection, screen);
            let panel = super::create_layers_panel_rect(selection, bar, screen, 20);
            if right {
                assert!(panel.pos.x >= selection.pos.x + selection.size.x + 8.0);
            } else {
                assert!(panel.pos.x + panel.size.x <= selection.pos.x - 8.0);
            }
            assert!(panel.pos.cmpge(vec2(8.0, 8.0)).all());
            assert!(
                (panel.pos + panel.size)
                    .cmple(screen - vec2(8.0, 8.0))
                    .all()
            );
        }
        for screen in [screen, vec2(200.0, 150.0)] {
            let selection = Rect {
                pos: glam::Vec2::ZERO,
                size: screen,
            };
            let panel = super::create_layers_panel_rect(
                selection,
                create_action_bar_rect(selection, screen),
                screen,
                100,
            );
            assert!(panel.pos.cmpge(vec2(8.0, 8.0)).all());
            assert!(
                (panel.pos + panel.size)
                    .cmple(screen - vec2(8.0, 8.0))
                    .all()
            );
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
            assert_eq!(super::get_resize_handle(point, rect), expected);
        }
        let tiny = Rect {
            pos: vec2(100.0, 100.0),
            size: vec2(8.0, 8.0),
        };
        assert_eq!(
            super::get_resize_handle(vec2(108.0, 100.0), tiny),
            Some(ResizeHandle::TopRight)
        );
        assert_eq!(
            super::get_resize_handle(vec2(108.0, 108.0), tiny),
            Some(ResizeHandle::BottomRight)
        );
    }

    #[test]
    fn window_hover_snaps_only_before_selection_and_leaves_target_undimmed() {
        let front = Rect {
            pos: vec2(100.0, 100.0),
            size: vec2(300.0, 200.0),
        };
        let back = Rect {
            pos: vec2(50.0, 50.0),
            size: vec2(500.0, 400.0),
        };
        let windows = [front, back];
        let pointer = vec2(150.0, 150.0);
        let hovered = super::get_hovered_capture_window(None, None, &windows, pointer);
        assert_eq!(hovered, Some(front));
        assert!(
            !get_dim_rects(hovered, vec2(1440.0, 900.0))
                .iter()
                .any(|rect| rect.intersects(pointer))
        );
        assert_eq!(
            super::get_hovered_capture_window(Some(front), None, &windows, pointer),
            None
        );
        assert_eq!(
            super::get_hovered_capture_window(None, Some(DragMode::New), &windows, pointer),
            None
        );
        assert_eq!(
            super::get_hovered_capture_window(None, None, &windows, vec2(1000.0, 800.0)),
            None
        );
    }

    #[test]
    fn magnifier_samples_retina_pixels_and_stays_inside_screen_edges() {
        let image =
            image::RgbaImage::from_fn(8, 6, |x, y| image::Rgba([x as u8, y as u8, 40, 255]));
        assert_eq!(
            super::get_sampled_pixel(&image, vec2(4.0, 3.0), vec2(1.5, 2.0)),
            Some((3, 4, crate::geometry::Color::from_rgb(3, 4, 40)))
        );
        assert_eq!(
            super::get_sampled_pixel(&image, vec2(4.0, 3.0), vec2(4.0, 3.0))
                .unwrap()
                .0,
            7
        );
        assert_eq!(
            super::get_sampled_pixel(&image, vec2(4.0, 3.0), vec2(-1.0, -1.0))
                .unwrap()
                .1,
            0
        );
        assert!(super::get_sampled_pixel(&image, glam::Vec2::ZERO, glam::Vec2::ZERO).is_none());
        assert_eq!(
            super::create_color_label(crate::geometry::Color(0x0102ff), false),
            "#0102ff"
        );
        assert_eq!(
            super::create_color_label(crate::geometry::Color(0x0102ff), true),
            "1, 2, 255"
        );
        for screen in [vec2(1440.0, 900.0), vec2(80.0, 70.0)] {
            for pointer in [
                glam::Vec2::ZERO,
                screen,
                vec2(10.0, screen.y),
                vec2(screen.x, 10.0),
            ] {
                let rect = super::create_magnifier_rect(pointer, screen);
                assert!(rect.pos.cmpge(glam::Vec2::ZERO).all());
                assert!((rect.pos + rect.size).cmple(screen).all());
            }
        }
    }

    #[test]
    fn double_click_selects_unicode_words() {
        assert_eq!(
            super::create_clicked_text_range("😀 café world", 5, 2),
            3..7
        );
        assert_eq!(
            super::create_clicked_text_range("😀 café world", 5, 1),
            5..5
        );
        assert_eq!(super::create_clicked_text_range("", 0, 2), 0..0);
        assert_eq!(super::create_clicked_text_range("word", 4, 2), 0..4);
    }

    use super::{
        Annotation, CursorStyle, DragMode, ResizeHandle, Tool, build_annotation_path,
        create_action_bar_rect, create_resized_selection, get_dim_rects, get_selection_cursor,
    };
    use crate::geometry::Rect;
    use glam::vec2;
    #[test]
    fn active_drag_cursor_wins_over_hovered_resize_handles() {
        let rect = Rect {
            pos: vec2(20.0, 20.0),
            size: vec2(100.0, 100.0),
        };
        assert_eq!(
            get_selection_cursor(Some(DragMode::Move), Tool::Select, rect.pos, Some(rect)),
            CursorStyle::ClosedHand
        );
        assert_eq!(
            get_selection_cursor(Some(DragMode::New), Tool::Select, rect.pos, Some(rect)),
            CursorStyle::Crosshair
        );
        assert_eq!(
            get_selection_cursor(Some(DragMode::Draw), Tool::Pen, rect.pos, Some(rect)),
            CursorStyle::Crosshair
        );
        assert_eq!(
            get_selection_cursor(
                Some(DragMode::Resize(ResizeHandle::Top)),
                Tool::Select,
                rect.pos,
                Some(rect)
            ),
            CursorStyle::ResizeUpDown
        );
        assert_eq!(
            get_selection_cursor(None, Tool::Select, rect.pos, Some(rect)),
            CursorStyle::ResizeUpLeftDownRight
        );
    }

    #[test]
    fn annotation_paths_tessellate_without_bitmap_rendering() {
        for tool in [
            Tool::Rectangle,
            Tool::Ellipse,
            Tool::Arrow,
            Tool::Line,
            Tool::Pen,
            Tool::Highlighter,
        ] {
            let annotation = Annotation {
                tool,
                points: vec![vec2(20.0, 20.0), vec2(100.0, 80.0)],
                color: crate::geometry::Color::WHITE,
                width: 3.0,
                text: String::new(),
                ..Default::default()
            };
            assert!(build_annotation_path(&annotation).unwrap().is_some());
        }
    }

    #[test]
    fn capture_dims_full_display_until_a_region_is_selected() {
        let size = vec2(1440.0, 900.0);
        assert_eq!(
            get_dim_rects(None, size),
            vec![Rect {
                pos: glam::Vec2::ZERO,
                size
            }]
        );
        let selection = Rect {
            pos: vec2(200.0, 100.0),
            size: vec2(300.0, 200.0),
        };
        let dim = get_dim_rects(Some(selection), size);
        assert!(!dim.iter().any(|rect| rect.intersects(vec2(350.0, 200.0))));
        assert_eq!(
            dim.iter()
                .map(|rect| rect.size.x * rect.size.y)
                .sum::<f32>(),
            size.x * size.y - 300.0 * 200.0
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
        let bar = create_action_bar_rect(origin, vec2(200.0, 100.0));
        assert!(bar.pos.x >= 0.0 && bar.pos.x + bar.size.x <= 200.0);
    }
}
