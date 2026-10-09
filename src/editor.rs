use crate::geometry::{Color, Rect};
use glam::{Vec2, vec2};

#[derive(Clone)]
pub struct Screenshot {
    pub image: std::sync::Arc<image::RgbaImage>,
    pub bounds: Rect,
    pub windows: Vec<Rect>,
}

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

#[derive(Clone, Copy, Debug)]
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
                    lines.len() as f32 * self.width * 1.3,
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

    pub fn replace_text(&mut self, range: std::ops::Range<usize>, text: &str, format: TextFormat) {
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
        range: std::ops::Range<usize>,
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

    pub fn get_format_runs(&self, range: std::ops::Range<usize>) -> Vec<(usize, TextFormat)> {
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
            return self.get_bounds().intersects(point);
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
                let relative = (point - (start + end) / 2.0) / radii;
                return if self.filled {
                    relative.length() <= 1.0
                } else {
                    (relative.length() - 1.0).abs() * radii.min_element() <= tolerance
                };
            }
            Tool::Arrow => {
                let [left, right] = get_arrow_head(start, end, self.width);
                return [(start, end), (left, end), (right, end)]
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
            && !(draft.tool == Tool::Line && draft.points.len() > 2)
            && draft.points[0].distance(*draft.points.last().unwrap()) < 1.0
        {
            return;
        }
        self.record_change();
        let index = if let Some(index) = self.editing_index.take() {
            self.layers[index].annotation = draft;
            index
        } else {
            let index = self.layers.len();
            self.layers.push(Layer {
                id: self.next_layer_id,
                annotation: draft,
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
        self.draft = None;
        self.editing_index = None;
        if let Some(next) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.layers, next));
        }
        self.selected = None;
    }

    pub fn delete_selected(&mut self) {
        if let Some(index) = self.selected.take() {
            self.record_change();
            self.layers.remove(index);
        }
    }

    pub fn get_annotation_at(&self, point: Vec2) -> Option<usize> {
        return self
            .layers
            .iter()
            .rposition(|annotation| annotation.contains_point(point));
    }

    pub fn start_editing_text(&mut self, index: usize) {
        self.commit_draft();
        self.draft = Some(self.layers[index].annotation.clone());
        self.editing_index = Some(index);
        self.selected = None;
    }

    pub fn get_visible_annotations(&self) -> impl Iterator<Item = &Annotation> {
        return self
            .layers
            .iter()
            .enumerate()
            .map(|(index, layer)| {
                if Some(index) == self.editing_index {
                    self.draft.as_ref().unwrap_or(&layer.annotation)
                } else {
                    &layer.annotation
                }
            })
            .chain(self.draft.iter().filter(|_| self.editing_index.is_none()));
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
            let id = self.layers[index].id;
            if self.settings_history != Some((id, key)) {
                self.record_change();
            }
            self.settings_history = Some((id, key));
            f(&mut self.layers[index].annotation);
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

    pub fn get_current_width(&self) -> f32 {
        return self.get_current_annotation().map_or_else(
            || match self.tool {
                Tool::Text => self.text_size,
                Tool::Highlighter => self.highlighter_width,
                _ => self.width,
            },
            |annotation| annotation.width,
        );
    }

    pub fn finish_settings_change(&mut self) {
        self.settings_history = None;
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

pub fn get_arrow_head(start: Vec2, end: Vec2, width: f32) -> [Vec2; 2] {
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
