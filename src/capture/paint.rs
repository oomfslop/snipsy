use crate::{
    editor::{ANNOTATION_FONT, Annotation, HIGHLIGHTER_OPACITY, LINE_HEIGHT, Tool, get_arrow_head},
    geometry::Rect,
    ui,
};
use anyhow::Result;
use glam::{Vec2, vec2};
use gpui_kit::*;
use std::ops::Range;

fn create_text_runs(annotation: &Annotation, range: Range<usize>) -> Vec<TextRun> {
    let color: Hsla = rgb(annotation.color.0).into();
    return annotation
        .get_format_runs(range)
        .into_iter()
        .map(|(length, format)| {
            let mut face = font(ANNOTATION_FONT);
            if format.bold {
                face = face.bold();
            }
            if format.italic {
                face = face.italic();
            }
            TextRun {
                len: length,
                font: face,
                color,
                background_color: None,
                underline: format.underline.then_some(UnderlineStyle {
                    color: Some(color),
                    thickness: px((annotation.width / 16.0).max(1.0)),
                    wavy: false,
                }),
                strikethrough: None,
            }
        })
        .collect();
}

pub fn shape_text_line(
    annotation: &Annotation,
    line: &str,
    offset: usize,
    window: &Window,
) -> ShapedLine {
    return window.text_system().shape_line(
        line.to_string().into(),
        px(annotation.width),
        &create_text_runs(annotation, offset..offset + line.len()),
        None,
    );
}

fn build_annotation_path(annotation: &Annotation) -> Result<Option<Path<Pixels>>> {
    let start = annotation.points[0];
    let end = *annotation.points.last().unwrap();
    let min = start.min(end);
    let max = start.max(end);
    let at = |point: Vec2| gpui_kit::point(px(point.x), px(point.y));
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
            let right = at(center + vec2(radius.x, 0.0));
            path.move_to(right);
            path.arc_to(
                at(radius),
                px(0.0),
                false,
                true,
                at(center - vec2(radius.x, 0.0)),
            );
            path.arc_to(at(radius), px(0.0), false, true, right);
            path.close();
        }
        Tool::Arrow if annotation.points.iter().all(|point| *point == start) => return Ok(None),
        Tool::Arrow | Tool::Line | Tool::Pen | Tool::Highlighter => {
            if annotation.points.len() < 2 {
                return Ok(None);
            }
            path.move_to(at(start));
            for point in &annotation.points[1..] {
                path.line_to(at(*point));
            }
            if annotation.tool == Tool::Arrow {
                let [left, right] = get_arrow_head(&annotation.points, annotation.width);
                path.move_to(at(left));
                path.line_to(at(end));
                path.line_to(at(right));
            }
        }
        _ => return Ok(None),
    }
    return Ok(Some(path.build()?));
}

pub fn paint_annotation(annotation: &Annotation, window: &mut Window, cx: &mut App) -> Result<()> {
    let mut color: Hsla = rgb(annotation.color.0).into();
    if annotation.tool == Tool::Highlighter {
        color.a = HIGHLIGHTER_OPACITY;
    }
    if let Some(path) = build_annotation_path(annotation)? {
        window.paint_path(path, color);
    }
    if matches!(annotation.tool, Tool::Pen | Tool::Highlighter) && annotation.points.len() == 1 {
        let radius = annotation.width / 2.0;
        let mut dot = fill(
            ui::to_bounds(Rect {
                pos: annotation.points[0] - radius,
                size: Vec2::splat(annotation.width),
            }),
            color,
        );
        dot.corner_radii = Corners::all(px(radius));
        window.paint_quad(dot);
    }
    if annotation.tool == Tool::Text {
        let height = px(annotation.width * LINE_HEIGHT);
        let mut offset = 0;
        for (index, text) in annotation.text.split('\n').enumerate() {
            let line = shape_text_line(annotation, text, offset, window);
            let baseline =
                px(annotation.points[0].y + annotation.width * (1.0 + index as f32 * LINE_HEIGHT));
            let y = baseline - line.ascent - (height - line.ascent - line.descent) / 2.0;
            line.paint(
                point(px(annotation.points[0].x), y),
                height,
                TextAlign::Left,
                None,
                window,
                cx,
            )?;
            offset += text.len() + 1;
        }
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::build_annotation_path;
    use crate::editor::{Annotation, Tool};
    use glam::vec2;

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
                ..Default::default()
            };
            assert!(build_annotation_path(&annotation).unwrap().is_some());
        }
        let collapsed = Annotation {
            tool: Tool::Arrow,
            points: vec![vec2(20.0, 20.0), vec2(20.0, 20.0)],
            ..Default::default()
        };
        assert!(build_annotation_path(&collapsed).unwrap().is_none());
    }

    #[test]
    fn converted_closed_arrows_keep_the_preview_path() {
        let annotation = Annotation {
            tool: Tool::Arrow,
            points: vec![vec2(20.0, 20.0), vec2(100.0, 20.0), vec2(20.0, 20.0)],
            ..Default::default()
        };
        assert!(build_annotation_path(&annotation).unwrap().is_some());
    }
}
