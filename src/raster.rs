use crate::{
    editor::{
        ANNOTATION_FONT, Annotation, BlurMode, HIGHLIGHTER_OPACITY, LINE_HEIGHT, Tool,
        get_arrow_head,
    },
    geometry::{Color, Rect},
};
use anyhow::{Context as _, Result, bail, ensure};
use gpui_kit::RenderImage;
use image::{Frame, Rgba, RgbaImage};
use resvg::{tiny_skia, usvg};
use std::{fmt::Write, io::Cursor, sync::Arc};
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone)]
pub struct Screenshot {
    pub image: Arc<RgbaImage>,
    pub bounds: Rect,
    pub windows: Vec<Rect>,
}

pub struct CursorSnapshot {
    pub image: RgbaImage,
    pub position: glam::Vec2,
    pub size: glam::Vec2,
}

pub fn draw_cursor(image: &mut RgbaImage, bounds: Rect, cursor: &CursorSnapshot) {
    let scale = image.width() as f32 / bounds.size.x;
    let size = (cursor.size * scale).round().max(glam::Vec2::ONE);
    let offset = ((cursor.position - bounds.pos) * scale).round();
    let resized;
    let pixels = if cursor.image.dimensions() == (size.x as u32, size.y as u32) {
        &cursor.image
    } else {
        resized = image::imageops::resize(
            &cursor.image,
            size.x as u32,
            size.y as u32,
            image::imageops::FilterType::Triangle,
        );
        &resized
    };
    image::imageops::overlay(image, pixels, offset.x as i64, offset.y as i64);
}

pub fn update_pixels(image: &mut RgbaImage, update: impl Fn(&mut [u8]) + Sync) {
    if image.is_empty() {
        return;
    }
    let threads = std::thread::available_parallelism().map_or(1, |count| count.get());
    let chunk_rows = (image.height() as usize).div_ceil(threads);
    let chunk_bytes = chunk_rows * image.width() as usize * 4;
    let update = &update;
    std::thread::scope(|scope| {
        for chunk in image.chunks_mut(chunk_bytes) {
            scope.spawn(move || chunk.chunks_exact_mut(4).for_each(update));
        }
    });
}

pub fn create_render_image(image: &RgbaImage) -> Arc<RenderImage> {
    return create_render_frames(std::slice::from_ref(&Frame::new(image.clone())));
}

pub fn create_render_frames(frames: &[Frame]) -> Arc<RenderImage> {
    let frames = frames
        .iter()
        .cloned()
        .map(|frame| {
            let delay = frame.delay();
            let left = frame.left();
            let top = frame.top();
            let mut image = frame.into_buffer();
            update_pixels(&mut image, |pixel| pixel.swap(0, 2));
            return Frame::from_parts(image, left, top, delay);
        })
        .collect::<Vec<_>>();
    return Arc::new(RenderImage::new(frames));
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>> {
    let mut output = Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png)?;
    return Ok(output.into_inner());
}

pub fn render_text_image(text: &str, fonts: &Arc<usvg::fontdb::Database>) -> Result<RgbaImage> {
    ensure!(!text.trim().is_empty(), "Clipboard text is empty");
    ensure!(text.len() <= 16_384, "Clipboard text is too large");
    // ponytail: hard-wrap at 80 graphemes, use shaped layout when text cards become editable.
    let mut wrapped = String::new();
    let mut lines = 0usize;
    let mut columns = 0usize;
    for (line_index, line) in text.split('\n').enumerate() {
        if line_index > 0 {
            wrapped.push('\n');
        }
        lines += 1;
        let mut line_columns = 0usize;
        for grapheme in line.graphemes(true) {
            if line_columns == 80 {
                wrapped.push('\n');
                lines += 1;
                line_columns = 0;
            }
            wrapped.push_str(grapheme);
            line_columns += 1;
            columns = columns.max(line_columns);
        }
    }
    ensure!(lines <= 60, "Clipboard text has too many lines");
    let font_size = 18.0;
    let padding = 16.0;
    let mut annotation = Annotation {
        tool: Tool::Text,
        points: vec![glam::vec2(padding, padding)],
        text: wrapped,
        width: font_size,
        color: Color(0x202024),
        ..Default::default()
    };
    let mut svg = "<svg xmlns='http://www.w3.org/2000/svg' width='1' height='1'>".to_string();
    append_annotation_svg(&mut svg, &annotation);
    svg.push_str("</svg>");
    let tree = usvg::Tree::from_str(
        &svg,
        &usvg::Options {
            fontdb: fonts.clone(),
            font_family: ANNOTATION_FONT.into(),
            ..Default::default()
        },
    )?;
    let text_bounds = tree.root().abs_stroke_bounding_box();
    let offset = glam::vec2(
        (padding - text_bounds.left()).max(0.0).ceil(),
        (padding - text_bounds.top()).max(0.0).ceil(),
    );
    annotation.points[0] += offset;
    let width = (columns as f32 * font_size * 0.7 + padding * 2.0)
        .max(text_bounds.right() + offset.x + padding)
        .ceil()
        .max(120.0) as u32;
    let height = (lines as f32 * font_size * LINE_HEIGHT + padding * 2.0)
        .max(text_bounds.bottom() + offset.y + padding)
        .ceil()
        .max(64.0) as u32;
    ensure!(
        u64::from(width) * u64::from(height) <= 16 * 1024 * 1024,
        "Clipboard text image is too large"
    );
    let bounds = Rect {
        pos: glam::Vec2::ZERO,
        size: glam::vec2(width as f32, height as f32),
    };
    return render_selection(
        &RgbaImage::from_pixel(width, height, Rgba([250, 250, 250, 255])),
        bounds,
        bounds,
        std::iter::once(&annotation),
        fonts,
    );
}

pub fn render_selection(
    image: &RgbaImage,
    image_bounds: Rect,
    selection: Rect,
    annotations: impl Iterator<Item = impl std::borrow::Borrow<Annotation>>,
    fonts: &Arc<usvg::fontdb::Database>,
) -> Result<RgbaImage> {
    let scale_x = image.width() as f32 / image_bounds.size.x;
    let scale_y = image.height() as f32 / image_bounds.size.y;
    let selection = Rect {
        pos: selection.pos - image_bounds.pos,
        size: selection.size,
    };
    let x = (selection.pos.x * scale_x).floor().max(0.0) as u32;
    let y = (selection.pos.y * scale_y).floor().max(0.0) as u32;
    let right = ((selection.pos.x + selection.size.x) * scale_x)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((selection.pos.y + selection.size.y) * scale_y)
        .ceil()
        .min(image.height() as f32) as u32;
    if right <= x || bottom <= y || x >= image.width() || y >= image.height() {
        bail!("Selection is outside the image");
    }
    let mut output = image::imageops::crop_imm(image, x, y, right - x, bottom - y).to_image();
    let offset = image_bounds.pos + glam::vec2(x as f32 / scale_x, y as f32 / scale_y);
    let mut svg = String::new();
    for annotation in annotations {
        let annotation = annotation.borrow();
        if annotation.tool == Tool::Mosaic {
            if !svg.is_empty() {
                composite_svg(&mut output, &svg, offset, scale_x, scale_y, fonts)?;
                svg.clear();
            }
            let (left, top, right, bottom) = get_mosaic_pixels(
                annotation,
                offset,
                glam::vec2(scale_x, scale_y),
                output.dimensions(),
            );
            let amount = get_blur_amount(annotation, scale_x);
            if annotation.blur_mode == BlurMode::Gaussian {
                if right > left && bottom > top {
                    let region =
                        image::imageops::crop_imm(&output, left, top, right - left, bottom - top)
                            .to_image();
                    let blurred = image::imageops::blur(&region, amount);
                    image::imageops::replace(&mut output, &blurred, left as i64, top as i64);
                }
                continue;
            }
            let block = amount as u32;
            for y in (top..bottom).step_by(block as usize) {
                for x in (left..right).step_by(block as usize) {
                    let mut sum = [0u64; 4];
                    let mut count = 0;
                    for py in y..(y + block).min(bottom) {
                        for px in x..(x + block).min(right) {
                            for (sum, channel) in sum.iter_mut().zip(output.get_pixel(px, py).0) {
                                *sum += channel as u64;
                            }
                            count += 1;
                        }
                    }
                    let pixel = Rgba(sum.map(|channel| (channel / count) as u8));
                    for py in y..(y + block).min(bottom) {
                        for px in x..(x + block).min(right) {
                            output.put_pixel(px, py, pixel);
                        }
                    }
                }
            }
            continue;
        }
        append_annotation_svg(&mut svg, annotation);
    }
    if !svg.is_empty() {
        composite_svg(&mut output, &svg, offset, scale_x, scale_y, fonts)?;
    }
    return Ok(output);
}

fn get_mosaic_pixels(
    annotation: &Annotation,
    offset: glam::Vec2,
    scale: glam::Vec2,
    (width, height): (u32, u32),
) -> (u32, u32, u32, u32) {
    let start = annotation.points[0].min(*annotation.points.last().unwrap()) - offset;
    let end = annotation.points[0].max(*annotation.points.last().unwrap()) - offset;
    return (
        (start.x * scale.x).max(0.0) as u32,
        (start.y * scale.y).max(0.0) as u32,
        (end.x * scale.x).ceil().clamp(0.0, width as f32) as u32,
        (end.y * scale.y).ceil().clamp(0.0, height as f32) as u32,
    );
}

fn get_blur_amount(annotation: &Annotation, scale: f32) -> f32 {
    return match annotation.blur_mode {
        BlurMode::Gaussian => (annotation.intensity * scale / 3.0).max(0.5),
        BlurMode::Pixelated => (annotation.intensity * scale).max(1.0).floor(),
    };
}

pub enum VideoEffect {
    Overlay {
        image: RgbaImage,
        time: Option<(f64, f64)>,
    },
    Blur {
        rect: Rect,
        mode: BlurMode,
        amount: f32,
        time: Option<(f64, f64)>,
    },
}

/// Mirrors `render_selection` for video frames: annotations between blurs that share a time
/// range are flattened into overlays, and blurs apply to everything composited below them.
pub fn create_video_effects<'a>(
    size: (u32, u32),
    canvas: Rect,
    annotations: impl Iterator<Item = (&'a Annotation, Option<(f64, f64)>)>,
    fonts: &Arc<usvg::fontdb::Database>,
) -> Result<Vec<VideoEffect>> {
    let scale = glam::vec2(size.0 as f32, size.1 as f32) / canvas.size;
    let mut effects = Vec::new();
    let mut group = Vec::new();
    let mut group_time = None;
    let flush =
        |group: &mut Vec<&Annotation>, time: Option<(f64, f64)>, effects: &mut Vec<VideoEffect>| {
            if group.is_empty() {
                return anyhow::Ok(());
            }
            let image = RgbaImage::new(size.0, size.1);
            let image = render_selection(&image, canvas, canvas, group.drain(..), fonts)?;
            effects.push(VideoEffect::Overlay { image, time });
            return Ok(());
        };
    for (annotation, time) in annotations {
        if annotation.tool != Tool::Mosaic {
            if time != group_time {
                flush(&mut group, group_time, &mut effects)?;
                group_time = time;
            }
            group.push(annotation);
            continue;
        }
        flush(&mut group, group_time, &mut effects)?;
        let (left, top, right, bottom) = get_mosaic_pixels(annotation, canvas.pos, scale, size);
        if right <= left || bottom <= top {
            continue;
        }
        effects.push(VideoEffect::Blur {
            rect: Rect::from_corners(
                glam::vec2(left as f32, top as f32),
                glam::vec2(right as f32, bottom as f32),
            ),
            mode: annotation.blur_mode,
            amount: get_blur_amount(annotation, scale.x),
            time,
        });
    }
    flush(&mut group, group_time, &mut effects)?;
    return Ok(effects);
}

fn composite_svg(
    output: &mut RgbaImage,
    body: &str,
    offset: glam::Vec2,
    scale_x: f32,
    scale_y: f32,
    fonts: &Arc<usvg::fontdb::Database>,
) -> Result<()> {
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}'><g transform='scale({scale_x} {scale_y}) translate({} {})'>{body}</g></svg>",
        output.width(),
        output.height(),
        -offset.x,
        -offset.y
    );
    let options = usvg::Options {
        fontdb: fonts.clone(),
        font_family: ANNOTATION_FONT.into(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&svg, &options)?;
    let mut pixmap =
        tiny_skia::Pixmap::new(output.width(), output.height()).context("Image too large")?;
    resvg::render(
        &tree,
        tiny_skia::Transform::identity(),
        &mut pixmap.as_mut(),
    );
    for (destination, source) in output.pixels_mut().zip(pixmap.pixels()) {
        let alpha = source.alpha() as u32;
        if alpha == 0 {
            continue;
        }
        let dst_alpha = destination[3] as u32;
        let out_alpha = alpha + (dst_alpha * (255 - alpha) + 127) / 255;
        for (index, channel) in [source.red(), source.green(), source.blue()]
            .into_iter()
            .enumerate()
        {
            let premultiplied = channel as u32 * 255
                + (destination[index] as u32 * dst_alpha * (255 - alpha) + 127) / 255;
            destination[index] = ((premultiplied + out_alpha / 2) / out_alpha).min(255) as u8;
        }
        destination[3] = out_alpha as u8;
    }
    return Ok(());
}

fn append_annotation_svg(svg: &mut String, annotation: &Annotation) {
    let start = annotation.points[0];
    let end = *annotation.points.last().unwrap();
    let min = start.min(end);
    let size = (end - start).abs();
    let _ = write!(
        svg,
        "<g stroke-width='{}' stroke-linecap='round' stroke-linejoin='round'",
        annotation.width
    );
    if annotation.filled && matches!(annotation.tool, Tool::Rectangle | Tool::Ellipse) {
        let _ = write!(svg, " fill='#{:06x}' stroke='none'", annotation.color.0);
    } else {
        let _ = write!(svg, " fill='none' stroke='#{:06x}'", annotation.color.0);
    }
    if annotation.tool == Tool::Highlighter {
        let _ = write!(svg, " opacity='{HIGHLIGHTER_OPACITY}'");
    }
    svg.push('>');
    match annotation.tool {
        Tool::Rectangle => {
            let _ = write!(
                svg,
                "<rect x='{}' y='{}' width='{}' height='{}'/>",
                min.x, min.y, size.x, size.y
            );
        }
        Tool::Ellipse => {
            let _ = write!(
                svg,
                "<ellipse cx='{}' cy='{}' rx='{}' ry='{}'/>",
                min.x + size.x / 2.0,
                min.y + size.y / 2.0,
                size.x / 2.0,
                size.y / 2.0
            );
        }
        Tool::Pen | Tool::Highlighter if annotation.points.len() == 1 => {
            let _ = write!(
                svg,
                "<circle cx='{}' cy='{}' r='{}' fill='#{:06x}' stroke='none'/>",
                start.x,
                start.y,
                annotation.width / 2.0,
                annotation.color.0
            );
        }
        Tool::Arrow | Tool::Line | Tool::Pen | Tool::Highlighter => {
            svg.push_str("<polyline points='");
            for point in &annotation.points {
                let _ = write!(svg, "{},{} ", point.x, point.y);
            }
            svg.push_str("'/>");
            if annotation.tool == Tool::Arrow {
                let [left, right] = get_arrow_head(&annotation.points, annotation.width);
                let _ = write!(
                    svg,
                    "<path d='M{} {}L{} {}L{} {}'/>",
                    left.x, left.y, end.x, end.y, right.x, right.y
                );
            }
        }
        Tool::Text => {
            let _ = write!(
                svg,
                "<text xml:space='preserve' fill='#{:06x}' stroke='none' font-family='{ANNOTATION_FONT}' font-size='{}'>",
                annotation.color.0, annotation.width
            );
            let mut offset = 0;
            for (index, line) in annotation.text.split('\n').enumerate() {
                let _ = write!(
                    svg,
                    "<tspan x='{}' y='{}'>",
                    start.x,
                    start.y + annotation.width * (1.0 + index as f32 * LINE_HEIGHT)
                );
                let mut byte = 0;
                for (length, format) in annotation.get_format_runs(offset..offset + line.len()) {
                    let text = &line[byte..byte + length];
                    let _ = write!(
                        svg,
                        "<tspan font-weight='{}' font-style='{}' text-decoration='{}'>{}</tspan>",
                        if format.bold { "bold" } else { "normal" },
                        if format.italic { "italic" } else { "normal" },
                        if format.underline {
                            "underline"
                        } else {
                            "none"
                        },
                        text.replace(
                            |character| {
                                matches!(
                                    character,
                                    '\0'..='\u{8}'
                                        | '\u{b}'
                                        | '\u{c}'
                                        | '\u{e}'..='\u{1f}'
                                        | '\u{fffe}'
                                        | '\u{ffff}'
                                )
                            },
                            "�",
                        )
                        .replace('&', "&amp;")
                        .replace('<', "&lt;")
                        .replace('>', "&gt;")
                        .replace('\"', "&quot;")
                        .replace('\'', "&apos;")
                    );
                    byte += length;
                }
                svg.push_str("</tspan>");
                offset += line.len() + 1;
            }
            svg.push_str("</text>");
        }
        _ => {}
    }
    svg.push_str("</g>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec2, vec2};

    fn load_system_fonts() -> Arc<usvg::fontdb::Database> {
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        return Arc::new(fonts);
    }

    #[test]
    fn rich_text_exports_independent_styles_and_escaped_content() {
        let mut annotation = Annotation {
            tool: Tool::Text,
            text: "a<&\né".into(),
            width: 24.0,
            ..Default::default()
        };
        annotation.set_text_attribute(1..3, crate::editor::TextAttribute::Bold, true);
        annotation.set_text_attribute(4..6, crate::editor::TextAttribute::Italic, true);
        annotation.set_text_attribute(4..6, crate::editor::TextAttribute::Underline, true);
        let mut svg = String::new();
        append_annotation_svg(&mut svg, &annotation);
        assert!(svg.contains(
            "font-weight='bold' font-style='normal' text-decoration='none'>&lt;&amp;</tspan>"
        ));
        assert!(svg.contains(
            "font-weight='normal' font-style='italic' text-decoration='underline'>é</tspan>"
        ));
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(120.0, 80.0),
        };
        let fonts = load_system_fonts();
        let output = render_selection(
            &RgbaImage::from_pixel(120, 80, Rgba([0, 0, 0, 255])),
            bounds,
            bounds,
            std::iter::once(&annotation),
            &fonts,
        )
        .unwrap();
        assert!(output.pixels().any(|pixel| pixel[0] > 0));
    }

    #[test]
    fn text_exports_preserve_leading_and_repeated_spaces() {
        let fonts = load_system_fonts();
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(240.0, 80.0),
        };
        let render = |text: &str| {
            let annotation = Annotation {
                tool: Tool::Text,
                text: text.into(),
                width: 24.0,
                ..Default::default()
            };
            render_selection(
                &RgbaImage::new(240, 80),
                bounds,
                bounds,
                std::iter::once(&annotation),
                &fonts,
            )
            .unwrap()
        };
        let get_left = |image: &RgbaImage| {
            image
                .enumerate_pixels()
                .filter(|(_, _, pixel)| pixel[3] != 0)
                .map(|(x, _, _)| x)
                .min()
                .unwrap()
        };
        assert!(get_left(&render("  X")) > get_left(&render("X")));
        assert_ne!(render("X X"), render("X  X"));
    }

    #[test]
    fn text_exports_replace_invalid_xml_characters() {
        let fonts = load_system_fonts();
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(400.0, 100.0),
        };
        let image = RgbaImage::new(400, 100);
        let render = |text: &str| {
            let annotation = Annotation {
                tool: Tool::Text,
                text: text.into(),
                width: 24.0,
                ..Default::default()
            };
            render_selection(&image, bounds, bounds, std::iter::once(&annotation), &fonts).unwrap()
        };
        assert_eq!(
            render("A\0\u{8}\u{b}\u{c}\u{e}\u{1f}\u{fffe}\u{ffff}<B>&\t\nC\rD"),
            render("A��������<B>&\t\nC\rD")
        );
    }

    #[test]
    fn line_strip_exports_every_vertex() {
        let annotation = Annotation {
            tool: Tool::Line,
            points: vec![vec2(1.0, 2.0), vec2(3.0, 4.0), vec2(5.0, 6.0)],
            ..Default::default()
        };
        let mut svg = String::new();
        append_annotation_svg(&mut svg, &annotation);
        assert!(svg.contains("<polyline points='1,2 3,4 5,6 '"));
    }

    #[test]
    fn converted_arrows_export_every_vertex_and_follow_the_last_segment() {
        let annotation = Annotation {
            tool: Tool::Arrow,
            points: vec![
                vec2(20.0, 20.0),
                vec2(100.0, 20.0),
                vec2(100.0, 100.0),
                vec2(100.0, 100.0),
            ],
            width: 8.0,
            ..Default::default()
        };
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(120.0, 120.0),
        };
        let output = render_selection(
            &RgbaImage::new(120, 120),
            bounds,
            bounds,
            std::iter::once(&annotation),
            &Arc::new(Default::default()),
        )
        .unwrap();

        assert!(output.get_pixel(60, 20)[3] > 0);
        assert!(output.get_pixel(100, 60)[3] > 0);
        assert_eq!(output.get_pixel(60, 60)[3], 0);
        assert!(output.get_pixel(84, 68)[3] > 0);
    }

    #[test]
    fn text_cards_wrap_unicode_and_reject_unbounded_height() {
        let image = render_text_image(&"😀".repeat(81), &load_system_fonts()).unwrap();
        assert!(image.width() >= 120);
        assert!(image.height() > 64);
        assert!(image.pixels().any(|pixel| pixel.0 != [250, 250, 250, 255]));
        assert!(render_text_image(&"line\n".repeat(61), &Arc::new(Default::default())).is_err());
    }

    #[test]
    fn text_cards_do_not_clip_wide_glyphs() {
        let fonts = load_system_fonts();
        let text = "W".repeat(80);
        let card = render_text_image(&text, &fonts).unwrap();
        let annotation = Annotation {
            tool: Tool::Text,
            points: vec![vec2(16.0, 16.0)],
            text,
            width: 18.0,
            color: Color(0x202024),
            ..Default::default()
        };
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(2048.0, 80.0),
        };
        let reference = render_selection(
            &RgbaImage::from_pixel(2048, 80, Rgba([250, 250, 250, 255])),
            bounds,
            bounds,
            std::iter::once(&annotation),
            &fonts,
        )
        .unwrap();
        let count_ink = |image: &RgbaImage| {
            image
                .pixels()
                .filter(|pixel| pixel.0 != [250, 250, 250, 255])
                .count()
        };
        assert_eq!(count_ink(&card), count_ink(&reference));
    }

    #[test]
    fn text_cards_preserve_indentation_and_blank_lines() {
        let fonts = load_system_fonts();
        let plain = render_text_image("W", &fonts).unwrap();
        let indented = render_text_image("  W", &fonts).unwrap();
        let get_left = |image: &RgbaImage| {
            image
                .enumerate_pixels()
                .filter(|(_, _, pixel)| pixel.0 != [250, 250, 250, 255])
                .map(|(x, _, _)| x)
                .min()
                .unwrap()
        };
        assert!(get_left(&indented) > get_left(&plain));
        assert!(render_text_image("W\n\n", &fonts).unwrap().height() > plain.height());
        assert!(render_text_image("\n\nW", &fonts).unwrap().height() > plain.height());
        assert!(render_text_image(" \n\t", &fonts).is_err());
    }

    #[test]
    fn animated_render_images_keep_frames_and_delays() {
        let delay = image::Delay::from_numer_denom_ms(120, 1);
        let frames = vec![
            Frame::from_parts(RgbaImage::new(2, 1), 0, 0, delay),
            Frame::from_parts(RgbaImage::new(2, 1), 0, 0, delay),
        ];
        let rendered = create_render_frames(&frames);
        assert_eq!(rendered.frame_count(), 2);
        assert_eq!(rendered.delay(1), delay);
    }

    #[test]
    fn filled_shapes_and_gaussian_strength_render_in_exports() {
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(40.0, 20.0),
        };
        let image = RgbaImage::from_fn(40, 20, |x, _| {
            Rgba([if x < 20 { 255 } else { 0 }, 0, 0, 255])
        });
        let fonts = Arc::new(usvg::fontdb::Database::new());
        let mut annotation = Annotation {
            tool: Tool::Rectangle,
            points: vec![vec2(10.0, 2.0), vec2(30.0, 18.0)],
            filled: true,
            color: Color(0x00ff00),
            ..Default::default()
        };
        let filled =
            render_selection(&image, bounds, bounds, [&annotation].into_iter(), &fonts).unwrap();
        assert_eq!(filled.get_pixel(20, 10).0, [0, 255, 0, 255]);
        annotation.tool = Tool::Mosaic;
        annotation.blur_mode = BlurMode::Gaussian;
        annotation.intensity = 3.0;
        let mild =
            render_selection(&image, bounds, bounds, [&annotation].into_iter(), &fonts).unwrap();
        annotation.intensity = 24.0;
        let strong =
            render_selection(&image, bounds, bounds, [&annotation].into_iter(), &fonts).unwrap();
        assert!(strong.get_pixel(13, 10)[0] < mild.get_pixel(13, 10)[0]);
        assert_eq!(strong.get_pixel(0, 0), image.get_pixel(0, 0));
    }

    #[test]
    fn export_crops_at_retina_scale_and_composites_annotations() {
        let image = RgbaImage::from_pixel(100, 80, Rgba([255, 255, 255, 255]));
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(50.0, 40.0),
        };
        let selection = Rect {
            pos: vec2(10.0, 10.0),
            size: vec2(20.0, 20.0),
        };
        let annotation = Annotation {
            tool: Tool::Rectangle,
            points: vec![vec2(12.0, 12.0), vec2(28.0, 28.0)],
            color: Color(0xff0000),
            width: 2.0,
            text: String::new(),
            ..Default::default()
        };
        let fonts = Arc::new(usvg::fontdb::Database::new());
        let output =
            render_selection(&image, bounds, selection, [&annotation].into_iter(), &fonts).unwrap();
        assert_eq!(output.dimensions(), (40, 40));
        assert_eq!(output.get_pixel(4, 4).0, [255, 0, 0, 255]);
        assert_eq!(output.get_pixel(20, 20).0, [255, 255, 255, 255]);
        assert!(
            render_selection(
                &image,
                bounds,
                Rect {
                    pos: vec2(70.0, 70.0),
                    size: Vec2::ONE
                },
                std::iter::empty::<&Annotation>(),
                &fonts
            )
            .is_err()
        );
    }

    #[test]
    fn cursor_is_drawn_at_its_position_scaled_and_blended() {
        let bounds = Rect {
            pos: vec2(100.0, 50.0),
            size: vec2(20.0, 10.0),
        };
        let mut image = RgbaImage::from_pixel(40, 20, Rgba([0, 0, 255, 255]));
        let mut cursor = RgbaImage::from_pixel(4, 4, Rgba([255, 0, 0, 255]));
        cursor.put_pixel(0, 0, Rgba([255, 0, 0, 0]));
        draw_cursor(
            &mut image,
            bounds,
            &CursorSnapshot {
                image: cursor,
                position: vec2(105.0, 52.0),
                size: vec2(4.0, 4.0),
            },
        );
        assert_eq!(image.get_pixel(9, 3).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(10, 4).0, [0, 0, 255, 255]);
        assert_eq!(image.get_pixel(13, 7).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(17, 11).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(18, 12).0, [0, 0, 255, 255]);
    }

    #[test]
    fn parallel_pixel_updates_cover_every_pixel() {
        for (width, height) in [(3, 7), (1, 1), (5, 64)] {
            let mut image =
                RgbaImage::from_fn(width, height, |x, y| Rgba([x as u8, y as u8, 9, 1]));
            update_pixels(&mut image, |pixel| pixel.swap(0, 2));
            assert!(
                image
                    .enumerate_pixels()
                    .all(|(x, y, pixel)| pixel.0 == [9, y as u8, x as u8, 1])
            );
        }
        update_pixels(&mut RgbaImage::new(0, 0), |_| unreachable!());
    }

    #[test]
    fn offset_image_bounds_shift_selection_and_annotations() {
        let image = RgbaImage::from_fn(40, 40, |x, y| Rgba([x as u8 * 6, y as u8 * 6, 0, 255]));
        let annotation = |offset| Annotation {
            tool: Tool::Rectangle,
            points: vec![vec2(6.0, 6.0) + offset, vec2(30.0, 30.0) + offset],
            color: Color(0xff0000),
            width: 2.0,
            text: String::new(),
            ..Default::default()
        };
        let fonts = Arc::new(usvg::fontdb::Database::new());
        let render = |offset: Vec2| {
            render_selection(
                &image,
                Rect {
                    pos: offset,
                    size: vec2(20.0, 20.0),
                },
                Rect {
                    pos: vec2(2.0, 2.0) + offset,
                    size: vec2(16.0, 16.0),
                },
                [&annotation(offset)].into_iter(),
                &fonts,
            )
            .unwrap()
        };
        assert_eq!(render(vec2(300.0, 150.0)), render(Vec2::ZERO));
        assert_eq!(render(Vec2::ZERO).dimensions(), (32, 32));
    }

    #[test]
    fn mosaic_changes_pixels_and_keeps_outside_untouched() {
        let image = RgbaImage::from_fn(20, 20, |x, _| Rgba([(x * 10) as u8, 0, 0, 255]));
        let bounds = Rect {
            pos: Vec2::ZERO,
            size: vec2(20.0, 20.0),
        };
        let annotation = Annotation {
            tool: Tool::Mosaic,
            points: vec![vec2(2.0, 2.0), vec2(18.0, 18.0)],
            color: Color::WHITE,
            width: 3.0,
            text: String::new(),
            ..Default::default()
        };
        let output = render_selection(
            &image,
            bounds,
            bounds,
            [&annotation].into_iter(),
            &Arc::new(usvg::fontdb::Database::new()),
        )
        .unwrap();
        assert_eq!(output.get_pixel(2, 2), output.get_pixel(3, 2));
        assert_eq!(output.get_pixel(0, 0), image.get_pixel(0, 0));
    }

    #[test]
    fn video_effects_flatten_annotations_between_blurs_by_time_range_in_video_pixels() {
        let canvas = Rect {
            pos: vec2(10.0, 10.0),
            size: vec2(20.0, 10.0),
        };
        let rectangle = Annotation {
            tool: Tool::Rectangle,
            points: vec![vec2(12.0, 12.0), vec2(16.0, 16.0)],
            filled: true,
            color: Color(0xff0000),
            ..Default::default()
        };
        let mosaic = Annotation {
            tool: Tool::Mosaic,
            points: vec![vec2(25.0, 18.0), vec2(20.0, 12.0)],
            intensity: 2.0,
            blur_mode: BlurMode::Pixelated,
            ..Default::default()
        };
        let effects = create_video_effects(
            (40, 20),
            canvas,
            [
                (&rectangle, None),
                (&rectangle, None),
                (&mosaic, Some((1.0, 2.0))),
                (&rectangle, None),
                (&rectangle, Some((0.5, 1.0))),
            ]
            .into_iter(),
            &Arc::new(usvg::fontdb::Database::new()),
        )
        .unwrap();
        let [
            VideoEffect::Overlay {
                image: overlay,
                time: None,
            },
            VideoEffect::Blur {
                rect,
                mode,
                amount,
                time: Some((1.0, 2.0)),
            },
            VideoEffect::Overlay { time: None, .. },
            VideoEffect::Overlay {
                time: Some((0.5, 1.0)),
                ..
            },
        ] = effects.as_slice()
        else {
            panic!("unexpected effects");
        };
        assert_eq!(overlay.dimensions(), (40, 20));
        assert_eq!(*overlay.get_pixel(8, 8), Rgba([255, 0, 0, 255]));
        assert_eq!(overlay.get_pixel(0, 0)[3], 0);
        assert_eq!(*rect, Rect::from_corners(vec2(20.0, 4.0), vec2(30.0, 16.0)));
        assert_eq!((*mode, *amount), (BlurMode::Pixelated, 4.0));
    }
}
