//! Checks that `Shape::geometry` covers the pixels the engine rasterizes.
//!
//! Each kind's engine rasterizer and its geometry, drawn with tiny-skia at
//! scale 1, are reduced to masks of covered pixels and compared.

use super::*;
use crate::drawing::{Geometry, Point};
use crate::test_util::make_test_round;
use tiny_skia::{FillRule, LineCap, Paint, PathBuilder, Pixmap, Stroke, Transform};

const W: u32 = 64;
const H: u32 = 48;

fn engine_mask(shape: &Shape, worker: &mut WorkerCtx<rand_chacha::ChaCha8Rng>) -> Vec<bool> {
    let mut coverage = vec![0u32; (W * H) as usize];
    for line in shape.rasterize(worker) {
        if line.y < 0 || line.y >= H as i32 {
            continue;
        }
        for x in line.x1.max(0)..=line.x2.min(W as i32 - 1) {
            let i = line.y as usize * W as usize + x as usize;
            coverage[i] = coverage[i].max(line.alpha);
        }
    }
    coverage.iter().map(|&a| a >= 0x8000).collect()
}

fn geometry_mask(geometry: &Geometry) -> Vec<bool> {
    let mut pixmap = Pixmap::new(W, H).expect("pixmap");
    let mut paint = Paint::default();
    paint.set_color_rgba8(0, 0, 0, 255);
    paint.anti_alias = true;
    let t = Transform::identity();
    let mut pb = PathBuilder::new();
    match geometry {
        Geometry::Rect {
            x,
            y,
            width,
            height,
        } => {
            pb.push_rect(
                tiny_skia::Rect::from_xywh(*x as f32, *y as f32, *width as f32, *height as f32)
                    .expect("rect"),
            );
            pixmap.fill_path(
                &pb.finish().expect("path"),
                &paint,
                FillRule::Winding,
                t,
                None,
            );
        }
        Geometry::Ellipse {
            cx,
            cy,
            rx,
            ry,
            rotation,
        } => {
            pb.push_oval(
                tiny_skia::Rect::from_xywh(
                    (cx - rx) as f32,
                    (cy - ry) as f32,
                    (2.0 * rx) as f32,
                    (2.0 * ry) as f32,
                )
                .expect("oval"),
            );
            let t = Transform::from_rotate_at(*rotation as f32, *cx as f32, *cy as f32);
            pixmap.fill_path(
                &pb.finish().expect("path"),
                &paint,
                FillRule::Winding,
                t,
                None,
            );
        }
        Geometry::Polygon(points) => {
            pb.move_to(points[0].x as f32, points[0].y as f32);
            for p in &points[1..] {
                pb.line_to(p.x as f32, p.y as f32);
            }
            pb.close();
            if let Some(path) = pb.finish() {
                pixmap.fill_path(&path, &paint, FillRule::Winding, t, None);
            }
        }
        Geometry::Quadratic {
            start,
            control,
            end,
            width,
        } => {
            pb.move_to(start.x as f32, start.y as f32);
            pb.quad_to(
                control.x as f32,
                control.y as f32,
                end.x as f32,
                end.y as f32,
            );
            let stroke = Stroke {
                width: *width as f32,
                line_cap: LineCap::Butt,
                ..Stroke::default()
            };
            pixmap.stroke_path(&pb.finish().expect("path"), &paint, &stroke, t, None);
        }
    }
    // Strictly more than half: a pixel cut exactly in half (the signature of
    // a half-pixel offset on an axis-aligned edge) counts as uncovered
    // instead of falling on either side of a tie.
    pixmap.pixels().iter().map(|p| p.alpha() > 128).collect()
}

fn shift(geometry: &Geometry, dx: f64, dy: f64) -> Geometry {
    let p = |p: &Point| Point::new(p.x + dx, p.y + dy);
    match geometry {
        Geometry::Rect {
            x,
            y,
            width,
            height,
        } => Geometry::Rect {
            x: x + dx,
            y: y + dy,
            width: *width,
            height: *height,
        },
        Geometry::Ellipse {
            cx,
            cy,
            rx,
            ry,
            rotation,
        } => Geometry::Ellipse {
            cx: cx + dx,
            cy: cy + dy,
            rx: *rx,
            ry: *ry,
            rotation: *rotation,
        },
        Geometry::Polygon(points) => Geometry::Polygon(points.iter().map(p).collect()),
        Geometry::Quadratic {
            start,
            control,
            end,
            width,
        } => Geometry::Quadratic {
            start: p(start),
            control: p(control),
            end: p(end),
            width: *width,
        },
    }
}

fn perimeter(geometry: &Geometry) -> f64 {
    let dist = |a: &Point, b: &Point| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    match geometry {
        Geometry::Rect { width, height, .. } => 2.0 * (width + height),
        Geometry::Ellipse { rx, ry, .. } => {
            let h = ((rx - ry) / (rx + ry)).powi(2);
            std::f64::consts::PI * (rx + ry) * (1.0 + 3.0 * h / (10.0 + (4.0 - 3.0 * h).sqrt()))
        }
        Geometry::Polygon(points) => (0..points.len())
            .map(|i| dist(&points[i], &points[(i + 1) % points.len()]))
            .sum(),
        Geometry::Quadratic {
            start,
            control,
            end,
            width,
        } => 2.0 * (dist(start, control) + dist(control, end)) + 2.0 * width,
    }
}

const KINDS: [ShapeKind; 8] = [
    ShapeKind::Triangle,
    ShapeKind::Rectangle,
    ShapeKind::Ellipse,
    ShapeKind::Circle,
    ShapeKind::RotatedRectangle,
    ShapeKind::Quadratic,
    ShapeKind::RotatedEllipse,
    ShapeKind::Polygon,
];

/// Mismatched pixels per unit of perimeter, summed over seeded random shapes
/// of `kind`, with the geometry shifted by `(dx, dy)`.
fn mismatch_per_perimeter(kind: ShapeKind, dx: f64, dy: f64) -> f64 {
    let (mut worker, round) = make_test_round(W, H, 11);
    let (mut mismatched, mut perimeter_total) = (0usize, 0.0);
    for _ in 0..SHAPES_PER_KIND {
        let mut shape = Shape::random(kind, &mut worker, &round);
        if let Shape::Quadratic(quadratic) = &mut shape {
            // The engine's 0.5-wide stroke never covers half a pixel in
            // the geometry, so every pixel would sit at the threshold. The
            // width is carried through unchanged; a wider stroke tests the
            // same centre-line convention without ties.
            quadratic.width = 3.0;
        }
        let engine = engine_mask(&shape, &mut worker);
        let geometry = shape.geometry();
        let drawn = geometry_mask(&shift(&geometry, dx, dy));
        mismatched += engine.iter().zip(&drawn).filter(|(a, b)| a != b).count();
        perimeter_total += perimeter(&geometry);
    }
    mismatched as f64 / perimeter_total
}

const SHAPES_PER_KIND: usize = 200;

/// Upper bound on mismatched pixels per unit of perimeter.
///
/// Engine and geometry masks can only disagree along the anti-aliased
/// boundary, so the mismatch count grows with the perimeter, not the area;
/// normalising by the perimeter makes one bound fit small and large shapes.
/// For kinds whose rasterizer samples the shape exactly, a correct mapping
/// stays below 0.06 and a half-pixel offset costs at least 0.16, so 0.1
/// separates them. Three rasterizers are coarser than their geometry, and
/// their bounds sit about 10% above the measured value:
/// - `Triangle` (0.456): rows are filled between truncated edge crossings
///   (about one extra pixel on the left of every row) and the lower half is
///   sampled one row early, so it is drawn one row lower.
/// - `RotatedRectangle` (0.265): corners are truncated towards the centre,
///   then every pixel an edge touches is filled, which fattens the shape.
/// - `Quadratic` (0.157): each flat segment is sampled at the left edge of
///   its columns (or the top edge of its rows, when steep) instead of at the
///   pixel centre, a half-pixel shift along the segment's major axis.
fn bound(kind: ShapeKind) -> f64 {
    match kind {
        ShapeKind::Triangle => 0.5,
        ShapeKind::RotatedRectangle => 0.29,
        ShapeKind::Quadratic => 0.175,
        _ => 0.1,
    }
}

const HALF_PIXEL_SHIFTS: [(f64, f64); 4] = [(0.5, 0.0), (-0.5, 0.0), (0.0, 0.5), (0.0, -0.5)];

/// Half-pixel offsets the coarse rasterizers cannot tell apart from the
/// mapping: their own bias is about as large (see [`bound`]).
const UNDETECTABLE_SHIFTS: [(ShapeKind, f64, f64); 3] = [
    (ShapeKind::Triangle, -0.5, 0.0),
    (ShapeKind::Triangle, 0.0, 0.5),
    (ShapeKind::Quadratic, 0.5, 0.0),
];

#[test]
fn geometry_covers_the_pixels_the_engine_rasterizes() {
    for kind in KINDS {
        let measured = mismatch_per_perimeter(kind, 0.0, 0.0);
        assert!(
            measured <= bound(kind),
            "{kind:?}: {measured:.3} mismatched pixels per unit of perimeter, bound {}",
            bound(kind)
        );
    }
}

#[test]
fn half_pixel_offsets_exceed_the_bound() {
    for kind in KINDS {
        for (dx, dy) in HALF_PIXEL_SHIFTS {
            if UNDETECTABLE_SHIFTS.contains(&(kind, dx, dy)) {
                continue;
            }
            let measured = mismatch_per_perimeter(kind, dx, dy);
            assert!(
                measured > bound(kind),
                "{kind:?} shifted by ({dx}, {dy}): {measured:.3} is within bound {}",
                bound(kind)
            );
        }
    }
}

#[test]
fn quadratic_geometry_keeps_the_working_width() {
    let shape = Shape::Quadratic(Quadratic {
        x1: 1.0,
        y1: 2.0,
        x2: 3.0,
        y2: 4.0,
        x3: 5.0,
        y3: 6.0,
        width: 0.5,
    });
    assert_eq!(
        shape.geometry(),
        Geometry::Quadratic {
            start: Point::new(1.0, 2.0),
            control: Point::new(3.0, 4.0),
            end: Point::new(5.0, 6.0),
            width: 0.5,
        }
    );
}

#[test]
fn rectangle_geometry_covers_inclusive_pixel_bounds() {
    let shape = Shape::Rectangle(Rectangle {
        x1: 4,
        y1: 3,
        x2: 2,
        y2: 1,
    });
    assert_eq!(
        shape.geometry(),
        Geometry::Rect {
            x: 2.0,
            y: 1.0,
            width: 3.0,
            height: 3.0,
        }
    );
}

#[test]
fn circle_geometry_is_centred_on_the_pixel() {
    let shape = Shape::Circle(Circle { x: 10, y: 20, r: 7 });
    assert_eq!(
        shape.geometry(),
        Geometry::Ellipse {
            cx: 10.5,
            cy: 20.5,
            rx: 7.0,
            ry: 7.0,
            rotation: 0.0,
        }
    );
}
