//! Raster writer for a [`Drawing`]: tiny-skia rendering and PNG encoding.

use image::ImageEncoder;
use primeval_core::{Color, Drawing, Geometry};
use tiny_skia::{FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, Rect, Stroke, Transform};

/// Render `drawing` onto `width` × `height` output pixels as opaque RGB8.
///
/// The canvas is stretched onto the output with `scale(width / canvas width,
/// height / canvas height)`, which matches the SVG writer's `viewBox` up to the
/// rounding of the output size. Returns `None` when tiny-skia cannot allocate
/// the pixmap.
pub(crate) fn render_rgb(drawing: &Drawing, width: u32, height: u32) -> Option<Vec<u8>> {
    let mut pixmap = Pixmap::new(width, height)?;
    pixmap.fill(skia_color(drawing.background));
    let transform = Transform::from_scale(
        width as f32 / drawing.width as f32,
        height as f32 / drawing.height as f32,
    );

    for shape in &drawing.shapes {
        let mut paint = Paint::default();
        let color = shape.color;
        paint.set_color_rgba8(color.r, color.g, color.b, color.a);
        paint.anti_alias = true;

        match &shape.geometry {
            Geometry::Quadratic {
                start,
                control,
                end,
                width,
            } => {
                let mut builder = PathBuilder::new();
                builder.move_to(start.x as f32, start.y as f32);
                builder.quad_to(
                    control.x as f32,
                    control.y as f32,
                    end.x as f32,
                    end.y as f32,
                );
                let Some(path) = builder.finish() else {
                    continue;
                };
                let stroke = Stroke {
                    width: *width as f32,
                    line_cap: LineCap::Butt,
                    ..Stroke::default()
                };
                pixmap.stroke_path(&path, &paint, &stroke, transform, None);
            }
            Geometry::Ellipse {
                cx, cy, rotation, ..
            } => {
                let Some(path) = fill_path(&shape.geometry) else {
                    continue;
                };
                let transform = transform.pre_rotate_at(*rotation as f32, *cx as f32, *cy as f32);
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
            Geometry::Rect { .. } | Geometry::Polygon(_) => {
                let Some(path) = fill_path(&shape.geometry) else {
                    continue;
                };
                pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
            }
        }
    }

    // The background is opaque and source-over keeps it opaque, so the
    // premultiplied colour channels are the straight ones.
    Some(
        pixmap
            .pixels()
            .iter()
            .flat_map(|pixel| [pixel.red(), pixel.green(), pixel.blue()])
            .collect(),
    )
}

/// The unrotated outline of a filled geometry, in canvas coordinates.
fn fill_path(geometry: &Geometry) -> Option<Path> {
    match geometry {
        Geometry::Rect {
            x,
            y,
            width,
            height,
        } => Rect::from_xywh(*x as f32, *y as f32, *width as f32, *height as f32)
            .map(PathBuilder::from_rect),
        Geometry::Ellipse { cx, cy, rx, ry, .. } => PathBuilder::from_oval(Rect::from_ltrb(
            (cx - rx) as f32,
            (cy - ry) as f32,
            (cx + rx) as f32,
            (cy + ry) as f32,
        )?),
        Geometry::Polygon(points) => {
            let (first, rest) = points.split_first()?;
            let mut builder = PathBuilder::new();
            builder.move_to(first.x as f32, first.y as f32);
            for point in rest {
                builder.line_to(point.x as f32, point.y as f32);
            }
            builder.close();
            builder.finish()
        }
        Geometry::Quadratic { .. } => None,
    }
}

fn skia_color(color: Color) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(color.r, color.g, color.b, color.a)
}

/// Encode opaque RGB8 pixels as a PNG image.
pub(crate) fn encode_png(width: u32, height: u32, rgb: &[u8]) -> image::ImageResult<Vec<u8>> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(
        rgb,
        width,
        height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use primeval_core::{DrawnShape, Point};

    const WHITE: Color = Color::new(255, 255, 255, 255);
    const RED: Color = Color::new(255, 0, 0, 255);

    fn drawing(shapes: Vec<DrawnShape>) -> Drawing {
        Drawing {
            width: 10,
            height: 10,
            background: WHITE,
            shapes,
        }
    }

    fn pixel(rgb: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * width + x) * 3) as usize;
        [rgb[i], rgb[i + 1], rgb[i + 2]]
    }

    #[test]
    fn output_has_the_requested_dimensions() {
        let rgb = render_rgb(&drawing(Vec::new()), 40, 25).expect("render");
        assert_eq!(rgb.len(), 40 * 25 * 3);
        assert!(rgb.iter().all(|&channel| channel == 255));
    }

    #[test]
    fn rect_covers_its_canvas_square_scaled() {
        // Canvas [2, 5) x [3, 7) at scale 4 is output [8, 20) x [12, 28).
        let rect = DrawnShape {
            geometry: Geometry::Rect {
                x: 2.0,
                y: 3.0,
                width: 3.0,
                height: 4.0,
            },
            color: RED,
        };
        let rgb = render_rgb(&drawing(vec![rect]), 40, 40).expect("render");

        for y in 0..40 {
            for x in 0..40 {
                let inside = (8..20).contains(&x) && (12..28).contains(&y);
                let expected = if inside { [255, 0, 0] } else { [255, 255, 255] };
                assert_eq!(pixel(&rgb, 40, x, y), expected, "({x}, {y})");
            }
        }
    }

    #[test]
    fn non_uniform_scale_stretches_like_the_svg_view_box() {
        // A 10 x 10 canvas onto 20 x 40: x doubles, y quadruples.
        let rect = DrawnShape {
            geometry: Geometry::Rect {
                x: 1.0,
                y: 1.0,
                width: 2.0,
                height: 2.0,
            },
            color: RED,
        };
        let rgb = render_rgb(&drawing(vec![rect]), 20, 40).expect("render");

        assert_eq!(pixel(&rgb, 20, 2, 4), [255, 0, 0]);
        assert_eq!(pixel(&rgb, 20, 5, 11), [255, 0, 0]);
        assert_eq!(pixel(&rgb, 20, 1, 4), [255, 255, 255]);
        assert_eq!(pixel(&rgb, 20, 6, 4), [255, 255, 255]);
        assert_eq!(pixel(&rgb, 20, 2, 3), [255, 255, 255]);
        assert_eq!(pixel(&rgb, 20, 2, 12), [255, 255, 255]);
    }

    #[test]
    fn every_geometry_kind_paints() {
        let shapes = [
            Geometry::Ellipse {
                cx: 5.0,
                cy: 5.0,
                rx: 3.0,
                ry: 1.5,
                rotation: 90.0,
            },
            Geometry::Polygon(vec![
                Point::new(1.0, 1.0),
                Point::new(9.0, 1.0),
                Point::new(5.0, 9.0),
            ]),
            Geometry::Quadratic {
                start: Point::new(1.0, 5.0),
                control: Point::new(5.0, 5.0),
                end: Point::new(9.0, 5.0),
                width: 1.0,
            },
        ];
        for geometry in shapes {
            let shape = DrawnShape {
                geometry: geometry.clone(),
                color: RED,
            };
            let rgb = render_rgb(&drawing(vec![shape]), 40, 40).expect("render");
            assert_eq!(pixel(&rgb, 40, 20, 20), [255, 0, 0], "{geometry:?}");
        }
    }

    #[test]
    fn rotation_turns_the_ellipse_around_its_centre() {
        let ellipse = DrawnShape {
            geometry: Geometry::Ellipse {
                cx: 5.0,
                cy: 5.0,
                rx: 4.0,
                ry: 1.0,
                rotation: 90.0,
            },
            color: RED,
        };
        let rgb = render_rgb(&drawing(vec![ellipse]), 40, 40).expect("render");

        // Rotated by 90 degrees, the long axis is vertical.
        assert_eq!(pixel(&rgb, 40, 20, 6), [255, 0, 0]);
        assert_eq!(pixel(&rgb, 40, 6, 20), [255, 255, 255]);
    }

    #[test]
    fn png_is_opaque_rgb() {
        let rgb = render_rgb(&drawing(Vec::new()), 4, 3).expect("render");
        let png = encode_png(4, 3, &rgb).expect("png");

        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
        let decoded = image::load_from_memory(&png).expect("decode");
        assert_eq!(decoded.color(), image::ColorType::Rgb8);
        assert_eq!((decoded.width(), decoded.height()), (4, 3));
    }
}
