use serde::{Deserialize, Serialize};

pub const IMAGE_BYTES: usize = 5 * 1024 * 1024;
pub const MESSAGE_BYTES: usize = 5 * 1024 * 1024;
pub const COUNT: usize = 4;
pub const CHUNK: usize = 32768;
pub const READ_CHUNK: usize = 512 * 1024;
pub const PIXELS: u64 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ImageRef {
    pub id: String,
    pub media_type: String,
    pub size: usize,
    pub width: u32,
    pub height: u32,
}
impl ImageRef {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.len() != 64
            || !self
                .id
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err("Invalid image ID.".into());
        }
        if !matches!(self.media_type.as_str(), "image/png" | "image/jpeg") {
            return Err("Only PNG and JPEG images are supported.".into());
        }
        if self.size == 0 || self.size > IMAGE_BYTES {
            return Err("Each image must be at most 5 MiB.".into());
        }
        dimensions(self.width, self.height)
    }
}
pub fn dimensions(width: u32, height: u32) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("Image dimensions could not be read.".into());
    }
    if width > 8192 || height > 8192 || u64::from(width) * u64::from(height) > PIXELS {
        return Err(
            "Images must fit within 8192 pixels per side and 16,777,216 pixels total.".into(),
        );
    }
    Ok(())
}
pub fn validate_message(images: &[ImageRef]) -> Result<(), String> {
    if images.len() > COUNT {
        return Err("A message can have at most 4 images.".into());
    }
    let mut seen = std::collections::HashSet::new();
    for image in images {
        image.validate()?;
        if !seen.insert(&image.id) {
            return Err("This image is already attached.".into());
        }
    }
    if images.iter().map(|image| image.size).sum::<usize>() > MESSAGE_BYTES {
        return Err("Images in a message must total at most 5 MiB.".into());
    }
    Ok(())
}
