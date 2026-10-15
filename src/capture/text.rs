use crate::{
    capture::{CaptureView, paint},
    editor::{Annotation, LINE_HEIGHT, TextAttribute, TextFormat, Tool, get_byte_offset},
    geometry::Rect,
};
use gpui_kit::*;
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Selection inside the text being edited, in UTF-16 offsets as the IME expects.
#[derive(Default)]
pub struct TextCursor {
    pub range: Range<usize>,
    pub reversed: bool,
    pub marked: Option<Range<usize>>,
    pub insertion_format: Option<TextFormat>,
}

impl TextCursor {
    pub fn set(&mut self, range: Range<usize>, reversed: bool) {
        self.range = range;
        self.reversed = reversed;
        self.insertion_format = None;
    }

    pub fn get_head(&self) -> usize {
        return if self.reversed {
            self.range.start
        } else {
            self.range.end
        };
    }

    fn get_anchor(&self) -> usize {
        return if self.reversed {
            self.range.end
        } else {
            self.range.start
        };
    }
}

fn get_utf16_length(text: &str) -> usize {
    return text.encode_utf16().count();
}

fn get_previous_grapheme(text: &str, offset: usize) -> usize {
    let byte = get_byte_offset(text, offset);
    return text[..byte]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(start, _)| get_utf16_length(&text[..start]));
}

fn get_next_grapheme(text: &str, offset: usize) -> usize {
    let byte = get_byte_offset(text, offset);
    return offset
        + text[byte..]
            .graphemes(true)
            .next()
            .map_or(0, get_utf16_length);
}

pub fn create_clicked_text_range(text: &str, offset: usize, click_count: usize) -> Range<usize> {
    if click_count < 2 {
        return offset..offset;
    }
    let byte = get_byte_offset(text, offset);
    let Some((start, word)) = text
        .split_word_bound_indices()
        .find(|(start, word)| *start <= byte && byte < start + word.len())
        .or_else(|| text.split_word_bound_indices().next_back())
    else {
        return 0..0;
    };
    let start = get_utf16_length(&text[..start]);
    return start..start + get_utf16_length(word);
}

pub(super) fn get_annotation_bounds(annotation: &Annotation, window: &Window) -> Rect {
    if annotation.tool != Tool::Text {
        return annotation.get_bounds();
    }
    let mut width = 0.0_f32;
    let mut lines = 0;
    let mut offset = 0;
    for line in annotation.text.split('\n') {
        width = width.max(
            paint::shape_text_line(annotation, line, offset, window)
                .width
                .into(),
        );
        lines += 1;
        offset += line.len() + 1;
    }
    return Rect {
        pos: annotation.points[0] - annotation.width / 2.0,
        size: glam::vec2(width, lines as f32 * annotation.width * LINE_HEIGHT) + annotation.width,
    };
}

impl CaptureView {
    pub(super) fn get_text_draft(&self) -> Option<&Annotation> {
        return self
            .editor
            .draft
            .as_ref()
            .filter(|draft| draft.tool == Tool::Text);
    }

    pub(super) fn handle_text_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        let text = self.get_text_draft().unwrap().text.clone();
        if modifiers.platform {
            match key {
                "b" => self.toggle_text_attribute(TextAttribute::Bold),
                "i" => self.toggle_text_attribute(TextAttribute::Italic),
                "u" => self.toggle_text_attribute(TextAttribute::Underline),
                "a" => self.text.set(0..get_utf16_length(&text), false),
                "c" | "x" if !self.text.range.is_empty() => {
                    let start = get_byte_offset(&text, self.text.range.start);
                    let end = get_byte_offset(&text, self.text.range.end);
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
                _ => match super::get_command_action(key, modifiers.shift) {
                    Some(action) => self.perform_action(action, window, cx),
                    None => return,
                },
            }
        } else {
            match key {
                "enter" if modifiers.shift => self.replace_text_in_range(None, "\n", window, cx),
                "enter" if self.text.marked.is_some() => return,
                "enter" => {
                    self.editor.commit_draft();
                    self.text.marked = None;
                }
                "backspace" | "delete" => {
                    let mut range = self.text.range.clone();
                    if range.is_empty() {
                        if key == "backspace" {
                            range.start = get_previous_grapheme(&text, range.start);
                        } else {
                            range.end = get_next_grapheme(&text, range.end);
                        }
                    }
                    self.replace_text_in_range(Some(range), "", window, cx);
                }
                "left" | "right" => {
                    let next = if !modifiers.shift && !self.text.range.is_empty() {
                        if key == "left" {
                            self.text.range.start
                        } else {
                            self.text.range.end
                        }
                    } else if key == "left" {
                        get_previous_grapheme(&text, self.text.get_head())
                    } else {
                        get_next_grapheme(&text, self.text.get_head())
                    };
                    if modifiers.shift {
                        let anchor = self.text.get_anchor();
                        self.text
                            .set(anchor.min(next)..anchor.max(next), next < anchor);
                    } else {
                        self.text.set(next..next, false);
                    }
                }
                "home" => self.text.set(0..0, false),
                "end" => {
                    let end = get_utf16_length(&text);
                    self.text.set(end..end, false);
                }
                _ => return,
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub(super) fn get_text_bounds(&self, offset: usize, window: &Window) -> Option<Bounds<Pixels>> {
        let draft = self.get_text_draft()?;
        let byte = get_byte_offset(&draft.text, offset);
        let prefix = &draft.text[..byte];
        let line = prefix.rsplit('\n').next().unwrap_or("");
        let shaped = paint::shape_text_line(draft, line, byte - line.len(), window);
        let row = prefix.matches('\n').count() as f32;
        return Some(Bounds::new(
            point(
                px(draft.points[0].x) + shaped.width,
                px(draft.points[0].y + row * draft.width * LINE_HEIGHT),
            ),
            size(px(1.0), px(draft.width * LINE_HEIGHT)),
        ));
    }

    pub(super) fn get_text_highlights(&self, window: &Window) -> Vec<Bounds<Pixels>> {
        let Some(draft) = self.get_text_draft() else {
            return Vec::new();
        };
        let range = &self.text.range;
        if range.is_empty() {
            return Vec::new();
        }
        let mut highlights = Vec::new();
        let mut offset = 0;
        for line in draft.text.split('\n') {
            let end = offset + get_utf16_length(line);
            if range.start <= end
                && range.end > offset
                && let (Some(start), Some(finish)) = (
                    self.get_text_bounds(range.start.max(offset), window),
                    self.get_text_bounds(range.end.min(end), window),
                )
            {
                highlights.push(Bounds::new(
                    start.origin,
                    size(
                        (finish.origin.x - start.origin.x).max(px(1.0)),
                        start.size.height,
                    ),
                ));
            }
            offset = end + 1;
        }
        return highlights;
    }

    pub(super) fn get_current_text_format(&self) -> TextFormat {
        let Some(annotation) = self.editor.get_current_annotation() else {
            return self.editor.text_format;
        };
        let editing = self.editor.draft.is_some();
        if editing && self.text.range.is_empty() {
            if let Some(format) = self.text.insertion_format {
                return format;
            }
            let byte = get_byte_offset(&annotation.text, self.text.range.start);
            return annotation
                .get_format(annotation.text[..byte].chars().count().saturating_sub(1));
        }
        let range = if editing {
            get_byte_offset(&annotation.text, self.text.range.start)
                ..get_byte_offset(&annotation.text, self.text.range.end)
        } else {
            0..annotation.text.len()
        };
        let runs = annotation.get_format_runs(range);
        if runs.is_empty() {
            return annotation.text_format;
        }
        return TextFormat {
            bold: runs.iter().all(|(_, format)| format.bold),
            italic: runs.iter().all(|(_, format)| format.italic),
            underline: runs.iter().all(|(_, format)| format.underline),
        };
    }

    pub(super) fn toggle_text_attribute(&mut self, attribute: TextAttribute) {
        let mut format = self.get_current_text_format();
        let enabled = !format.get_attribute(attribute);
        format.set_attribute(attribute, enabled);
        self.editor.text_format = format;
        let range = self.text.range.clone();
        let Some(draft) = self
            .editor
            .draft
            .as_mut()
            .filter(|draft| draft.tool == Tool::Text)
        else {
            self.editor
                .update_current_annotation("text-format", |annotation| {
                    if annotation.tool == Tool::Text {
                        annotation.set_text_attribute(0..annotation.text.len(), attribute, enabled);
                    }
                });
            return;
        };
        if range.is_empty() {
            self.text.insertion_format = Some(format);
            if draft.text.is_empty() {
                draft.text_format = format;
            }
            return;
        }
        let start = get_byte_offset(&draft.text, range.start);
        let end = get_byte_offset(&draft.text, range.end);
        draft.set_text_attribute(start..end, attribute, enabled);
        self.text.insertion_format = None;
    }
}

impl EntityInputHandler for CaptureView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = &self.get_text_draft()?.text;
        let start = get_byte_offset(text, range.start);
        let end = get_byte_offset(text, range.end).max(start);
        *adjusted = Some(get_utf16_length(&text[..start])..get_utf16_length(&text[..end]));
        return Some(text[start..end].into());
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        return self.get_text_draft().map(|_| UTF16Selection {
            range: self.text.range.clone(),
            reversed: self.text.reversed,
        });
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        return self.text.marked.clone();
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.text.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let format = self.get_current_text_format();
        let range = range
            .or(self.text.marked.take())
            .unwrap_or(self.text.range.clone());
        let Some(draft) = self
            .editor
            .draft
            .as_mut()
            .filter(|draft| draft.tool == Tool::Text)
        else {
            return;
        };
        let start = get_byte_offset(&draft.text, range.start);
        let end = get_byte_offset(&draft.text, range.end).max(start);
        let cursor = get_utf16_length(&draft.text[..start]) + get_utf16_length(text);
        draft.replace_text(start..end, text, format);
        self.text.set(cursor..cursor, false);
        self.text.insertion_format = Some(format);
        self.text.marked = None;
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
        let length = get_utf16_length(text);
        let end = self.text.range.end;
        let start = end.saturating_sub(length);
        self.text.marked = (!text.is_empty()).then_some(start..end);
        if let Some(selected) = selected {
            self.text.range = start + selected.start.min(length)..start + selected.end.min(length);
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
        let draft = self.get_text_draft()?;
        let y: f32 = point.y.into();
        let lines = draft.text.split('\n').collect::<Vec<_>>();
        let row = (((y - draft.points[0].y) / (draft.width * LINE_HEIGHT))
            .floor()
            .max(0.0) as usize)
            .min(lines.len() - 1);
        let mut offset = 0;
        let mut byte_offset = 0;
        for line in &lines[..row] {
            offset += get_utf16_length(line) + 1;
            byte_offset += line.len() + 1;
        }
        let line = lines[row];
        let shaped = paint::shape_text_line(draft, line, byte_offset, window);
        let byte = shaped.closest_index_for_x(point.x - px(draft.points[0].x));
        return Some(offset + get_utf16_length(&line[..byte]));
    }
}

#[cfg(test)]
mod tests {
    use super::{create_clicked_text_range, get_next_grapheme, get_previous_grapheme};

    #[test]
    fn double_click_selects_unicode_words() {
        assert_eq!(create_clicked_text_range("😀 café world", 5, 2), 3..7);
        assert_eq!(create_clicked_text_range("😀 café world", 5, 1), 5..5);
        assert_eq!(create_clicked_text_range("", 0, 2), 0..0);
        assert_eq!(create_clicked_text_range("word", 4, 2), 0..4);
    }

    #[test]
    fn caret_steps_over_whole_graphemes() {
        let text = "a👍🏽b";
        assert_eq!(get_next_grapheme(text, 1), 5);
        assert_eq!(get_previous_grapheme(text, 5), 1);
        assert_eq!(get_previous_grapheme(text, 0), 0);
        assert_eq!(get_next_grapheme(text, 6), 6);
    }
}
