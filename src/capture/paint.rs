use crate::{
    editor::{ANNOTATION_FONT, Annotation, HIGHLIGHTER_OPACITY, LINE_HEIGHT, Tool, get_arrow_head},
    geometry::{self, Rect},
    ui,
};
use anyhow::Result;
use glam::{Vec2, vec2};
use gpui_kit::*;
use std::{f32::consts::TAU, ops::Range};

const HIGHLIGHTER_CIRCLE_SEGMENTS: usize = 16;

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

/// Fills the union of `rects` once with rounded corners, both outer and inner, so overlaps
/// neither show seams nor blend twice. With `outer`, the rects are cut out of it instead.
pub fn paint_rounded_union(
    rects: &[Rect],
    outer: Option<Rect>,
    radius: f32,
    color: impl Into<Background>,
    window: &mut Window,
) {
    if rects.is_empty() && outer.is_none() {
        return;
    }
    let at = |point: Vec2| gpui_kit::point(px(point.x), px(point.y));
    let mut path = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::even_odd()));
    if let Some(outer) = outer {
        path.add_polygon(
            &[
                outer.pos,
                vec2(outer.end().x, outer.pos.y),
                outer.end(),
                vec2(outer.pos.x, outer.end().y),
            ]
            .map(at),
            true,
        );
    }
    for contour in geometry::get_union_contours(rects) {
        let count = contour.len();
        for index in 0..count {
            let point = contour[index];
            let previous = contour[(index + count - 1) % count];
            let next = contour[(index + 1) % count];
            let radius = radius
                .min(point.distance(previous) / 2.0)
                .min(point.distance(next) / 2.0);
            let start = point + (previous - point).normalize_or_zero() * radius;
            let end = point + (next - point).normalize_or_zero() * radius;
            if index == 0 {
                path.move_to(at(start));
            } else {
                path.line_to(at(start));
            }
            path.curve_to(at(end), at(point));
        }
        path.close();
    }
    match path.build() {
        Ok(path) => window.paint_path(path, color),
        Err(error) => eprintln!("could not build rounded union: {error:#}"),
    }
}

/// GPUI blends each stroke triangle separately, so a translucent stroke darkens wherever its
/// joins overlap. A non-zero fill merges the overlapping pieces into one coverage area.
fn build_highlighter_path(annotation: &Annotation) -> Result<Option<Path<Pixels>>> {
    let radius = annotation.width / 2.0;
    let mut points: Vec<Vec2> = Vec::new();
    for point in &annotation.points {
        if points
            .last()
            .is_none_or(|last| last.distance(*point) >= radius / 2.0)
        {
            points.push(*point);
        }
    }
    let end = *annotation.points.last().unwrap();
    if *points.last().unwrap() != end {
        points.push(end);
    }
    let at = |point: Vec2| gpui_kit::point(px(point.x), px(point.y));
    let mut path = PathBuilder::fill().with_style(PathStyle::Fill(FillOptions::non_zero()));
    for pair in points.windows(2) {
        let normal = (pair[1] - pair[0]).normalize_or_zero().perp() * radius;
        if normal == Vec2::ZERO {
            continue;
        }
        path.move_to(at(pair[0] + normal));
        path.line_to(at(pair[1] + normal));
        path.line_to(at(pair[1] - normal));
        path.line_to(at(pair[0] - normal));
        path.close();
    }
    for point in &points {
        path.move_to(at(*point + vec2(radius, 0.0)));
        for step in 1..HIGHLIGHTER_CIRCLE_SEGMENTS {
            let angle = -(step as f32) * TAU / HIGHLIGHTER_CIRCLE_SEGMENTS as f32;
            path.line_to(at(*point + Vec2::from_angle(angle) * radius));
        }
        path.close();
    }
    return Ok(Some(path.build()?));
}

fn build_annotation_path(annotation: &Annotation) -> Result<Option<Path<Pixels>>> {
    if annotation.tool == Tool::Highlighter {
        return build_highlighter_path(annotation);
    }
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
        Tool::Arrow | Tool::Line | Tool::Pen => {
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
    if annotation.tool == Tool::Pen && annotation.points.len() == 1 {
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

    fn get_triangle_area(path: &gpui_kit::Path<gpui_kit::Pixels>) -> f32 {
        return path
            .vertices
            .chunks(3)
            .map(|triangle| {
                let [a, b, c] = [0, 1, 2].map(|index| {
                    let point = triangle[index].xy_position;
                    vec2(point.x.into(), point.y.into())
                });
                (b - a).perp_dot(c - a).abs() / 2.0
            })
            .sum();
    }

    #[test]
    fn highlighter_strokes_do_not_overlap_themselves() {
        let scribble = Annotation {
            tool: Tool::Highlighter,
            width: 20.0,
            points: (0..200)
                .map(|index| vec2(50.0 + (index as f32).sin(), 50.0 + (index as f32).cos()))
                .collect(),
            ..Default::default()
        };
        let path = build_annotation_path(&scribble).unwrap().unwrap();
        let disc = std::f32::consts::PI * 11.0 * 11.0;
        assert!(
            get_triangle_area(&path) < disc,
            "{}",
            get_triangle_area(&path)
        );

        let line = Annotation {
            points: (0..=100)
                .map(|index| vec2(index as f32 * 10.0, 0.0))
                .collect(),
            ..scribble.clone()
        };
        let area = get_triangle_area(&build_annotation_path(&line).unwrap().unwrap());
        let expected = 1000.0 * 20.0 + std::f32::consts::PI * 100.0;
        assert!((area - expected).abs() < expected * 0.02, "{area}");

        let long = Annotation {
            width: 64.0,
            points: (0..5000)
                .map(|index| {
                    let t = index as f32 * 0.5;
                    vec2(t, 200.0 * (t / 40.0).sin())
                })
                .collect(),
            ..scribble
        };
        assert!(build_annotation_path(&long).unwrap().is_some());
        let dot = Annotation {
            points: vec![vec2(10.0, 10.0)],
            ..Default::default()
        };
        let dot = Annotation {
            tool: Tool::Highlighter,
            width: 20.0,
            ..dot
        };
        assert!(build_annotation_path(&dot).unwrap().is_some());
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
