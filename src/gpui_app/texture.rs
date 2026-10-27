use gpui_kit::RenderImage;
use image::{DynamicImage, Frame, RgbaImage};
use std::sync::Arc;

use crate::utils::ColorChannel;

/// GPUI hands a `RenderImage` buffer straight to its sprite atlas, which expects BGRA.
/// Only the `ImageSource::Resource` decode path converts for us, so R and B swap here.
///
/// The egui shell swizzled channels in its image shader; gpui has no custom shader on this
/// path, so the same selection is baked into the upload instead.
pub fn to_render_image(img: &DynamicImage, channel: ColorChannel) -> Arc<RenderImage> {
    let mut rgba = img.as_rgba8().cloned().unwrap_or_else(|| img.to_rgba8());
    if channel != ColorChannel::Rgba {
        for pixel in rgba.pixels_mut() {
            let [r, g, b, a] = pixel.0;
            pixel.0 = match channel {
                ColorChannel::Red => [r, r, r, 255],
                ColorChannel::Green => [g, g, g, 255],
                ColorChannel::Blue => [b, b, b, 255],
                ColorChannel::Alpha => [a, a, a, 255],
                ColorChannel::Rgb => [r, g, b, 255],
                ColorChannel::Rgba => [r, g, b, a],
            };
        }
    }
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(rgba)]))
}

/// egui's top bar overwrote `current_channel` with RGBA on **every frame** the edit panel was open
/// (`src/ui/top_bar.rs:47-54`), so entering edit mode discards whatever channel was selected and
/// leaving it does not restore that choice.
pub fn shown_channel(edit_enabled: bool, requested: ColorChannel) -> ColorChannel {
    if edit_enabled {
        ColorChannel::Rgba
    } else {
        requested
    }
}

/// egui's `load_brush_texture` (HEAD `src/ui/edit_ui.rs:11`) uploaded each brush bitmap and cached it
/// by URI so the stroke combo could show the dab it paints. These are the same bitmaps
/// `PaintStroke::render` uses, in the format gpui's atlas wants.
pub fn brush_textures(brushes: &[RgbaImage]) -> Vec<Arc<RenderImage>> {
    brushes
        .iter()
        .map(|brush| to_render_image(&DynamicImage::ImageRgba8(brush.clone()), ColorChannel::Rgba))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{brush_textures, shown_channel};
    use crate::image_editing::EditState;
    use crate::utils::ColorChannel;
    use image::{Rgba, RgbaImage};

    #[test]
    fn edit_mode_overrides_whichever_channel_was_selected() {
        assert_eq!(shown_channel(true, ColorChannel::Red), ColorChannel::Rgba);
        assert_eq!(shown_channel(true, ColorChannel::Alpha), ColorChannel::Rgba);
        assert_eq!(shown_channel(false, ColorChannel::Red), ColorChannel::Red);
        assert_eq!(shown_channel(true, ColorChannel::Rgba), ColorChannel::Rgba);
    }

    /// The atlas wants BGRA, and the size the preview box is laid out from has to survive the upload.
    #[test]
    fn a_brush_upload_swaps_red_for_blue_and_keeps_its_pixels() {
        let dab = RgbaImage::from_pixel(2, 2, Rgba([255, 10, 3, 200]));
        let textures = brush_textures(&[dab]);
        assert_eq!(textures.len(), 1);
        let bytes = textures[0].as_bytes(0).expect("one frame");
        assert_eq!(bytes.len(), 2 * 2 * 4);
        assert_eq!(&bytes[..4], &[3, 10, 255, 200]);
        assert_eq!(&bytes[bytes.len() - 4..], &[3, 10, 255, 200]);
    }

    /// One preview per embedded brush: a PNG that stopped decoding or a count that drifted would show
    /// up here as a short list or an empty frame.
    #[test]
    fn every_embedded_brush_uploads() {
        let brushes = EditState::default().brushes;
        let textures = brush_textures(&brushes);
        assert_eq!(textures.len(), brushes.len());
        assert!(
            textures.len() > 1,
            "egui's combo listed more than one brush"
        );
        for (texture, brush) in textures.iter().zip(&brushes) {
            assert_eq!(
                texture.as_bytes(0).map(<[u8]>::len),
                Some((brush.width() * brush.height() * 4) as usize),
            );
        }
    }
}
