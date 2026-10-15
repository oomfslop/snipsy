use crate::{
    app::{
        content::{self, PinContent},
        storage,
    },
    geometry::Rect,
    pin::{MIN_SIZE, PinData},
};
use anyhow::{Result, ensure};
use glam::Vec2;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::Path, sync::Arc};

pub enum BackupCommand {
    Save(PinData),
    Remove(u128),
}

#[derive(Serialize, Deserialize)]
struct PinMetadata {
    bounds: Rect,
    opacity: f32,
    original_size: Vec2,
    thumbnail_size: Option<Vec2>,
    text: Option<String>,
    animated: bool,
}

pub async fn run_worker(
    directory: &Path,
    commands: async_channel::Receiver<BackupCommand>,
    mut written: HashMap<u128, PinContent>,
) {
    while let Ok(command) = commands.recv().await {
        let result = match command {
            BackupCommand::Save(pin) => write(directory, &pin, &mut written),
            BackupCommand::Remove(id) => {
                written.remove(&id);
                remove(directory, id)
            }
        };
        if let Err(error) = result {
            eprintln!("could not back up pin: {error:#}");
        }
    }
}

fn get_image_path(directory: &Path, id: u128, animated: bool) -> std::path::PathBuf {
    return directory.join(format!("{id}.{}", if animated { "gif" } else { "png" }));
}

fn write(directory: &Path, pin: &PinData, written: &mut HashMap<u128, PinContent>) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let content = &pin.content;
    let unchanged = written.get(&pin.id).is_some_and(|previous| {
        Arc::ptr_eq(&previous.image, &content.image)
            && previous.frames.as_ref().map(Arc::as_ptr) == content.frames.as_ref().map(Arc::as_ptr)
    });
    let animated = content.frames.is_some();
    if !unchanged {
        let path = get_image_path(directory, pin.id, animated);
        match &content.frames {
            Some(frames) => storage::write_animation(frames, &path)?,
            None => storage::write_image(&content.image, &path)?,
        }
    }
    storage::write_toml(
        &directory.join(format!("{}.toml", pin.id)),
        &PinMetadata {
            bounds: pin.bounds,
            opacity: pin.opacity,
            original_size: pin.original_size,
            thumbnail_size: pin.thumbnail_size,
            text: content.text.clone(),
            animated,
        },
    )?;
    if !unchanged {
        storage::remove_file_if_exists(&get_image_path(directory, pin.id, !animated))?;
        written.insert(pin.id, content.clone());
    }
    return Ok(());
}

fn remove(directory: &Path, id: u128) -> Result<()> {
    for extension in ["png", "gif", "toml"] {
        storage::remove_file_if_exists(&directory.join(format!("{id}.{extension}")))?;
    }
    return Ok(());
}

pub fn load(directory: &Path) -> Result<Vec<PinData>> {
    let mut pins = Vec::new();
    for (id, path) in storage::list_numbered_files(directory, "toml")? {
        match std::fs::read_to_string(&path)
            .map_err(Into::into)
            .and_then(|text| read(directory, id, &text))
        {
            Ok(pin) => pins.push(pin),
            Err(error) => eprintln!("could not restore pin {id}: {error:#}"),
        }
    }
    return Ok(pins);
}

fn read(directory: &Path, id: u128, text: &str) -> Result<PinData> {
    let metadata: PinMetadata = toml::from_str(text)?;
    ensure!(
        metadata.bounds.pos.is_finite()
            && metadata.bounds.size.is_finite()
            && metadata.original_size.is_finite()
            && metadata.original_size.min_element() > 0.0
            && metadata
                .thumbnail_size
                .is_none_or(|size| size.is_finite() && size.min_element() > 0.0)
            && (0.2..=1.0).contains(&metadata.opacity),
        "invalid geometry or opacity"
    );
    let loaded = content::load_image_file(&get_image_path(directory, id, metadata.animated))?;
    return Ok(PinData {
        id,
        content: PinContent {
            text: metadata.text,
            ..loaded
        },
        bounds: Rect {
            size: metadata.bounds.size.max(Vec2::splat(MIN_SIZE)),
            ..metadata.bounds
        },
        opacity: metadata.opacity,
        original_size: metadata.original_size,
        thumbnail_size: metadata.thumbnail_size,
    });
}

#[cfg(test)]
mod tests {
    use super::{load, read, remove, write};
    use crate::{app::content::PinContent, geometry::Rect, pin::PinData};
    use glam::vec2;
    use image::{Delay, Frame, Rgba, RgbaImage};
    use std::{collections::HashMap, sync::Arc};

    fn create_pin(id: u128, content: PinContent) -> PinData {
        return PinData {
            id,
            content,
            bounds: Rect {
                pos: vec2(-20.0, 30.0),
                size: vec2(320.0, 180.0),
            },
            opacity: 0.6,
            original_size: vec2(640.0, 360.0),
            thumbnail_size: Some(vec2(160.0, 90.0)),
        };
    }

    #[test]
    fn backups_round_trip_text_geometry_and_reject_invalid_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let image = Arc::new(RgbaImage::from_pixel(3, 2, Rgba([1, 2, 3, 255])));
        let pin = create_pin(
            42,
            PinContent {
                image: image.clone(),
                frames: None,
                text: Some("héllo".into()),
            },
        );
        write(directory.path(), &pin, &mut HashMap::new()).unwrap();
        let loaded = load(directory.path()).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, 42);
        assert_eq!(*loaded[0].content.image, *image);
        assert_eq!(loaded[0].content.text.as_deref(), Some("héllo"));
        assert_eq!(loaded[0].opacity, 0.6);
        assert_eq!(loaded[0].bounds, pin.bounds);
        std::fs::write(
            directory.path().join("42.toml"),
            "opacity = 2.0\noriginal_size = [1.0, 1.0]\nanimated = false\n\n[bounds]\npos = [0.0, 0.0]\nsize = [10.0, 10.0]\n",
        )
        .unwrap();
        assert!(load(directory.path()).unwrap().is_empty());
        remove(directory.path(), 42).unwrap();
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn restoring_pins_rejects_nonfinite_geometry() {
        let directory = tempfile::tempdir().unwrap();
        let pin = create_pin(42, PinContent::from_image(Arc::new(RgbaImage::new(1, 1))));
        write(directory.path(), &pin, &mut HashMap::new()).unwrap();
        let text = std::fs::read_to_string(directory.path().join("42.toml")).unwrap();
        let replace = |metadata: &mut toml::Table, path: &[&str], value: toml::Value| {
            let (last, parents) = path.split_last().unwrap();
            let mut table = metadata;
            for key in parents {
                table = table[*key].as_table_mut().unwrap();
            }
            match &mut table[*last] {
                toml::Value::Array(array) => array[0] = value,
                field => *field = value,
            }
        };
        for (path, value) in [
            (&["bounds", "pos"][..], toml::Value::Float(1e40)),
            (&["bounds", "size"], toml::Value::Float(1e40)),
            (&["original_size"], toml::Value::Float(1e40)),
            (&["thumbnail_size"], toml::Value::Float(1e40)),
        ] {
            let mut metadata: toml::Table = text.parse().unwrap();
            replace(&mut metadata, path, value);
            assert!(
                read(directory.path(), 42, &metadata.to_string()).is_err(),
                "{path:?}"
            );
        }
    }

    #[test]
    fn restoring_pins_clamps_slightly_undersized_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let mut pin = create_pin(42, PinContent::from_image(Arc::new(RgbaImage::new(1, 1))));
        pin.bounds.size = vec2(39.9999, 120.0);
        write(directory.path(), &pin, &mut HashMap::new()).unwrap();
        let loaded = load(directory.path()).unwrap();
        assert_eq!(loaded[0].bounds.size, vec2(super::MIN_SIZE, 120.0));
    }

    #[test]
    fn animated_backups_keep_frames_and_delays_until_metadata_commits() {
        let delay = Delay::from_numer_denom_ms(80, 1);
        let frames = vec![
            Frame::from_parts(
                RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255])),
                0,
                0,
                delay,
            ),
            Frame::from_parts(
                RgbaImage::from_pixel(2, 1, Rgba([4, 5, 6, 255])),
                0,
                0,
                delay,
            ),
        ];
        let directory = tempfile::tempdir().unwrap();
        let mut pin = create_pin(
            43,
            PinContent {
                image: Arc::new(frames[0].buffer().clone()),
                frames: Some(Arc::new(frames)),
                text: None,
            },
        );
        let mut written = HashMap::new();
        write(directory.path(), &pin, &mut written).unwrap();
        let loaded = load(directory.path()).unwrap();
        let frames = loaded[0].content.frames.as_ref().unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[1].delay(), delay);
        let metadata = directory.path().join("43.toml");
        let saved_metadata = metadata.with_extension("toml.bak");
        std::fs::rename(&metadata, &saved_metadata).unwrap();
        std::fs::create_dir(&metadata).unwrap();
        pin.content.frames = None;
        assert!(write(directory.path(), &pin, &mut written).is_err());
        assert!(directory.path().join("43.gif").exists());
        assert!(written[&43].frames.is_some());
        std::fs::remove_dir(&metadata).unwrap();
        std::fs::rename(&saved_metadata, &metadata).unwrap();
        assert!(load(directory.path()).unwrap()[0].content.frames.is_some());
        write(directory.path(), &pin, &mut written).unwrap();
        assert!(!directory.path().join("43.gif").exists());
        assert!(load(directory.path()).unwrap()[0].content.frames.is_none());
    }
}
