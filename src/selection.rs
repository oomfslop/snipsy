use crate::{
    app::{self, AppState},
    editor::{Annotation, Editor, Screenshot, Tool, get_byte_offset},
    geometry::{Color, Rect},
    lens, pin, raster,
};
use glam::{Vec2, vec2};
use gpui::{prelude::*, *};
use image::RgbaImage;
use std::{ops::Range, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

const HANDLE_RADIUS: f32 = 5.0;
const HANDLE_HIT_RADIUS: f32 = 10.0;
const BAR_WIDTH: f32 = 632.0;
const BAR_HEIGHT: f32 = 48.0;
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
    Erase,
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
    marked_range: Option<Range<usize>>,
    pending_action: Option<Action>,
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
            pointer: Vec2::ZERO,
            start: Vec2::ZERO,
            drag_origin: None,
            annotation_origin: Vec::new(),
            drag_mode: None,
            drag_recorded: false,
            focus,
            error: None,
            text_range: 0..0,
            text_reversed: false,
            marked_range: None,
            pending_action: None,
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
        self.background = Some(background);
        self.screenshot = Some(screenshot);
        self.activate(window, cx);
        if let Some(action) = self.pending_action.take() {
            self.perform_action(action, window, cx);
        }
        cx.notify();
    }

    fn begin_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.pointer = vec2(event.position.x.into(), event.position.y.into())
            .clamp(Vec2::ZERO, self.bounds.size);
        self.start = self.pointer;
        self.drag_recorded = false;
        self.editor.commit_draft();
        if let Some(selection) = self.selection {
            if let Some(handle) = get_resize_handle(self.pointer, selection) {
                self.drag_origin = Some(selection);
                self.drag_mode = Some(DragMode::Resize(handle));
                cx.notify();
                return;
            }
            if selection.intersects(self.pointer) {
                match self.editor.tool {
                    Tool::Select => {
                        if let Some(index) = self.editor.get_annotation_at(self.pointer) {
                            if event.click_count >= 2
                                && self.editor.annotations[index].tool == Tool::Text
                            {
                                self.editor.start_editing_text(index);
                                let end = self
                                    .editor
                                    .draft
                                    .as_ref()
                                    .unwrap()
                                    .text
                                    .encode_utf16()
                                    .count();
                                self.text_range = end..end;
                                self.text_reversed = false;
                                self.marked_range = None;
                                if let Some(offset) =
                                    self.character_index_for_point(event.position, window, cx)
                                {
                                    self.text_range = offset..offset;
                                }
                                cx.notify();
                                return;
                            }
                            self.editor.selected = Some(index);
                            let annotation = &self.editor.annotations[index];
                            self.editor.color = annotation.color;
                            self.editor.width = match annotation.tool {
                                Tool::Text => ((annotation.width - 12.0) / 4.0).clamp(1.0, 12.0),
                                Tool::Highlighter => annotation.width / 6.0,
                                _ => annotation.width,
                            };
                            self.annotation_origin = self.editor.annotations[index].points.clone();
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
                            if let Some(index) = self
                                .editor
                                .get_annotation_at(self.pointer)
                                .filter(|index| self.editor.annotations[*index].tool == Tool::Text)
                            {
                                self.editor.start_editing_text(index);
                                let end = self
                                    .editor
                                    .draft
                                    .as_ref()
                                    .unwrap()
                                    .text
                                    .encode_utf16()
                                    .count();
                                self.text_range = end..end;
                                self.text_reversed = false;
                                self.marked_range = None;
                                if let Some(offset) =
                                    self.character_index_for_point(event.position, window, cx)
                                {
                                    self.text_range = offset..offset;
                                }
                                cx.notify();
                                return;
                            }
                            self.text_range = 0..0;
                            self.text_reversed = false;
                            self.marked_range = None;
                        }
                        self.editor.draft = Some(Annotation {
                            tool,
                            points: vec![self.pointer],
                            color: self.editor.color,
                            width: if tool == Tool::Text {
                                self.editor.width * 4.0 + 12.0
                            } else if tool == Tool::Highlighter {
                                self.editor.width * 6.0
                            } else {
                                self.editor.width
                            },
                            text: String::new(),
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
        self.drag_mode = Some(DragMode::New);
        self.drag_origin = None;
        self.selection = None;
        self.editor.selected = None;
        cx.notify();
    }

    fn update_drag(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.get_cursor();
        let hovered = self.get_hovered_window();
        self.pointer = vec2(event.position.x.into(), event.position.y.into())
            .clamp(Vec2::ZERO, self.bounds.size);
        if self.drag_mode.is_none() {
            if cursor != self.get_cursor() || hovered != self.get_hovered_window() {
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
                Tool::Arrow => {
                    let angle = (delta.y.atan2(delta.x) / std::f32::consts::FRAC_PI_4).round()
                        * std::f32::consts::FRAC_PI_4;
                    point = self.start + vec2(angle.cos(), angle.sin()) * delta.length();
                }
                _ => {}
            }
        }
        match self.drag_mode {
            Some(DragMode::New) => self.selection = Some(create_selection_rect(self.start, point)),
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
                    self.selection = Some(create_resized_selection(origin, handle, point));
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
                    self.editor.annotations[index].points = self
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
            self.selection = self
                .screenshot
                .as_ref()
                .map(|screenshot| screenshot.windows.as_slice())
                .unwrap_or_default()
                .iter()
                .find(|rect| rect.intersects(self.pointer))
                .copied();
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
            self.editor.annotations.remove(index);
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
            self.editor.annotations.iter(),
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
                                app::remember_capture(image, cx);
                                self.dismiss(window, cx);
                            }
                            Action::Save => app::save_image(image, cx),
                            Action::Pin => {
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
        let tool = match key {
            "v" => Some(Tool::Select),
            "r" => Some(Tool::Rectangle),
            "o" => Some(Tool::Ellipse),
            "a" => Some(Tool::Arrow),
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
            "enter" => self.perform_action(Action::Copy, window, cx),
            "backspace" | "delete" => {
                self.editor.delete_selected();
                cx.notify();
            }
            "[" => {
                self.editor.set_width(self.editor.width - 1.0);
                cx.notify();
            }
            "]" => {
                self.editor.set_width(self.editor.width + 1.0);
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
                        for point in &mut self.editor.annotations[index].points {
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
            .size(px(36.0))
            .flex_shrink_0()
            .rounded_md()
            .cursor_pointer()
            .bg(if selected { rgb(ACCENT) } else { rgba(0) })
            .text_color(if enabled {
                rgb(0xf2f2f2)
            } else {
                rgb(0x66666d)
            })
            .hover(move |s| s.bg(rgb(if selected { 0x448cff } else { 0x414148 })))
            .child(svg().path(icon).size(px(21.0)).text_color(if enabled {
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
                "a" => {
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
                    self.text_range = 0..0;
                    self.text_reversed = false;
                }
                "end" => {
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
            &[TextRun {
                len: line.len(),
                font: font("Arial"),
                color: rgb(draft.color.0).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            }],
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
        if self.selection.is_some() {
            return None;
        }
        return self
            .screenshot
            .as_ref()?
            .windows
            .iter()
            .find(|rect| rect.intersects(self.pointer))
            .copied();
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
        draft.text.replace_range(start..end, text);
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
        for (index, line) in draft.text.split('\n').enumerate() {
            if index == row {
                let shaped = window.text_system().shape_line(
                    line.to_string().into(),
                    px(draft.width),
                    &[TextRun {
                        len: line.len(),
                        font: font("Arial"),
                        color: rgb(draft.color.0).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                );
                let byte = shaped.closest_index_for_x(point.x - px(draft.points[0].x));
                return Some(offset + line[..byte].encode_utf16().count());
            }
            offset += line.encode_utf16().count() + 1;
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
        let border = selection.or(hovered);
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
                            for rect in get_dim_rects(selection, screen_size) {
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
                    .top(px((rect.pos.y - 34.0).max(8.0)))
                    .px_2()
                    .py_1()
                    .rounded_md()
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
                let rect = self.editor.annotations[index].get_bounds();
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
            if self.drag_mode.is_none() {
                let bar = create_action_bar_rect(selection, screen_size);
                let tools = [
                    ("select", "Select / move (V)", Tool::Select),
                    ("rectangle", "Rectangle (R)", Tool::Rectangle),
                    ("ellipse", "Ellipse (O)", Tool::Ellipse),
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
                        .gap(px(2.0))
                        .px(px(6.0))
                        .rounded_lg()
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
                                self.editor.tool == tool,
                                true,
                                cx,
                            )
                        }))
                        .child(
                            div()
                                .w(px(1.0))
                                .h(px(22.0))
                                .mx(px(4.0))
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
                        .child(self.build_button(
                            "save",
                            "Save (⌘S)",
                            Action::Save,
                            false,
                            true,
                            cx,
                        ))
                        .child(self.build_button(
                            "copy",
                            "Copy (Enter / ⌘C)",
                            Action::Copy,
                            true,
                            true,
                            cx,
                        )),
                );
                if (self.editor.tool != Tool::Select && self.editor.tool != Tool::Eraser)
                    || self.editor.selected.is_some()
                {
                    let palette_y = if bar.pos.y + BAR_HEIGHT + 44.0 <= screen_size.y - 8.0 {
                        bar.pos.y + BAR_HEIGHT + 6.0
                    } else {
                        (bar.pos.y - 44.0).max(8.0)
                    };
                    root = root.child(
                        div()
                            .cursor(CursorStyle::Arrow)
                            .absolute()
                            .left(px(bar.pos.x))
                            .top(px(palette_y))
                            .flex()
                            .items_center()
                            .gap_2()
                            .p_2()
                            .rounded_lg()
                            .bg(rgb(0x28282d))
                            .border_1()
                            .border_color(rgb(0x44444c))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .children(COLORS.into_iter().map(|color| {
                                div()
                                    .id(("color", color as usize))
                                    .size(px(20.0))
                                    .rounded_full()
                                    .bg(rgb(color))
                                    .border_2()
                                    .border_color(if self.editor.color.0 == color {
                                        rgb(0xffffff)
                                    } else {
                                        rgb(0x55555d)
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |state, _, _, cx| {
                                        state.editor.color = Color(color);
                                        if let Some(index) = state.editor.selected {
                                            state.editor.record_change();
                                            state.editor.annotations[index].color = Color(color);
                                        }
                                        if let Some(draft) = &mut state.editor.draft {
                                            draft.color = Color(color);
                                        }
                                        cx.notify();
                                    }))
                            }))
                            .child(div().w(px(1.0)).h(px(18.0)).bg(rgb(0x55555d)))
                            .children([2.0, 4.0, 8.0].into_iter().enumerate().map(
                                |(index, width)| {
                                    div()
                                        .id(("width", index))
                                        .size(px(22.0))
                                        .rounded_md()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .bg(if self.editor.width == width {
                                            rgb(0x484850)
                                        } else {
                                            rgba(0)
                                        })
                                        .child(
                                            div()
                                                .size(px(width + 2.0))
                                                .rounded_full()
                                                .bg(rgb(0xeeeeee)),
                                        )
                                        .on_click(cx.listener(move |state, _, _, cx| {
                                            state.editor.set_width(width);
                                            cx.notify();
                                        }))
                                },
                            )),
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
            .px_2()
            .py_1()
            .rounded_md()
            .bg(rgb(0x19191c))
            .text_color(rgb(0xffffff))
            .text_sm()
            .child(self.0.clone())
    }
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

fn get_selection_cursor(
    drag: Option<DragMode>,
    tool: Tool,
    pointer: Vec2,
    selection: Option<Rect>,
) -> CursorStyle {
    match drag {
        Some(DragMode::New | DragMode::Draw | DragMode::Erase) => return CursorStyle::Crosshair,
        Some(DragMode::Move | DragMode::MoveAnnotation(_)) => return CursorStyle::ClosedHand,
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
        Tool::Arrow => {
            if start == end {
                return Ok(None);
            }
            let [left, right] = crate::editor::get_arrow_head(start, end, annotation.width);
            path.move_to(at(start));
            path.line_to(at(end));
            path.move_to(at(left));
            path.line_to(at(end));
            path.line_to(at(right));
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
        for (index, text) in annotation.text.split('\n').enumerate() {
            let line = window.text_system().shape_line(
                text.to_string().into(),
                px(annotation.width),
                &[TextRun {
                    len: text.len(),
                    font: font("Arial"),
                    color: color.into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            let height = px(annotation.width * 1.3);
            let baseline =
                px(annotation.points[0].y + annotation.width * (1.0 + index as f32 * 1.3));
            let y = baseline - line.ascent - (height - line.ascent - line.descent) / 2.0;
            line.paint(point(px(annotation.points[0].x), y), height, window, cx)
                .map_err(|error| error.to_string())?;
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
    return get_resize_handle_centers(selection)
        .into_iter()
        .find_map(|(handle, center)| {
            ((point - center).abs().max_element() <= HANDLE_HIT_RADIUS).then_some(handle)
        });
}
fn create_action_bar_rect(selection: Rect, bounds: Vec2) -> Rect {
    let width = BAR_WIDTH.min((bounds.x - 16.0).max(1.0));
    let x =
        (selection.pos.x + selection.size.x - width).clamp(8.0, (bounds.x - width - 8.0).max(8.0));
    let below = selection.pos.y + selection.size.y + 10.0;
    let above = selection.pos.y - BAR_HEIGHT - 10.0;
    let y = if below + BAR_HEIGHT <= bounds.y - 8.0 {
        below
    } else if above >= 8.0 {
        above
    } else {
        (selection.pos.y + selection.size.y - BAR_HEIGHT - 10.0)
            .clamp(8.0, (bounds.y - BAR_HEIGHT - 8.0).max(8.0))
    };
    return Rect {
        pos: vec2(x, y),
        size: vec2(width, BAR_HEIGHT),
    };
}

#[cfg(test)]
mod tests {
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
            Tool::Pen,
            Tool::Highlighter,
        ] {
            let annotation = Annotation {
                tool,
                points: vec![vec2(20.0, 20.0), vec2(100.0, 80.0)],
                color: crate::geometry::Color::WHITE,
                width: 3.0,
                text: String::new(),
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
