use image::{Pixel, Rgba, RgbaImage};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};

/// A point in image space, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    fn distance(self, other: Self) -> f32 {
        ((self.x - other.x).powi(2) + (self.y - other.y).powi(2)).sqrt()
    }
}

/// Evenly spaced dot centers along a polyline, `spacing` pixels apart.
///
/// This replaces the toolkit-specific dotted line of the previous renderer: it walks
/// consecutive points and steps along each segment.
fn dotted_line(points: &[Point], spacing: f32) -> Vec<Point> {
    if points.len() < 2 || spacing <= 0.0 {
        return vec![];
    }

    let mut dots = vec![];

    for (p0, p1) in points.iter().zip(&points[1..]) {
        let length = p0.distance(*p1);
        if length <= 0.0 {
            continue;
        }

        let dir = Point::new((p1.x - p0.x) / length, (p1.y - p0.y) / length);

        let mut travelled = 0.0;
        while travelled < length {
            dots.push(Point::new(
                p0.x + dir.x * travelled,
                p0.y + dir.y * travelled,
            ));
            travelled += spacing;
        }
    }

    dots
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PaintStroke {
    pub points: Vec<(f32, f32)>,
    pub fade: bool,
    pub color: [f32; 4],
    /// brush width from 0-1. 1 is equal to 1/10th of the smallest image dimension.
    pub width: f32,
    pub brush_index: usize,
    /// For ui preview: if highlit, paint brush stroke differently
    pub highlight: bool,
    pub committed: bool,
    pub flip_random: bool,
}

impl PaintStroke {
    pub fn without_points(&self) -> Self {
        Self {
            points: vec![],
            ..self.clone()
        }
    }

    pub fn new() -> Self {
        Self {
            color: [1., 1., 1., 1.],
            width: 0.05,
            ..Default::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    // render brush stroke
    pub fn render(&self, img: &mut RgbaImage, brushes: &[RgbaImage]) {
        // Calculate the brush: use a fraction of the smallest image size
        let max_brush_size = img.width().min(img.height());

        let mut brush = image::imageops::resize(
            &brushes[self.brush_index],
            (self.width * max_brush_size as f32) as u32,
            (self.width * max_brush_size as f32) as u32,
            image::imageops::Triangle,
        );

        // transform points from UV into image space
        let abs_points = self
            .points
            .iter()
            .map(|p| Point::new(img.width() as f32 * p.0, img.height() as f32 * p.1))
            .collect::<Vec<_>>();

        let points = dotted_line(&abs_points, (brush.width() as f32 / 4.0).max(1.5));

        for (i, pos_on_line) in points.iter().enumerate() {
            if self.flip_random {
                // seed by brush position so randomness only changes per brush instance
                let mut rng =
                    ChaCha8Rng::seed_from_u64(pos_on_line.x as u64 + pos_on_line.y as u64);

                let flip_x: bool = rng.random();
                let flip_y: bool = rng.random();

                if flip_x {
                    image::imageops::flip_horizontal_in_place(&mut brush);
                }
                if flip_y {
                    image::imageops::flip_vertical_in_place(&mut brush);
                }
            }

            let mut stroke_color = self.color;

            if self.fade {
                let fraction = 1.0 - i as f32 / points.len() as f32;
                stroke_color[3] *= fraction;
            }

            if self.highlight {
                stroke_color[0] *= 2.5;
                stroke_color[1] *= 2.5;
                stroke_color[2] *= 2.5;
                stroke_color[3] *= 2.5;
            }
            paint_at(img, &brush, pos_on_line, stroke_color);
        }
    }
}

pub fn paint_at(img: &mut RgbaImage, brush: &RgbaImage, pos: &Point, color: [f32; 4]) {
    // To test
    // img.put_pixel(pos.x as u32, pos.y as u32, color_to_pixel(color));
    // return;

    let brush_offset = Point::new(brush.width() as f32 / 2., brush.height() as f32 / 2.);
    let top_left = Point::new(pos.x - brush_offset.x, pos.y - brush_offset.y);

    for (b_x, b_y, b_pixel) in brush.enumerate_pixels() {
        if let Some(p) = img.get_pixel_mut_checked(top_left.x as u32 + b_x, top_left.y as u32 + b_y)
        {
            // multiply brush with user color os it's tinted
            let colored_pixel = Rgba([
                (color[0] * b_pixel[0] as f32) as u8,
                (color[1] * b_pixel[1] as f32) as u8,
                (color[2] * b_pixel[2] as f32) as u8,
                (color[3] * b_pixel[3] as f32) as u8,
            ]);
            // colored_pixel.blend(&color_to_pixel(color));
            p.blend(&colored_pixel);
        }
    }
}
