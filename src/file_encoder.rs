//! File encoders - this defines save options.
//!
//! To add more formats, add a variant to the `[FileEncoder]` struct.

use anyhow::Result;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, PngEncoder};
use image::{DynamicImage, ImageEncoder};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use strum::{Display, EnumIter};

#[derive(Default, Debug, Serialize, Deserialize, Clone, Eq, PartialEq, Display, EnumIter)]

pub enum CompressionLevel {
    #[default]
    Default,
    Best,
    Fast,
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq, Display, EnumIter)]
pub enum FileEncoder {
    Jpg { quality: u32 },
    Png { compressionlevel: CompressionLevel },
    Bmp,
    WebP,
    Avif,
}

impl Default for FileEncoder {
    fn default() -> Self {
        Self::Png {
            compressionlevel: CompressionLevel::Default,
        }
    }
}

impl FileEncoder {
    pub fn matching_variant(path: &Path, variants: &Vec<Self>) -> Self {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default()
            .to_lowercase()
            .replace("jpeg", "jpg");

        for v in variants {
            if v.ext() == ext {
                return v.clone();
            }
        }

        Self::Png {
            compressionlevel: CompressionLevel::Default,
        }
    }

    pub fn ext(&self) -> String {
        self.to_string().to_lowercase()
    }

    pub fn save(&self, image: &DynamicImage, path: &Path) -> Result<()> {
        match self {
            FileEncoder::Jpg { quality } => {
                let w = File::create(path)?;
                JpegEncoder::new_with_quality(w, *quality as u8).encode_image(image)?;
            }
            FileEncoder::Png { compressionlevel } => {
                let file = File::create(path)?;
                let writer = BufWriter::new(file);
                let encoder = PngEncoder::new_with_quality(
                    writer,
                    match compressionlevel {
                        CompressionLevel::Best => CompressionType::Best,
                        CompressionLevel::Default => CompressionType::Default,
                        CompressionLevel::Fast => CompressionType::Fast,
                    },
                    image::codecs::png::FilterType::default(),
                );
                encoder.write_image(
                    image.as_bytes(),
                    image.width(),
                    image.height(),
                    image::ExtendedColorType::Rgba8,
                )?;
            }
            FileEncoder::Bmp => {
                image.save_with_format(path, image::ImageFormat::Bmp)?;
            }
            FileEncoder::WebP => {
                image.save_with_format(path, image::ImageFormat::WebP)?;
            }
            FileEncoder::Avif => {
                image.save_with_format(path, image::ImageFormat::Avif)?;
            }
        }
        Ok(())
    }
}

/// Save an image with the encoding options that match the destination, then re-apply its EXIF and
/// refresh its thumbnail. The egui shell kept this in its UI module, but nothing about it is UI,
/// so both shells share it from here.
pub fn save_with_encoding(
    image: &DynamicImage,
    path: &Path,
    image_info: &Option<crate::utils::ExtendedImageInfo>,
    encoders: &Vec<FileEncoder>,
) -> Result<()> {
    let encoding_options = FileEncoder::matching_variant(path, encoders);
    encoding_options.save(image, path)?;
    log::debug!("Saved to {}", path.display());
    if let Some(info) = image_info
        && info.raw_exif.is_some()
    {
        crate::utils::fix_exif(path, info.raw_exif.clone())?;
    }
    crate::thumbnails::generate(path)?;
    Ok(())
}
