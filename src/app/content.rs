use crate::{
    app::{AppState, storage},
    platform, raster,
};
use anyhow::{Context as _, Result, ensure};
use gpui_kit::{App, ClipboardEntry, Task};
use image::{
    AnimationDecoder, Frame, ImageDecoder, ImageFormat, ImageReader, Rgba, RgbaImage,
    codecs::gif::GifDecoder,
};
use resvg::usvg::fontdb;
use std::{
    io::{BufRead, BufReader, Cursor, Seek},
    path::Path,
    sync::Arc,
};

#[derive(Clone)]
pub struct PinContent {
    pub image: Arc<RgbaImage>,
    pub frames: Option<Arc<Vec<Frame>>>,
    pub text: Option<String>,
}

impl PinContent {
    pub fn from_image(image: Arc<RgbaImage>) -> Self {
        return Self {
            image,
            frames: None,
            text: None,
        };
    }

    pub fn get_default_extension(&self) -> &'static str {
        return if self.frames.is_some() { "gif" } else { "png" };
    }
}

pub fn write_path(content: &PinContent, path: &Path) -> Result<()> {
    let path = if path.extension().is_none() {
        path.with_extension(content.get_default_extension())
    } else {
        path.to_path_buf()
    };
    if let Some(frames) = &content.frames
        && matches!(ImageFormat::from_path(&path), Ok(ImageFormat::Gif))
    {
        return storage::write_animation(frames, &path);
    }
    return storage::write_image(&content.image, &path);
}

pub fn load_clipboard(cx: &mut App) -> Result<Task<Result<PinContent>>> {
    let fonts = cx.global::<AppState>().fonts.clone();
    if let Some(path) = platform::get_clipboard_paths().into_iter().next() {
        return Ok(cx
            .background_executor()
            .spawn(async move { load_path(&path, &fonts) }));
    }
    let item = cx.read_from_clipboard().context("Clipboard is empty")?;
    let image = item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image.clone()),
        _ => None,
    });
    if let Some(image) = image {
        return Ok(cx
            .background_executor()
            .spawn(async move { decode_image(Cursor::new(&image.bytes)) }));
    }
    let text = item.text().context("Clipboard has no supported content")?;
    return Ok(cx
        .background_executor()
        .spawn(async move { load_text(text, &fonts) }));
}

pub fn load_path_in_background(path: &Path, cx: &App) -> Task<Result<PinContent>> {
    let fonts = cx.global::<AppState>().fonts.clone();
    let path = path.to_path_buf();
    return cx
        .background_executor()
        .spawn(async move { load_path(&path, &fonts) });
}

pub fn load_path(path: &Path, fonts: &Arc<fontdb::Database>) -> Result<PinContent> {
    if !path.is_dir() {
        match load_image_file(path) {
            Err(error)
                if matches!(
                    error.downcast_ref::<image::ImageError>(),
                    Some(image::ImageError::Unsupported(_))
                ) => {}
            result => return result.with_context(|| path.display().to_string()),
        }
    }
    return load_text(path.to_string_lossy().into_owned(), fonts)
        .with_context(|| path.display().to_string());
}

pub fn load_image_file(path: &Path) -> Result<PinContent> {
    return decode_image(BufReader::new(std::fs::File::open(path)?));
}

pub fn load_text(text: String, fonts: &Arc<fontdb::Database>) -> Result<PinContent> {
    if let Some(color) = parse_color(&text) {
        return Ok(PinContent::from_image(Arc::new(RgbaImage::from_pixel(
            240,
            160,
            Rgba(color),
        ))));
    }
    return Ok(PinContent {
        image: Arc::new(raster::render_text_image(&text, fonts)?),
        frames: None,
        text: Some(text),
    });
}

fn decode_image(reader: impl BufRead + Seek) -> Result<PinContent> {
    let reader = ImageReader::new(reader).with_guessed_format()?;
    if reader.format() == Some(ImageFormat::Gif) {
        let mut decoder = GifDecoder::new(reader.into_inner())?;
        decoder.set_limits(image::Limits::default())?;
        return collect_animation_frames(decoder.into_frames());
    }
    return Ok(PinContent::from_image(Arc::new(
        reader.decode()?.to_rgba8(),
    )));
}

fn collect_animation_frames(
    frames: impl Iterator<Item = image::ImageResult<Frame>>,
) -> Result<PinContent> {
    let mut decoded = Vec::new();
    let mut bytes = 0usize;
    for frame in frames {
        ensure!(decoded.len() < 500, "Animation has too many frames");
        let frame = frame?;
        bytes = bytes.saturating_add(frame.buffer().len());
        ensure!(bytes <= 256 * 1024 * 1024, "Animation is too large");
        decoded.push(frame);
    }
    let image = Arc::new(
        decoded
            .first()
            .context("Animation has no frames")?
            .buffer()
            .clone(),
    );
    return Ok(PinContent {
        image,
        frames: (decoded.len() > 1).then(|| Arc::new(decoded)),
        text: None,
    });
}

pub fn parse_color(text: &str) -> Option<[u8; 4]> {
    let text = text.trim();
    match text.to_ascii_lowercase().as_str() {
        "black" => return Some([0, 0, 0, 255]),
        "white" => return Some([255, 255, 255, 255]),
        "red" => return Some([255, 0, 0, 255]),
        "green" => return Some([0, 128, 0, 255]),
        "blue" => return Some([0, 0, 255, 255]),
        "transparent" => return Some([0, 0, 0, 0]),
        _ => {}
    }
    if let Some(hex) = text.strip_prefix('#') {
        if !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return None;
        }
        let mut color = [0, 0, 0, 255];
        match hex.len() {
            3 | 4 => {
                for (channel, digit) in color.iter_mut().zip(hex.chars()) {
                    *channel = digit.to_digit(16)? as u8 * 17;
                }
            }
            6 | 8 => {
                for (channel, pair) in color.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
                    *channel = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
                }
            }
            _ => return None,
        }
        return Some(color);
    }
    let values = text
        .strip_prefix("rgb(")
        .and_then(|text| text.strip_suffix(')'))
        .unwrap_or(text)
        .split(|character: char| character == ',' || character.is_whitespace())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if values.len() != 3 {
        return None;
    }
    let unit = values.iter().any(|value| value.contains(['.', 'e', 'E']));
    let mut color = [0, 0, 0, 255];
    for (channel, value) in color.iter_mut().zip(values) {
        *channel = if unit {
            let value = value.parse::<f32>().ok()?;
            if !(0.0..=1.0).contains(&value) {
                return None;
            }
            (value * 255.0).round() as u8
        } else {
            value.parse().ok()?
        };
    }
    return Some(color);
}

#[cfg(test)]
mod tests {
    use super::{
        collect_animation_frames, decode_image, load_image_file, load_path, parse_color, write_path,
    };
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::{io::Cursor, sync::Arc};

    #[test]
    fn decoding_gifs_rejects_oversized_canvases_before_allocating_pixels() {
        let mut bytes = Cursor::new(Vec::new());
        RgbaImage::new(1, 1)
            .write_to(&mut bytes, image::ImageFormat::Gif)
            .unwrap();
        let mut bytes = bytes.into_inner();
        bytes[6..10].fill(255);
        let error = decode_image(Cursor::new(bytes)).err().unwrap();
        assert!(
            matches!(
                error.downcast_ref::<image::ImageError>(),
                Some(image::ImageError::Limits(_))
            ),
            "{error:#}"
        );
    }

    #[test]
    fn clipboard_colors_accept_hex_integer_and_unit_rgb() {
        assert_eq!(parse_color("transparent"), Some([0, 0, 0, 0]));
        assert_eq!(parse_color("red"), Some([255, 0, 0, 255]));
        assert_eq!(parse_color("#0f8"), Some([0, 255, 136, 255]));
        assert_eq!(parse_color("#0102ff80"), Some([1, 2, 255, 128]));
        assert_eq!(parse_color("rgb(255, 2, 3)"), Some([255, 2, 3, 255]));
        assert_eq!(parse_color("1.0 0 0.5"), Some([255, 0, 128, 255]));
        assert_eq!(parse_color("1 0 0"), Some([1, 0, 0, 255]));
        assert_eq!(parse_color("256, 0, 0"), None);
        assert_eq!(parse_color("1.1 0.0 0.0"), None);
        assert_eq!(parse_color("#ggg"), None);
        assert_eq!(parse_color("#+f+f+f"), None);
        assert_eq!(parse_color("not a color"), None);
    }

    #[test]
    fn paths_load_images_or_render_their_names_and_report_broken_images() {
        let directory = tempfile::tempdir().unwrap();
        let image_path = directory.path().join("image.png");
        RgbaImage::from_pixel(2, 3, Rgba([1, 2, 3, 255]))
            .save(&image_path)
            .unwrap();
        let fonts = Arc::new(resvg::usvg::fontdb::Database::new());
        let content = load_path(&image_path, &fonts).unwrap();
        assert_eq!(content.image.dimensions(), (2, 3));
        assert_eq!(content.text, None);

        let file_path = directory.path().join("notes.txt");
        std::fs::write(&file_path, "notes").unwrap();
        let mut fonts = resvg::usvg::fontdb::Database::new();
        fonts.load_system_fonts();
        let fonts = Arc::new(fonts);
        let content = load_path(&file_path, &fonts).unwrap();
        assert_eq!(content.text.as_deref(), Some(file_path.to_str().unwrap()));

        assert!(load_path(&directory.path().join("missing.png"), &fonts).is_err());
        let corrupt_path = directory.path().join("corrupt.png");
        let mut corrupt = std::fs::read(&image_path).unwrap();
        corrupt.truncate(corrupt.len() / 2);
        std::fs::write(&corrupt_path, corrupt).unwrap();
        assert!(load_path(&corrupt_path, &fonts).is_err());
    }

    #[test]
    fn saving_animations_preserves_frames_and_delays_and_allows_still_exports() {
        let content = collect_animation_frames([80, 120].into_iter().map(|milliseconds| {
            Ok(Frame::from_parts(
                RgbaImage::from_pixel(2, 1, Rgba([milliseconds, 0, 0, 255])),
                0,
                0,
                Delay::from_numer_denom_ms(milliseconds.into(), 1),
            ))
        }))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        for name in ["animation.GIF", "animation"] {
            let path = directory.path().join(name);
            write_path(&content, &path).unwrap();
            let path = if path.extension().is_none() {
                path.with_extension("gif")
            } else {
                path
            };
            let loaded = load_image_file(&path).unwrap();
            let frames = loaded.frames.as_ref().unwrap();
            assert_eq!(frames.len(), 2);
            for (frame, expected) in frames.iter().zip(content.frames.as_ref().unwrap().iter()) {
                assert_eq!(frame.buffer(), expected.buffer());
                assert_eq!(frame.delay(), expected.delay());
            }
        }
        let path = directory.path().join("still.png");
        write_path(&content, &path).unwrap();
        let loaded = load_image_file(&path).unwrap();
        assert!(loaded.frames.is_none());
        assert_eq!(*loaded.image, *content.image);
        let path = directory.path().join("keep.txt");
        std::fs::write(&path, "keep").unwrap();
        assert!(write_path(&content, &path).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "keep");
    }

    #[test]
    fn animations_keep_every_frame() {
        let delay = Delay::from_numer_denom_ms(80, 1);
        let frame = |value| {
            Ok(Frame::from_parts(
                RgbaImage::from_pixel(2, 1, Rgba([value, 0, 0, 255])),
                0,
                0,
                delay,
            ))
        };
        let content = collect_animation_frames([frame(1), frame(2)].into_iter()).unwrap();
        assert_eq!(content.frames.as_ref().unwrap().len(), 2);
        assert_eq!(content.image.get_pixel(0, 0)[0], 1);
        let single = collect_animation_frames([frame(1)].into_iter()).unwrap();
        assert!(single.frames.is_none());
    }
}
