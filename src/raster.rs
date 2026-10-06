use std::{fmt::Write, io::Cursor, sync::Arc};

use gpui::RenderImage;
use image::{Frame, Rgba, RgbaImage};
use resvg::{tiny_skia, usvg};

use crate::{
    editor::{Annotation, BlurMode, Tool, get_arrow_head},
    geometry::Rect,
};

#[derive(Clone, Copy)]
pub enum PixelAlpha {
    Premultiplied,
    Straight,
    Opaque,
}

pub fn decode_bgra(
    bytes: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    alpha: PixelAlpha,
) -> Result<RgbaImage, String> {
    if width == 0 || height == 0 {
        return Err("Empty screen image".into());
    }
    let row = (width as usize)
        .checked_mul(4)
        .ok_or("Screen image too large")?;
    let length = row
        .checked_mul(height as usize)
        .ok_or("Screen image too large")?;
    let required = stride
        .checked_mul(height as usize - 1)
        .and_then(|size| size.checked_add(row))
        .ok_or("Screen image too large")?;
    if stride < row || bytes.len() < required {
        return Err("Invalid screen image stride or data length".into());
    }
    let mut pixels = Vec::new();
    pixels
        .try_reserve_exact(length)
        .map_err(|error| error.to_string())?;
    for y in 0..height as usize {
        for pixel in bytes[y * stride..y * stride + row].chunks_exact(4) {
            let a = if matches!(alpha, PixelAlpha::Opaque) {
                255
            } else {
                pixel[3]
            };
            let mut rgb = [pixel[2], pixel[1], pixel[0]];
            if matches!(alpha, PixelAlpha::Premultiplied) && a != 255 {
                rgb = rgb.map(|channel| {
                    if a == 0 {
                        0
                    } else {
                        ((channel as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
                    }
                });
            }
            pixels.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
    }
    return RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| "Invalid screen image dimensions".into());
}

pub fn create_render_image(image: &RgbaImage) -> Arc<RenderImage> {
    let mut image = image.clone();
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    return Arc::new(RenderImage::new(vec![Frame::new(image)]));
}

pub fn encode_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut output = Cursor::new(Vec::new());
    image
        .write_to(&mut output, image::ImageFormat::Png)
        .map_err(|error| error.to_string())?;
    return Ok(output.into_inner());
}

pub fn render_selection(
    image: &RgbaImage,
    bounds: Rect,
    selection: Rect,
    annotations: impl Iterator<Item = impl std::borrow::Borrow<Annotation>>,
    fonts: &Arc<usvg::fontdb::Database>,
) -> Result<RgbaImage, String> {
    let scale_x = image.width() as f32 / bounds.size.x;
    let scale_y = image.height() as f32 / bounds.size.y;
    let x = (selection.pos.x * scale_x).floor().max(0.0) as u32;
    let y = (selection.pos.y * scale_y).floor().max(0.0) as u32;
    let right = ((selection.pos.x + selection.size.x) * scale_x)
        .ceil()
        .min(image.width() as f32) as u32;
    let bottom = ((selection.pos.y + selection.size.y) * scale_y)
        .ceil()
        .min(image.height() as f32) as u32;
    if right <= x || bottom <= y || x >= image.width() || y >= image.height() {
        return Err("Selection is outside the image".into());
    }
    let mut output = image::imageops::crop_imm(image, x, y, right - x, bottom - y).to_image();
    let offset = glam::vec2(x as f32 / scale_x, y as f32 / scale_y);
    let mut svg = String::new();
    for annotation in annotations {
        let annotation = annotation.borrow();
        if annotation.tool == Tool::Mosaic {
            if !svg.is_empty() {
                composite_svg(&mut output, &svg, offset, scale_x, scale_y, fonts)?;
                svg.clear();
            }
            let start = annotation.points[0].min(*annotation.points.last().unwrap()) - offset;
            let end = annotation.points[0].max(*annotation.points.last().unwrap()) - offset;
            let left = (start.x * scale_x).max(0.0) as u32;
            let top = (start.y * scale_y).max(0.0) as u32;
            let right = (end.x * scale_x).ceil().clamp(0.0, output.width() as f32) as u32;
            let bottom = (end.y * scale_y).ceil().clamp(0.0, output.height() as f32) as u32;
            if annotation.blur_mode == BlurMode::Gaussian {
                if right > left && bottom > top {
                    let region =
                        image::imageops::crop_imm(&output, left, top, right - left, bottom - top)
                            .to_image();
                    let blurred = image::imageops::blur(
                        &region,
                        (annotation.intensity * scale_x / 3.0).max(0.5),
                    );
                    image::imageops::replace(&mut output, &blurred, left as i64, top as i64);
                }
                continue;
            }
            let block = (annotation.intensity * scale_x).max(1.0) as u32;
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

fn composite_svg(
    output: &mut RgbaImage,
    body: &str,
    offset: glam::Vec2,
    scale_x: f32,
    scale_y: f32,
    fonts: &Arc<usvg::fontdb::Database>,
) -> Result<(), String> {
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' width='{}' height='{}'><g transform='scale({scale_x} {scale_y}) translate({} {})'>{body}</g></svg>",
        output.width(),
        output.height(),
        -offset.x,
        -offset.y
    );
    let options = usvg::Options {
        fontdb: fonts.clone(),
        font_family: "Arial".into(),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(&svg, &options).map_err(|error| error.to_string())?;
    let mut pixmap =
        tiny_skia::Pixmap::new(output.width(), output.height()).ok_or("Image too large")?;
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
        svg.push_str(" opacity='0.35'");
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
        Tool::Arrow | Tool::Line => {
            let [left, right] = get_arrow_head(start, end, annotation.width);
            let _ = write!(
                svg,
                "<path d='M{} {}L{} {}'/>",
                start.x, start.y, end.x, end.y
            );
            if annotation.tool == Tool::Arrow {
                let _ = write!(
                    svg,
                    "<path d='M{} {}L{} {}L{} {}'/>",
                    left.x, left.y, end.x, end.y, right.x, right.y
                );
            }
        }
        Tool::Pen | Tool::Highlighter => {
            if annotation.points.len() == 1 {
                let _ = write!(
                    svg,
                    "<circle cx='{}' cy='{}' r='{}' fill='#{:06x}' stroke='none'/>",
                    start.x,
                    start.y,
                    annotation.width / 2.0,
                    annotation.color.0
                );
            } else {
                svg.push_str("<polyline points='");
                for point in &annotation.points {
                    let _ = write!(svg, "{},{} ", point.x, point.y);
                }
                svg.push_str("'/>");
            }
        }
        Tool::Text => {
            let _ = write!(
                svg,
                "<text fill='#{:06x}' stroke='none' font-family='Arial' font-size='{}'>",
                annotation.color.0, annotation.width
            );
            let mut offset = 0;
            for (index, line) in annotation.text.split('\n').enumerate() {
                let _ = write!(
                    svg,
                    "<tspan x='{}' y='{}'>",
                    start.x,
                    start.y + annotation.width * (1.0 + index as f32 * 1.3)
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
                        text.replace('&', "&amp;")
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
    use crate::geometry::Color;
    use glam::{Vec2, vec2};

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
        let mut fonts = usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        let output = render_selection(
            &RgbaImage::from_pixel(120, 80, Rgba([0, 0, 0, 255])),
            bounds,
            bounds,
            std::iter::once(&annotation),
            &Arc::new(fonts),
        )
        .unwrap();
        assert!(output.pixels().any(|pixel| pixel[0] > 0));
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
    fn capture_pixels_preserve_rows_padding_and_alpha() {
        let image = decode_bgra(
            &[10, 20, 30, 0, 99, 99, 99, 99, 40, 50, 60, 0],
            1,
            2,
            8,
            PixelAlpha::Opaque,
        )
        .unwrap();
        assert_eq!(image.get_pixel(0, 0).0, [30, 20, 10, 255]);
        assert_eq!(image.get_pixel(0, 1).0, [60, 50, 40, 255]);
        assert_eq!(
            decode_bgra(&[64, 32, 16, 128], 1, 1, 4, PixelAlpha::Premultiplied)
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [32, 64, 128, 128]
        );
        assert_eq!(
            decode_bgra(&[64, 32, 16, 128], 1, 1, 4, PixelAlpha::Straight)
                .unwrap()
                .get_pixel(0, 0)
                .0,
            [16, 32, 64, 128]
        );
        assert!(decode_bgra(&[0; 3], 1, 1, 4, PixelAlpha::Opaque).is_err());
        assert!(decode_bgra(&[0; 4], 1, 1, 3, PixelAlpha::Opaque).is_err());
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
}
