use anyhow::{Context as _, Result, ensure};
use image::{
    Frame, ImageFormat, RgbaImage,
    codecs::gif::{GifEncoder, Repeat},
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::File,
    io::ErrorKind,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn write_atomically(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    write(file.as_file_mut())?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    return Ok(());
}

pub fn read_toml<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    return match std::fs::read_to_string(path) {
        Ok(input) => toml::from_str(&input).with_context(|| path.display().to_string()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(error.into()),
    };
}

pub fn write_toml<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = toml::to_string(value)?;
    return write_atomically(path, |file| {
        std::io::Write::write_all(file, text.as_bytes())?;
        return Ok(());
    });
}

pub fn remove_file_if_exists(path: &Path) -> Result<()> {
    return match std::fs::remove_file(path) {
        Err(error) if error.kind() != ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    };
}

pub fn get_timestamp_id() -> u128 {
    return SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
}

pub fn list_numbered_files(directory: &Path, extension: &str) -> Result<Vec<(u128, PathBuf)>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut files = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|value| value == extension))
        .filter_map(|path| Some((path.file_stem()?.to_str()?.parse().ok()?, path)))
        .collect::<Vec<_>>();
    files.sort_unstable_by_key(|(id, _)| *id);
    return Ok(files);
}

pub fn create_unique_path(
    directory: &Path,
    prefix: &str,
    mut id: u128,
    extension: &str,
) -> PathBuf {
    loop {
        let path = directory.join(format!("{prefix}{id}.{extension}"));
        if !path.exists() {
            return path;
        }
        id += 1;
    }
}

pub fn write_image(image: &RgbaImage, path: &Path) -> Result<()> {
    let format = ImageFormat::from_path(path)?;
    ensure!(
        matches!(
            format,
            ImageFormat::Png
                | ImageFormat::Jpeg
                | ImageFormat::WebP
                | ImageFormat::Bmp
                | ImageFormat::Tiff
                | ImageFormat::Tga
                | ImageFormat::Ico
                | ImageFormat::Gif
        ),
        "Choose a PNG, JPEG, WebP, BMP, TIFF, TGA, ICO, or GIF filename"
    );
    return write_atomically(path, |file| {
        if format == ImageFormat::Jpeg {
            image::DynamicImage::ImageRgba8(image.clone())
                .to_rgb8()
                .write_to(file, format)?;
        } else {
            image.write_to(file, format)?;
        }
        return Ok(());
    });
}

pub fn write_animation(frames: &[Frame], path: &Path) -> Result<()> {
    return write_atomically(path, |file| {
        let mut encoder = GifEncoder::new(file);
        encoder.set_repeat(Repeat::Infinite)?;
        encoder.encode_frames(frames.iter().cloned())?;
        return Ok(());
    });
}

#[cfg(test)]
mod tests {
    use super::{create_unique_path, write_image};
    use image::{Rgba, RgbaImage};

    #[test]
    fn save_replaces_png_and_rejects_unsupported_formats_without_truncating() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capture.png");
        std::fs::write(&path, b"old file").unwrap();
        let image = RgbaImage::from_pixel(3, 2, Rgba([255, 50, 80, 128]));
        write_image(&image, &path).unwrap();
        assert_eq!(image::open(&path).unwrap().to_rgba8(), image);
        let unsupported = directory.path().join("capture.txt");
        std::fs::write(&unsupported, b"keep this").unwrap();
        assert!(write_image(&image, &unsupported).is_err());
        assert_eq!(std::fs::read(&unsupported).unwrap(), b"keep this");
        for extension in ["jpg", "webp", "bmp", "tiff", "tga", "ico", "gif"] {
            let path = directory.path().join(format!("capture.{extension}"));
            write_image(&image, &path).unwrap();
            let saved = image::open(path).unwrap();
            assert_eq!((saved.width(), saved.height()), (3, 2));
        }
    }

    #[test]
    fn unique_paths_do_not_replace_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        for extension in ["png", "gif"] {
            std::fs::write(
                directory.path().join(format!("Snipsy-7.{extension}")),
                b"existing",
            )
            .unwrap();
            assert_eq!(
                create_unique_path(directory.path(), "Snipsy-", 7, extension),
                directory.path().join(format!("Snipsy-8.{extension}"))
            );
        }
    }
}
