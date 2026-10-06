use crate::geometry::{Color, Rect};
use glam::{Vec2, vec2};

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
    Pen,
    Highlighter,
    Text,
    Mosaic,
    Eraser,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub tool: Tool,
    pub points: Vec<Vec2>,
    pub color: Color,
    pub width: f32,
    pub text: String,
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

    pub fn contains_point(&self, point: Vec2) -> bool {
        if self.tool == Tool::Text || self.tool == Tool::Mosaic {
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
                return (relative.length() - 1.0).abs() * radii.min_element() <= tolerance;
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
    pub annotations: Vec<Annotation>,
    pub draft: Option<Annotation>,
    pub editing_index: Option<usize>,
    pub selected: Option<usize>,
    pub undo: Vec<Vec<Annotation>>,
    pub redo: Vec<Vec<Annotation>>,
}

impl Editor {
    pub fn new() -> Self {
        return Self {
            tool: Tool::Select,
            color: Color::from_rgb(255, 75, 75),
            width: 3.0,
            annotations: Vec::new(),
            draft: None,
            editing_index: None,
            selected: None,
            undo: Vec::new(),
            redo: Vec::new(),
        };
    }

    pub fn record_change(&mut self) {
        self.undo.push(self.annotations.clone());
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
                self.annotations.remove(index);
            }
            return;
        }
        if draft.tool != Tool::Text
            && draft.tool != Tool::Pen
            && draft.tool != Tool::Highlighter
            && draft.points[0].distance(*draft.points.last().unwrap()) < 1.0
        {
            return;
        }
        self.record_change();
        if let Some(index) = self.editing_index.take() {
            self.annotations[index] = draft;
        } else {
            self.annotations.push(draft);
        }
        self.selected = None;
    }

    pub fn select_tool(&mut self, tool: Tool) {
        self.commit_draft();
        self.tool = tool;
        self.selected = None;
    }

    pub fn undo_change(&mut self) {
        if self.draft.take().is_some() {
            self.editing_index = None;
            return;
        }
        if let Some(previous) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.annotations, previous));
        }
        self.selected = None;
    }

    pub fn redo_change(&mut self) {
        self.draft = None;
        self.editing_index = None;
        if let Some(next) = self.redo.pop() {
            self.undo
                .push(std::mem::replace(&mut self.annotations, next));
        }
        self.selected = None;
    }

    pub fn delete_selected(&mut self) {
        if let Some(index) = self.selected.take() {
            self.record_change();
            self.annotations.remove(index);
        }
    }

    pub fn get_annotation_at(&self, point: Vec2) -> Option<usize> {
        return self
            .annotations
            .iter()
            .rposition(|annotation| annotation.contains_point(point));
    }

    pub fn start_editing_text(&mut self, index: usize) {
        self.commit_draft();
        self.draft = Some(self.annotations[index].clone());
        self.editing_index = Some(index);
        self.selected = None;
    }

    pub fn get_visible_annotations(&self) -> impl Iterator<Item = &Annotation> {
        return self
            .annotations
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != self.editing_index)
            .map(|(_, annotation)| annotation)
            .chain(self.draft.iter());
    }

    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(1.0, 12.0);
        if let Some(index) = self.selected {
            self.record_change();
            let annotation = &mut self.annotations[index];
            annotation.width = match annotation.tool {
                Tool::Text => self.width * 4.0 + 12.0,
                Tool::Highlighter => self.width * 6.0,
                _ => self.width,
            };
        }
        if let Some(draft) = &mut self.draft {
            draft.width = match draft.tool {
                Tool::Text => self.width * 4.0 + 12.0,
                Tool::Highlighter => self.width * 6.0,
                _ => self.width,
            };
        }
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
    fn undo_redo_and_new_edits_preserve_history() {
        let mut editor = Editor::new();
        editor.draft = Some(Annotation {
            tool: Tool::Arrow,
            points: vec![Vec2::ZERO, vec2(50.0, 20.0)],
            color: Color::WHITE,
            width: 3.0,
            text: String::new(),
        });
        editor.commit_draft();
        editor.undo_change();
        assert!(editor.annotations.is_empty());
        editor.redo_change();
        assert_eq!(editor.annotations.len(), 1);
        editor.undo_change();
        editor.draft = Some(Annotation {
            tool: Tool::Text,
            points: vec![Vec2::ZERO],
            color: Color::WHITE,
            width: 24.0,
            text: "Hello".into(),
        });
        editor.commit_draft();
        assert!(editor.redo.is_empty());
        assert_eq!(editor.annotations[0].tool, Tool::Text);
    }

    #[test]
    fn hit_testing_checks_shape_strokes() {
        let rectangle = Annotation {
            tool: Tool::Rectangle,
            points: vec![vec2(10.0, 10.0), vec2(100.0, 100.0)],
            color: Color::WHITE,
            width: 3.0,
            text: String::new(),
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
        });
        editor.commit_draft();
        assert!(editor.undo.is_empty());
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
        });
        editor.commit_draft();
        editor.start_editing_text(0);
        editor.draft.as_mut().unwrap().text = "Second".into();
        assert_eq!(editor.get_visible_annotations().count(), 1);
        editor.undo_change();
        assert_eq!(editor.annotations[0].text, "First");
        editor.start_editing_text(0);
        editor.draft.as_mut().unwrap().text = "Second".into();
        editor.commit_draft();
        editor.undo_change();
        assert_eq!(editor.annotations[0].text, "First");
        assert_eq!(get_byte_offset("a😀b", 2), 1);
        assert_eq!(get_byte_offset("a😀b", 3), 5);
    }
}
