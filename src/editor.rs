use crate::geometry::{Color, Rect};
use glam::{Vec2, vec2};
use std::ops::Range;

pub const LINE_HEIGHT: f32 = 1.3;
/// Previews and exports render with the same font so saved images match the editor.
pub const ANNOTATION_FONT: &str = "Arial";
pub const HIGHLIGHTER_OPACITY: f32 = 0.35;
const HIGHLIGHTER_COLOR: Color = Color(0xffdf38);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    Rectangle,
    Ellipse,
    Arrow,
    Line,
    Pen,
    Highlighter,
    Text,
    Mosaic,
    Eraser,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BlurMode {
    #[default]
    Pixelated,
    Gaussian,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextFormat {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAttribute {
    Bold,
    Italic,
    Underline,
}

impl TextFormat {
    pub fn get_attribute(self, attribute: TextAttribute) -> bool {
        return match attribute {
            TextAttribute::Bold => self.bold,
            TextAttribute::Italic => self.italic,
            TextAttribute::Underline => self.underline,
        };
    }
    pub fn set_attribute(&mut self, attribute: TextAttribute, value: bool) {
        match attribute {
            TextAttribute::Bold => self.bold = value,
            TextAttribute::Italic => self.italic = value,
            TextAttribute::Underline => self.underline = value,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub tool: Tool,
    pub points: Vec<Vec2>,
    pub color: Color,
    pub width: f32,
    pub text: String,
    pub filled: bool,
    pub blur_mode: BlurMode,
    pub intensity: f32,
    pub text_format: TextFormat,
    pub formats: Vec<TextFormat>,
}

impl Default for Annotation {
    fn default() -> Self {
        return Self {
            tool: Tool::Select,
            points: vec![Vec2::ZERO],
            color: Color::WHITE,
            width: 3.0,
            text: String::new(),
            filled: false,
            blur_mode: BlurMode::Pixelated,
            intensity: 12.0,
            text_format: TextFormat::default(),
            formats: Vec::new(),
        };
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub id: u64,
    pub annotation: Annotation,
    pub time: Option<(f64, f64)>,
}
impl std::ops::Deref for Layer {
    type Target = Annotation;
    fn deref(&self) -> &Annotation {
        return &self.annotation;
    }
}
impl std::ops::DerefMut for Layer {
    fn deref_mut(&mut self) -> &mut Annotation {
        return &mut self.annotation;
    }
}

impl Annotation {
    pub fn get_bounds(&self) -> Rect {
        let mut min = self.points[0];
        let mut max = min;
        for point in &self.points {
            min = min.min(*point);
            max = max.max(*point);
        }
        if self.tool == Tool::Arrow {
            for point in get_arrow_head(&self.points, self.width) {
                min = min.min(point);
                max = max.max(point);
            }
        }
        if self.tool == Tool::Text {
            let lines: Vec<_> = self.text.split('\n').collect();
            max = min
                + vec2(
                    lines
                        .iter()
                        .map(|line| line.chars().count())
                        .max()
                        .unwrap_or(0) as f32
                        * self.width
                        * 0.7,
                    lines.len() as f32 * self.width * LINE_HEIGHT,
                );
        }
        return Rect {
            pos: min - Vec2::splat(self.width / 2.0),
            size: max - min + Vec2::splat(self.width),
        };
    }

    pub fn get_format(&self, character: usize) -> TextFormat {
        return self
            .formats
            .get(character)
            .copied()
            .unwrap_or(self.text_format);
    }

    pub fn replace_text(&mut self, range: Range<usize>, text: &str, format: TextFormat) {
        let start = self.text[..range.start].chars().count();
        let end = self.text[..range.end].chars().count();
        self.formats
            .resize(self.text.chars().count(), self.text_format);
        self.formats.splice(
            start..end,
            std::iter::repeat_n(format, text.chars().count()),
        );
        self.text.replace_range(range, text);
    }

    pub fn set_text_attribute(
        &mut self,
        range: Range<usize>,
        attribute: TextAttribute,
        enabled: bool,
    ) {
        let start = self.text[..range.start].chars().count();
        let end = self.text[..range.end].chars().count();
        self.formats
            .resize(self.text.chars().count(), self.text_format);
        for format in &mut self.formats[start..end] {
            format.set_attribute(attribute, enabled);
        }
        if start == 0 && end == self.formats.len() {
            self.text_format.set_attribute(attribute, enabled);
        }
    }

    pub fn get_format_runs(&self, range: Range<usize>) -> Vec<(usize, TextFormat)> {
        let offset = self.text[..range.start].chars().count();
        let mut runs: Vec<(usize, TextFormat)> = Vec::new();
        for (index, character) in self.text[range].chars().enumerate() {
            let format = self.get_format(offset + index);
            if let Some((length, _)) = runs.last_mut().filter(|(_, previous)| *previous == format) {
                *length += character.len_utf8();
            } else {
                runs.push((character.len_utf8(), format));
            }
        }
        return runs;
    }

    pub fn contains_point(&self, point: Vec2) -> bool {
        if (self.tool == Tool::Rectangle && self.filled)
            || self.tool == Tool::Text
            || self.tool == Tool::Mosaic
        {
            return self.get_bounds().contains(point);
        }
        let start = self.points[0];
        let end = *self.points.last().unwrap();
        let tolerance = self.width / 2.0 + 6.0;
        match self.tool {
            Tool::Rectangle => {
                let min = start.min(end);
                let max = start.max(end);
                return [
                    (min, vec2(max.x, min.y)),
                    (vec2(max.x, min.y), max),
                    (max, vec2(min.x, max.y)),
                    (vec2(min.x, max.y), min),
                ]
                .iter()
                .any(|(a, b)| get_segment_distance(point, *a, *b) <= tolerance);
            }
            Tool::Ellipse => {
                let radii = (end - start).abs() / 2.0;
                if radii.min_element() < 1.0 {
                    return get_segment_distance(point, start, end) <= tolerance;
                }
                let center = (start + end) / 2.0;
                if self.filled {
                    return ((point - center) / radii).length() <= 1.0;
                }
                let arc = lyon::geom::Arc {
                    center: lyon::math::point(center.x, center.y),
                    radii: lyon::math::vector(radii.x, radii.y),
                    start_angle: lyon::math::Angle::zero(),
                    sweep_angle: lyon::math::Angle::two_pi(),
                    x_rotation: lyon::math::Angle::zero(),
                };
                let mut from = arc.from();
                return arc.flattened(0.1).any(|to| {
                    let segment = lyon::geom::LineSegment { from, to };
                    from = to;
                    segment.distance_to_point(lyon::math::point(point.x, point.y)) <= tolerance
                });
            }
            Tool::Arrow => {
                let [left, right] = get_arrow_head(&self.points, self.width);
                return self
                    .points
                    .windows(2)
                    .any(|pair| get_segment_distance(point, pair[0], pair[1]) <= tolerance)
                    || [(left, end), (right, end)]
                        .iter()
                        .any(|(a, b)| get_segment_distance(point, *a, *b) <= tolerance);
            }
            _ => {
                return self
                    .points
                    .windows(2)
                    .any(|pair| get_segment_distance(point, pair[0], pair[1]) <= tolerance)
                    || point.distance(start) <= tolerance;
            }
        }
    }
}

pub struct Editor {
    pub tool: Tool,
    pub color: Color,
    pub width: f32,
    pub highlighter_width: f32,
    pub layers: Vec<Layer>,
    pub draft: Option<Annotation>,
    pub editing_index: Option<usize>,
    pub selected: Option<usize>,
    pub undo: Vec<Vec<Layer>>,
    pub redo: Vec<Vec<Layer>>,
    pub time: f64,
    next_layer_id: u64,
    settings_history: Option<(u64, &'static str)>,
    pub filled: bool,
    pub blur_mode: BlurMode,
    pub intensity: f32,
    pub text_size: f32,
    pub text_format: TextFormat,
}

impl Editor {
    pub fn new() -> Self {
        return Self {
            tool: Tool::Select,
            color: Color::from_rgb(255, 75, 75),
            width: 3.0,
            highlighter_width: 18.0,
            layers: Vec::new(),
            draft: None,
            editing_index: None,
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
            time: 0.0,
            next_layer_id: 1,
            settings_history: None,
            filled: false,
            blur_mode: BlurMode::Pixelated,
            intensity: 12.0,
            text_size: 24.0,
            text_format: TextFormat::default(),
        };
    }

    pub fn record_change(&mut self) {
        self.settings_history = None;
        self.undo.push(self.layers.clone());
        if self.undo.len() > 100 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn commit_draft(&mut self) {
        let Some(draft) = self.draft.take() else {
            return;
        };
        if draft.tool == Tool::Text && draft.text.trim().is_empty() {
            if let Some(index) = self.editing_index.take() {
                self.record_change();
                self.layers.remove(index);
                self.selected = None;
            }
            return;
        }
        if draft.tool != Tool::Text
            && draft.tool != Tool::Pen
            && draft.tool != Tool::Highlighter
            && !(matches!(draft.tool, Tool::Line | Tool::Arrow) && draft.points.len() > 2)
            && draft.points[0].distance(*draft.points.last().unwrap()) < 1.0
        {
            return;
        }
        let index = if let Some(index) = self.editing_index.take() {
            if self.layers[index].annotation != draft {
                self.record_change();
                self.layers[index].annotation = draft;
            }
            index
        } else {
            self.record_change();
            let index = self.layers.len();
            self.layers.push(Layer {
                id: self.next_layer_id,
                annotation: draft,
                time: None,
            });
            self.next_layer_id += 1;
            index
        };
        self.selected = Some(index);
    }

    pub fn select_tool(&mut self, tool: Tool) {
        self.settings_history = None;
        self.commit_draft();
        self.tool = tool;
        self.selected = None;
        if tool == Tool::Highlighter {
            self.color = HIGHLIGHTER_COLOR;
        }
    }

    pub fn undo_change(&mut self) {
        self.settings_history = None;
        if self.draft.take().is_some() {
            self.editing_index = None;
            return;
        }
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.layers, previous));
        }
        self.selected = None;
    }

    pub fn redo_change(&mut self) {
        self.settings_history = None;
        if let Some(next) = self.redo.pop() {
            self.draft = None;
            self.editing_index = None;
            self.undo.push(std::mem::replace(&mut self.layers, next));
            self.selected = None;
        }
    }

    pub fn delete_selected(&mut self) {
        if let Some(index) = self.selected.take() {
            self.record_change();
            self.layers.remove(index);
        }
    }

    pub fn start_editing_text(&mut self, index: usize) {
        self.commit_draft();
        self.draft = Some(self.layers[index].annotation.clone());
        self.editing_index = Some(index);
        self.selected = None;
    }

    pub fn get_timed_annotations(&self) -> impl Iterator<Item = (&Annotation, Option<(f64, f64)>)> {
        return self
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| {
                let annotation = if Some(index) == self.editing_index {
                    self.draft.as_ref().unwrap_or(&layer.annotation)
                } else {
                    &layer.annotation
                };
                (annotation, layer.time)
            })
            .chain(
                self.draft
                    .iter()
                    .filter(|_| self.editing_index.is_none())
                    .map(|draft| (draft, None)),
            );
    }

    pub fn get_visible_annotations(&self) -> impl Iterator<Item = &Annotation> {
        return self
            .get_timed_annotations()
            .filter(|(_, range)| get_is_in_time_range(*range, self.time))
            .map(|(annotation, _)| annotation);
    }

    pub fn get_is_layer_visible(&self, index: usize) -> bool {
        return get_is_in_time_range(self.layers[index].time, self.time);
    }

    pub fn set_time(&mut self, time: f64) -> bool {
        if time == self.time {
            return false;
        }
        let previous = std::mem::replace(&mut self.time, time);
        let changed = self.layers.iter().any(|layer| {
            get_is_in_time_range(layer.time, previous) != get_is_in_time_range(layer.time, time)
        });
        if self
            .selected
            .is_some_and(|index| !self.get_is_layer_visible(index))
        {
            self.selected = None;
            return true;
        }
        return changed;
    }

    pub fn map_layers(&mut self, from: Rect, to: Rect) {
        let scale = to.size.x / from.size.x;
        let map = |annotation: &mut Annotation| {
            for point in &mut annotation.points {
                *point = to.pos + (*point - from.pos) * scale;
            }
            annotation.width *= scale;
            annotation.intensity *= scale;
        };
        for layers in std::iter::once(&mut self.layers)
            .chain(&mut self.undo)
            .chain(&mut self.redo)
        {
            layers.iter_mut().for_each(|layer| map(layer));
        }
        self.draft.iter_mut().for_each(map);
    }

    pub fn set_layer_time(&mut self, index: usize, time: (f64, f64)) {
        if self.layers[index].time == Some(time) {
            return;
        }
        let id = self.layers[index].id;
        if self.settings_history != Some((id, "time")) {
            self.record_change();
        }
        self.settings_history = Some((id, "time"));
        self.layers[index].time = Some(time);
    }

    pub fn get_current_annotation(&self) -> Option<&Annotation> {
        return self
            .draft
            .as_ref()
            .or_else(|| self.selected.map(|index| &self.layers[index].annotation));
    }

    pub fn get_current_tool(&self) -> Tool {
        return self
            .get_current_annotation()
            .map_or(self.tool, |annotation| annotation.tool);
    }

    pub fn update_current_annotation(
        &mut self,
        key: &'static str,
        f: impl FnOnce(&mut Annotation),
    ) {
        if let Some(draft) = &mut self.draft {
            f(draft);
        } else if let Some(index) = self.selected {
            let mut annotation = self.layers[index].annotation.clone();
            f(&mut annotation);
            if annotation == self.layers[index].annotation {
                return;
            }
            let id = self.layers[index].id;
            if self.settings_history != Some((id, key)) {
                self.record_change();
            }
            self.settings_history = Some((id, key));
            self.layers[index].annotation = annotation;
        }
    }

    pub fn set_color(&mut self, color: Color) {
        self.color = color;
        self.update_current_annotation("color", |annotation| annotation.color = color);
    }

    pub fn set_variant(&mut self, tool: Tool) {
        if self.tool != Tool::Select {
            self.tool = tool;
        }
        self.update_current_annotation("variant", |annotation| annotation.tool = tool);
    }

    pub fn set_filled(&mut self, filled: bool) {
        self.filled = filled;
        self.update_current_annotation("fill", |annotation| annotation.filled = filled);
    }

    pub fn set_blur_mode(&mut self, mode: BlurMode) {
        self.blur_mode = mode;
        self.update_current_annotation("blur", |annotation| annotation.blur_mode = mode);
    }

    pub fn set_intensity(&mut self, intensity: f32) {
        self.intensity = intensity.clamp(1.0, 48.0);
        let value = self.intensity;
        self.update_current_annotation("intensity", |annotation| annotation.intensity = value);
    }

    pub fn set_text_size(&mut self, size: f32) {
        self.text_size = size.clamp(8.0, 144.0);
        let value = self.text_size;
        self.update_current_annotation("text-size", |annotation| annotation.width = value);
    }

    pub fn move_layer(&mut self, id: u64, target_id: u64, above: bool) {
        self.commit_draft();
        let Some(index) = self.layers.iter().position(|layer| layer.id == id) else {
            return;
        };
        let Some(target) = self.layers.iter().position(|layer| layer.id == target_id) else {
            return;
        };
        if index == target {
            return;
        }
        let slot = target + usize::from(above);
        let next = slot - usize::from(index < slot);
        if index == next {
            return;
        }
        self.record_change();
        let layer = self.layers.remove(index);
        self.layers.insert(next, layer);
        self.selected = Some(next);
    }

    fn get_tool_width(&self, tool: Tool) -> f32 {
        return match tool {
            Tool::Text => self.text_size,
            Tool::Highlighter => self.highlighter_width,
            _ => self.width,
        };
    }

    pub fn get_current_width(&self) -> f32 {
        return self.get_current_annotation().map_or_else(
            || self.get_tool_width(self.tool),
            |annotation| annotation.width,
        );
    }

    pub fn create_annotation(&self, tool: Tool, point: Vec2) -> Annotation {
        return Annotation {
            tool,
            points: vec![point],
            color: self.color,
            width: self.get_tool_width(tool),
            filled: self.filled,
            blur_mode: self.blur_mode,
            intensity: self.intensity,
            text_format: self.text_format,
            ..Default::default()
        };
    }

    pub fn select_layer(&mut self, index: usize) {
        self.selected = Some(index);
        let annotation = &self.layers[index].annotation;
        self.color = annotation.color;
        match annotation.tool {
            Tool::Text => self.text_size = annotation.width,
            Tool::Highlighter => self.highlighter_width = annotation.width,
            _ => self.width = annotation.width,
        }
    }

    pub fn finish_settings_change(&mut self) {
        self.settings_history = None;
    }

    pub fn adjust_size(&mut self, step: f32) {
        let width = self.get_current_width();
        if self.get_current_tool() == Tool::Text {
            self.set_text_size(width + step * 2.0);
        } else {
            self.set_width(width + step);
        }
    }

    pub fn set_width(&mut self, width: f32) {
        let value = width.clamp(1.0, 64.0);
        if self.get_current_tool() == Tool::Highlighter {
            self.highlighter_width = value;
        } else {
            self.width = value;
        }
        self.update_current_annotation("width", |annotation| {
            if annotation.tool != Tool::Text {
                annotation.width = value;
            }
        });
    }
}

pub fn get_is_in_time_range(range: Option<(f64, f64)>, time: f64) -> bool {
    return range.is_none_or(|(start, end)| (start..=end).contains(&time));
}

pub fn get_byte_offset(text: &str, utf16_offset: usize) -> usize {
    let mut offset = 0;
    for (byte, character) in text.char_indices() {
        if offset + character.len_utf16() > utf16_offset {
            return byte;
        }
        offset += character.len_utf16();
    }
    return text.len();
}

pub fn get_arrow_head(points: &[Vec2], width: f32) -> [Vec2; 2] {
    let end = *points.last().unwrap();
    let start = points
        .iter()
        .rev()
        .copied()
        .find(|point| *point != end)
        .unwrap_or(end);
    let direction = (end - start).normalize_or_zero();
    let normal = vec2(-direction.y, direction.x);
    let length = (width * 4.0).max(12.0).min(start.distance(end) * 0.6);
    return [
        end - direction * length + normal * length * 0.5,
        end - direction * length - normal * length * 0.5,
    ];
}

fn get_segment_distance(point: Vec2, start: Vec2, end: Vec2) -> f32 {
    let delta = end - start;
    let length = delta.length_squared();
    if length == 0.0 {
        return point.distance(start);
    }
    return point.distance(start + delta * ((point - start).dot(delta) / length).clamp(0.0, 1.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_edit_latest_layer_and_keep_identity_with_undo() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Rectangle,
            points: vec![Vec2::ZERO, vec2(50.0, 50.0)],
            ..Default::default()
        });
        editor.commit_draft();
        let id = editor.layers[0].id;
        editor.set_color(Color(0xff0000));
        editor.set_color(Color(0x00ff00));
        assert_eq!(editor.undo.len(), 2);
        assert_eq!(editor.layers[0].id, id);
        editor.undo_change();
        assert_eq!(editor.layers[0].color, Color::WHITE);
        editor.selected = Some(0);
        editor.set_variant(Tool::Ellipse);
        editor.set_filled(true);
        assert!(editor.layers[0].contains_point(vec2(25.0, 25.0)));
        assert_eq!(editor.layers[0].id, id);
    }

    #[test]
    fn unchanged_settings_preserve_undo_and_redo() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Rectangle,
            points: vec![Vec2::ZERO, vec2(50.0, 50.0)],
            ..Default::default()
        });
        editor.commit_draft();
        editor.set_color(Color(0xff0000));
        editor.undo_change();
        editor.select_layer(0);
        let undo = editor.undo.clone();
        let redo = editor.redo.clone();

        editor.set_color(Color::WHITE);
        editor.set_variant(Tool::Rectangle);
        editor.set_filled(false);
        editor.set_blur_mode(BlurMode::Pixelated);
        editor.set_intensity(12.0);
        editor.set_width(3.0);

        assert_eq!(editor.undo, undo);
        assert_eq!(editor.redo, redo);
        editor.redo_change();
        assert_eq!(editor.layers[0].color, Color(0xff0000));
    }

    #[test]
    fn clamped_settings_do_not_add_undo_entries() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Text,
            text: "Text".into(),
            width: 144.0,
            ..Default::default()
        });
        editor.commit_draft();

        editor.set_text_size(146.0);
        editor.set_width(4.0);

        assert_eq!(editor.undo.len(), 1);
        editor.undo_change();
        assert!(editor.layers.is_empty());
    }

    #[test]
    fn layer_order_changes_keep_selection_on_the_same_layer() {
        let mut editor = Editor::new();
        for tool in [Tool::Rectangle, Tool::Arrow] {
            editor.draft = Some(Annotation {
                tool,
                points: vec![Vec2::ZERO, vec2(50.0, 50.0)],
                ..Default::default()
            });
            editor.commit_draft();
        }
        let id = editor.layers[1].id;
        editor.move_layer(id, editor.layers[0].id, false);
        assert_eq!(editor.selected, Some(0));
        assert_eq!(editor.layers[0].id, id);
        editor.undo_change();
        assert_eq!(editor.layers[1].id, id);
    }

    #[test]
    fn dropping_layers_before_and_after_targets_preserves_order_and_undo() {
        let mut editor = Editor::new();
        for _ in 0..4 {
            editor.draft = Some(Annotation {
                tool: Tool::Rectangle,
                points: vec![Vec2::ZERO, vec2(50.0, 50.0)],
                ..Default::default()
            });
            editor.commit_draft();
        }
        let ids: Vec<_> = editor.layers.iter().map(|layer| layer.id).collect();
        editor.move_layer(ids[0], ids[2], true);
        assert_eq!(
            editor
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            vec![ids[1], ids[2], ids[0], ids[3]]
        );
        assert_eq!(editor.selected, Some(2));
        editor.move_layer(ids[3], ids[1], false);
        assert_eq!(
            editor
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            vec![ids[3], ids[1], ids[2], ids[0]]
        );
        assert_eq!(editor.selected, Some(0));
        let history = editor.undo.len();
        editor.move_layer(ids[3], ids[1], false);
        editor.move_layer(ids[3], ids[3], true);
        editor.move_layer(999, ids[3], true);
        assert_eq!(editor.undo.len(), history);
        editor.undo_change();
        assert_eq!(editor.layers[3].id, ids[3]);
        editor.redo_change();
        assert_eq!(editor.layers[0].id, ids[3]);
    }

    #[test]
    fn editing_text_preserves_layer_order_in_preview() {
        let mut editor = Editor::new();
        for tool in [Tool::Text, Tool::Rectangle] {
            editor.draft = Some(Annotation {
                tool,
                text: "Text".into(),
                points: vec![Vec2::ZERO, vec2(50.0, 50.0)],
                ..Default::default()
            });
            editor.commit_draft();
        }
        editor.start_editing_text(0);
        assert_eq!(
            editor
                .get_visible_annotations()
                .map(|annotation| annotation.tool)
                .collect::<Vec<_>>(),
            vec![Tool::Text, Tool::Rectangle]
        );
    }

    #[test]
    fn rich_text_formats_unicode_ranges_and_preserves_them_when_typing() {
        let mut annotation = Annotation {
            tool: Tool::Text,
            text: "a😀bc".into(),
            ..Default::default()
        };
        annotation.set_text_attribute(1..5, TextAttribute::Bold, true);
        assert!(annotation.get_format(1).bold);
        assert!(!annotation.get_format(2).bold);
        annotation.replace_text(
            5..6,
            "é",
            TextFormat {
                italic: true,
                ..Default::default()
            },
        );
        assert_eq!(annotation.text, "a😀éc");
        assert!(annotation.get_format(1).bold);
        assert!(annotation.get_format(2).italic);
        assert_eq!(
            annotation
                .get_format_runs(0..annotation.text.len())
                .iter()
                .map(|(size, _)| *size)
                .sum::<usize>(),
            annotation.text.len()
        );
        annotation.set_text_attribute(0..annotation.text.len(), TextAttribute::Underline, true);
        assert!(
            annotation
                .get_format_runs(0..annotation.text.len())
                .iter()
                .all(|(_, format)| format.underline)
        );
    }

    #[test]
    fn undo_redo_and_new_edits_preserve_history() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Arrow,
            points: vec![Vec2::ZERO, vec2(50.0, 20.0)],
            color: Color::WHITE,
            width: 3.0,
            text: String::new(),
            ..Default::default()
        });
        editor.commit_draft();
        editor.undo_change();
        assert!(editor.layers.is_empty());
        editor.redo_change();
        assert_eq!(editor.layers.len(), 1);
        editor.undo_change();
        editor.draft = Some(Annotation {
            tool: Tool::Text,
            points: vec![Vec2::ZERO],
            color: Color::WHITE,
            width: 24.0,
            text: "Hello".into(),
            ..Default::default()
        });
        editor.commit_draft();
        assert!(editor.redo.is_empty());
        assert_eq!(editor.layers[0].tool, Tool::Text);
    }

    #[test]
    fn empty_redo_keeps_new_and_edited_drafts() {
        let mut editor = Editor::new();
        let draft = Annotation {
            tool: Tool::Text,
            text: "Uncommitted text".into(),
            ..Default::default()
        };
        editor.draft = Some(draft.clone());
        editor.redo_change();
        assert_eq!(editor.draft, Some(draft));

        editor.commit_draft();
        editor.start_editing_text(0);
        editor.draft.as_mut().unwrap().text.push_str(" with edits");
        let draft = editor.draft.clone();
        editor.redo_change();
        assert_eq!(editor.draft, draft);
        assert_eq!(editor.editing_index, Some(0));
    }

    #[test]
    fn hit_testing_checks_shape_strokes() {
        let rectangle = Annotation {
            tool: Tool::Rectangle,
            points: vec![vec2(10.0, 10.0), vec2(100.0, 100.0)],
            color: Color::WHITE,
            width: 3.0,
            text: String::new(),
            ..Default::default()
        };
        assert!(rectangle.contains_point(vec2(50.0, 10.0)));
        assert!(!rectangle.contains_point(vec2(50.0, 50.0)));
    }

    #[test]
    fn ellipse_hit_testing_uses_pixel_distance_to_the_outline() {
        for radii in [vec2(500.0, 10.0), vec2(10.0, 500.0), Vec2::splat(40.0)] {
            let mut annotation = Annotation {
                tool: Tool::Ellipse,
                points: vec![-radii, radii],
                ..Default::default()
            };
            let major = if radii.x >= radii.y {
                vec2(radii.x, 0.0)
            } else {
                vec2(0.0, radii.y)
            };
            assert!(!annotation.contains_point(major * 1.4));
            assert!(!annotation.contains_point(major * 0.5));
            assert!(!annotation.contains_point(radii));
            for degrees in (0..360).step_by(15) {
                let point = radii * Vec2::from_angle((degrees as f32).to_radians());
                let normal = (point / (radii * radii)).normalize();
                assert!(annotation.contains_point(point));
                assert!(annotation.contains_point(point + normal * 7.0));
                assert!(!annotation.contains_point(point + normal * 8.0));
            }
            annotation.filled = true;
            assert!(annotation.contains_point(Vec2::ZERO));
            assert!(annotation.contains_point(major * 0.5));
            assert!(!annotation.contains_point(major * 1.4));
        }
    }

    #[test]
    fn empty_text_does_not_create_history() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Text,
            points: vec![Vec2::ZERO],
            color: Color::WHITE,
            width: 24.0,
            text: "  ".into(),
            ..Default::default()
        });
        editor.commit_draft();
        assert!(editor.undo.is_empty());
    }

    #[test]
    fn closed_line_strip_is_not_discarded_as_a_zero_length_line() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Line,
            points: vec![vec2(10.0, 10.0), vec2(30.0, 20.0), vec2(10.0, 10.0)],
            ..Default::default()
        });
        editor.commit_draft();
        assert_eq!(editor.layers.len(), 1);
        assert_eq!(editor.layers[0].points.len(), 3);
    }

    #[test]
    fn converted_arrows_hit_test_the_line_strip_instead_of_a_diagonal() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Line,
            points: vec![vec2(20.0, 20.0), vec2(100.0, 20.0), vec2(100.0, 100.0)],
            ..Default::default()
        });
        editor.commit_draft();
        editor.set_variant(Tool::Arrow);

        assert!(editor.layers[0].contains_point(vec2(60.0, 20.0)));
        assert!(editor.layers[0].contains_point(vec2(100.0, 60.0)));
        assert!(!editor.layers[0].contains_point(vec2(60.0, 60.0)));
        editor.undo_change();
        assert_eq!(editor.layers[0].tool, Tool::Line);
    }

    #[test]
    fn arrow_bounds_include_the_arrowhead() {
        for end in [vec2(100.0, 20.0), vec2(20.0, 100.0)] {
            let annotation = Annotation {
                tool: Tool::Arrow,
                points: vec![vec2(20.0, 20.0), end],
                ..Default::default()
            };
            let bounds = annotation.get_bounds();
            for point in get_arrow_head(&annotation.points, annotation.width) {
                assert!(bounds.contains(point));
            }
        }
    }

    #[test]
    fn converted_closed_arrows_keep_the_draft() {
        let mut editor = Editor::new();
        editor.tool = Tool::Line;
        editor.draft = Some(Annotation {
            tool: Tool::Line,
            points: vec![vec2(20.0, 20.0), vec2(100.0, 20.0), vec2(20.0, 20.0)],
            ..Default::default()
        });
        editor.set_variant(Tool::Arrow);
        editor.commit_draft();

        assert_eq!(editor.layers.len(), 1);
        assert_eq!(editor.layers[0].tool, Tool::Arrow);
        assert_eq!(editor.layers[0].points.len(), 3);
    }

    #[test]
    fn committing_unchanged_text_preserves_undo_and_redo() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Text,
            text: "Text".into(),
            ..Default::default()
        });
        editor.commit_draft();
        editor.set_color(Color(0xff0000));
        editor.undo_change();
        editor.start_editing_text(0);
        editor.commit_draft();
        assert_eq!(editor.undo.len(), 1);
        assert_eq!(editor.redo.len(), 1);
        assert_eq!(editor.selected, Some(0));
        assert_eq!(editor.editing_index, None);
        assert!(editor.draft.is_none());
        editor.redo_change();
        assert_eq!(editor.layers[0].color, Color(0xff0000));
        editor.undo_change();
        editor.undo_change();
        assert!(editor.layers.is_empty());
    }

    #[test]
    fn editing_text_can_cancel_and_undo() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Text,
            points: vec![Vec2::ZERO],
            color: Color::WHITE,
            width: 24.0,
            text: "First".into(),
            ..Default::default()
        });
        editor.commit_draft();
        editor.start_editing_text(0);
        editor.draft.as_mut().unwrap().text = "Second".into();
        assert_eq!(editor.get_visible_annotations().count(), 1);
        editor.undo_change();
        assert_eq!(editor.layers[0].text, "First");
        editor.start_editing_text(0);
        editor.draft.as_mut().unwrap().text = "Second".into();
        editor.commit_draft();
        editor.undo_change();
        assert_eq!(editor.layers[0].text, "First");
        assert_eq!(get_byte_offset("a😀b", 2), 1);
        assert_eq!(get_byte_offset("a😀b", 3), 5);
    }
}
