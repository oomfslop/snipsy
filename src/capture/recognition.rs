use crate::{
    capture::{CaptureView, Drag, DragMode, layout},
    editor::Tool,
    geometry::Rect,
    platform::{self, RecognizedCode, RecognizedLine},
    ui,
};
use anyhow::anyhow;
use glam::{Vec2, vec2};
use gpui_kit::{
    assets::IconName,
    component::{
        Sizable as _,
        button::{Button, ButtonVariants as _},
    },
    prelude::*,
    *,
};
use unicode_segmentation::UnicodeSegmentation;

const BOX_PADDING: f32 = 2.0;
pub const CORNER_RADIUS: f32 = 2.0;
const CODE_CHIP_WIDTH: f32 = 320.0;
const CODE_CHIP_HEIGHT: f32 = 28.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct TextPosition {
    line: usize,
    offset: usize,
}

/// Lines and codes are in overlay coordinates and only valid for the selection they were
/// recognized in.
pub struct RecognizedText {
    selection: Rect,
    lines: Vec<RecognizedLine>,
    codes: Vec<RecognizedCode>,
    range: (TextPosition, TextPosition),
}

#[derive(Default)]
pub struct TextRecognition {
    result: Option<RecognizedText>,
    task: Option<(Rect, Task<()>)>,
}

impl RecognizedText {
    fn get_position(&self, point: Vec2) -> Option<TextPosition> {
        let get_distance = |rect: &Rect| {
            (rect.pos - point)
                .max(point - rect.end())
                .max(Vec2::ZERO)
                .length()
        };
        let (line, nearest) =
            self.lines.iter().enumerate().min_by(|(_, a), (_, b)| {
                get_distance(&a.bounds).total_cmp(&get_distance(&b.bounds))
            })?;
        let offset = nearest
            .glyphs
            .iter()
            .find(|(_, glyph)| glyph.pos.x + glyph.size.x / 2.0 > point.x)
            .map_or(nearest.text.len(), |(offset, _)| *offset);
        return Some(TextPosition { line, offset });
    }

    fn get_clicked_range(
        &self,
        position: TextPosition,
        click_count: usize,
    ) -> (TextPosition, TextPosition) {
        let text = &self.lines[position.line].text;
        let at = |offset| TextPosition {
            line: position.line,
            offset,
        };
        return match click_count {
            0 | 1 => (position, position),
            2 => text
                .split_word_bound_indices()
                .find(|(start, word)| position.offset < start + word.len())
                .map_or((at(text.len()), at(text.len())), |(start, word)| {
                    (at(start), at(start + word.len()))
                }),
            _ => (at(0), at(text.len())),
        };
    }

    fn get_line_range(&self, line: usize) -> (usize, usize) {
        let (start, end) = self.range;
        let text = &self.lines[line].text;
        return (
            if line == start.line { start.offset } else { 0 },
            if line == end.line {
                end.offset
            } else {
                text.len()
            },
        );
    }

    fn get_selected_text(&self) -> String {
        let (start, end) = self.range;
        if start == end && self.lines.is_empty() {
            return self
                .codes
                .iter()
                .map(|code| code.payload.as_str())
                .collect::<Vec<_>>()
                .join("\n");
        }
        if start == end {
            return self
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
        }
        return (start.line..=end.line)
            .map(|line| {
                let (from, to) = self.get_line_range(line);
                &self.lines[line].text[from..to]
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    fn get_links(&self) -> Vec<(Rect, &str)> {
        return self
            .lines
            .iter()
            .flat_map(|line| {
                line.links.iter().filter_map(|(range, url)| {
                    let rect = line
                        .glyphs
                        .iter()
                        .filter(|(offset, _)| range.contains(offset))
                        .map(|(_, glyph)| *glyph)
                        .reduce(Rect::union)?;
                    Some((rect, url.as_str()))
                })
            })
            .collect();
    }

    fn get_highlights(&self) -> Vec<Rect> {
        let (start, end) = self.range;
        if start == end {
            return Vec::new();
        }
        return (start.line..=end.line)
            .filter_map(|line| {
                let (from, to) = self.get_line_range(line);
                self.lines[line]
                    .glyphs
                    .iter()
                    .filter(|(offset, _)| (from..to).contains(offset))
                    .map(|(_, glyph)| *glyph)
                    .reduce(Rect::union)
            })
            .collect();
    }
}

pub struct RecognitionOverlay {
    pub boxes: Vec<Rect>,
    pub codes: Vec<Rect>,
    pub links: Vec<Rect>,
    pub highlights: Vec<Rect>,
}

fn get_padded_rect(rect: Rect) -> Rect {
    return Rect {
        pos: rect.pos - BOX_PADDING,
        size: rect.size + BOX_PADDING * 2.0,
    };
}

impl CaptureView {
    fn get_recognized_text(&self) -> Option<&RecognizedText> {
        return self.recognition.result.as_ref().filter(|result| {
            self.editor.tool == Tool::RecognizeText && Some(result.selection) == self.selection
        });
    }

    pub(super) fn get_recognition_status(&self) -> &'static str {
        return match self.get_recognized_text() {
            None => "Recognizing text…",
            Some(result) if !result.lines.is_empty() => "Drag to select text",
            Some(_) => "No text found",
        };
    }

    pub(super) fn get_recognition_overlay(&self) -> Option<RecognitionOverlay> {
        let result = self.get_recognized_text()?;
        return Some(RecognitionOverlay {
            boxes: result
                .lines
                .iter()
                .map(|line| line.bounds)
                .chain(result.codes.iter().map(|code| code.bounds))
                .map(get_padded_rect)
                .collect(),
            codes: result
                .codes
                .iter()
                .map(|code| get_padded_rect(code.bounds))
                .collect(),
            links: result
                .get_links()
                .into_iter()
                .map(|(rect, _)| get_padded_rect(rect))
                .collect(),
            highlights: result.get_highlights(),
        });
    }

    pub(super) fn update_text_recognition(&mut self, cx: &mut Context<Self>) {
        let Some(selection) = self
            .selection
            .filter(|selection| layout::get_is_valid_selection(*selection))
        else {
            return;
        };
        if self.editor.tool != Tool::RecognizeText
            || self.drag.is_some()
            || self
                .recognition
                .result
                .as_ref()
                .is_some_and(|result| result.selection == selection)
            || self
                .recognition
                .task
                .as_ref()
                .is_some_and(|(pending, _)| *pending == selection)
        {
            return;
        }
        let Some(screenshot) = &self.screenshot else {
            return;
        };
        let image = screenshot.image.clone();
        let canvas = self.get_canvas();
        let task = cx.spawn(async move |this, cx| {
            let recognized = cx
                .background_executor()
                .spawn(async move {
                    let scale = image.width() as f32 / canvas.size.x;
                    let size = vec2(image.width() as f32, image.height() as f32);
                    let start = ((selection.pos - canvas.pos) * scale)
                        .round()
                        .clamp(Vec2::ZERO, size);
                    let end = ((selection.end() - canvas.pos) * scale)
                        .round()
                        .clamp(start, size);
                    let crop = image::imageops::crop_imm(
                        &*image,
                        start.x as u32,
                        start.y as u32,
                        (end.x - start.x) as u32,
                        (end.y - start.y) as u32,
                    )
                    .to_image();
                    if crop.width() == 0 || crop.height() == 0 {
                        return Err(anyhow!("Selection is empty"));
                    }
                    let to_overlay = |rect: Rect| Rect {
                        pos: canvas.pos + (start + rect.pos) / scale,
                        size: rect.size / scale,
                    };
                    let mut recognized = platform::recognize_image(crop)?;
                    for line in &mut recognized.lines {
                        line.bounds = to_overlay(line.bounds);
                        for (_, glyph) in &mut line.glyphs {
                            *glyph = to_overlay(*glyph);
                        }
                    }
                    for code in &mut recognized.codes {
                        code.bounds = to_overlay(code.bounds);
                    }
                    return Ok(recognized);
                })
                .await;
            let _ = this.update(cx, |capture, cx| {
                capture.recognition.task = None;
                match recognized {
                    Ok(recognized) => {
                        capture.recognition.result = Some(RecognizedText {
                            selection,
                            lines: recognized.lines,
                            codes: recognized.codes,
                            range: Default::default(),
                        });
                    }
                    Err(error) => {
                        capture.error = Some(format!("{error:#}").into());
                        capture.recognition.result = Some(RecognizedText {
                            selection,
                            lines: Vec::new(),
                            codes: Vec::new(),
                            range: Default::default(),
                        });
                    }
                }
                cx.notify();
            });
        });
        self.recognition.task = Some((selection, task));
    }

    /// Returns false when there is no recognized text to select.
    pub(super) fn begin_recognized_text_selection(&mut self, click_count: usize) -> bool {
        let pointer = self.pointer;
        let Some(result) = self.get_recognized_text() else {
            return false;
        };
        let Some(position) = result.get_position(pointer) else {
            return false;
        };
        let (start, end) = result.get_clicked_range(position, click_count);
        self.recognition.result.as_mut().unwrap().range = (start, end);
        self.drag = Some(Drag::new(
            DragMode::SelectRecognizedText(start, end),
            pointer,
        ));
        return true;
    }

    pub(super) fn update_recognized_text_selection(
        &mut self,
        start: TextPosition,
        end: TextPosition,
    ) {
        let pointer = self.pointer;
        let Some(head) = self
            .get_recognized_text()
            .and_then(|result| result.get_position(pointer))
        else {
            return;
        };
        self.recognition.result.as_mut().unwrap().range = (start.min(head), end.max(head));
    }

    pub(super) fn get_hovered_link(&self) -> Option<String> {
        return self.get_recognized_text().and_then(|result| {
            result
                .get_links()
                .into_iter()
                .find(|(rect, _)| get_padded_rect(*rect).contains(self.pointer))
                .map(|(_, url)| url.to_string())
        });
    }

    pub(super) fn open_recognized_link(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(url) = self.get_hovered_link() else {
            return false;
        };
        cx.open_url(&url);
        self.dismiss(window, cx);
        return true;
    }

    pub(super) fn select_all_recognized_text(&mut self) -> bool {
        let Some(result) = self.get_recognized_text() else {
            return false;
        };
        let Some(last) = result.lines.len().checked_sub(1) else {
            return false;
        };
        let end = TextPosition {
            line: last,
            offset: result.lines[last].text.len(),
        };
        self.recognition.result.as_mut().unwrap().range = (TextPosition::default(), end);
        return true;
    }

    /// Copies the selected text, or all of it when nothing is selected, or the codes when there
    /// is no text, and closes the overlay.
    pub(super) fn copy_recognized_text(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(result) = self.get_recognized_text() else {
            self.error = Some("Text is still being recognized".into());
            return false;
        };
        if result.lines.is_empty() && result.codes.is_empty() {
            self.error = Some("No text or codes found".into());
            return false;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(result.get_selected_text()));
        self.dismiss(window, cx);
        return true;
    }

    pub(super) fn render_recognized_codes(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(result) = self.get_recognized_text() else {
            return Vec::new();
        };
        return result
            .codes
            .iter()
            .enumerate()
            .map(|(index, code)| {
                let bounds = get_padded_rect(code.bounds);
                let screen = self.get_layout_screen(bounds);
                let below = bounds.end().y + layout::GAP;
                let y = if below + CODE_CHIP_HEIGHT <= screen.end().y - layout::MARGIN {
                    below
                } else {
                    bounds.pos.y - layout::GAP - CODE_CHIP_HEIGHT
                };
                let x = bounds
                    .pos
                    .x
                    .min(screen.end().x - CODE_CHIP_WIDTH - layout::MARGIN)
                    .max(screen.pos.x + layout::MARGIN);
                let payload = code.payload.clone();
                let is_link = payload.starts_with("https://") || payload.starts_with("http://");
                ui::panel(cx)
                    .id(("recognized-code", index))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .h(px(CODE_CHIP_HEIGHT))
                    .max_w(px(CODE_CHIP_WIDTH))
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_2()
                    .pr_1()
                    .text_sm()
                    .cursor(CursorStyle::Arrow)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(div().min_w_0().truncate().child(payload.replace('\n', " ")))
                    .child(
                        Button::new("copy")
                            .icon(IconName::Copy)
                            .tooltip("Copy")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener({
                                let payload = payload.clone();
                                move |capture, _, window, cx| {
                                    cx.stop_propagation();
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        payload.clone(),
                                    ));
                                    capture.dismiss(window, cx);
                                }
                            })),
                    )
                    .when(is_link, |chip| {
                        chip.child(
                            Button::new("open")
                                .icon(IconName::ExternalLink)
                                .tooltip("Open link")
                                .ghost()
                                .xsmall()
                                .on_click(cx.listener(move |capture, _, window, cx| {
                                    cx.stop_propagation();
                                    cx.open_url(&payload);
                                    capture.dismiss(window, cx);
                                })),
                        )
                    })
                    .into_any_element()
            })
            .collect();
    }
}
