use crate::drawing::{Geometry, Point};
use crate::error::ParseError;
use crate::scanline::Scanline;
use crate::util::{degrees, radians, rotate_sc};
use crate::worker::{SearchRound, WorkerCtx};
use rand::{Rng, RngExt};
use rand_distr::{Distribution, StandardNormal};
use std::str::FromStr;

const POSITION_SIGMA: f64 = 16.0;
const ANGLE_SIGMA: f64 = 32.0;

/// The shape family the search draws from.
///
/// [`ShapeKind::Any`] picks a concrete family at random for each candidate.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Any,
    Triangle,
    Rectangle,
    Ellipse,
    Circle,
    RotatedRectangle,
    Quadratic,
    RotatedEllipse,
    Polygon,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shape {
    Triangle(Triangle),
    Rectangle(Rectangle),
    Ellipse(Ellipse),
    Circle(Circle),
    RotatedRectangle(RotatedRectangle),
    Quadratic(Quadratic),
    RotatedEllipse(RotatedEllipse),
    Polygon(Polygon),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Triangle {
    pub(crate) x1: i32,
    pub(crate) y1: i32,
    pub(crate) x2: i32,
    pub(crate) y2: i32,
    pub(crate) x3: i32,
    pub(crate) y3: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Rectangle {
    pub(crate) x1: i32,
    pub(crate) y1: i32,
    pub(crate) x2: i32,
    pub(crate) y2: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Ellipse {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) rx: i32,
    pub(crate) ry: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Circle {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) r: i32,
}

/// A rectangle of `sx` × `sy` rotated by `angle` degrees about `(x, y)`.
///
/// Unlike Go's `primitive`, it intentionally has no aspect-ratio limit:
/// enforcing Go's limit (long side at most 5 × the short side) measurably
/// hurt quality, for example the synthetic-texture rotated-rectangle score
/// got 44% worse at 100 steps and 20% worse at 200.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RotatedRectangle {
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) sx: i32,
    pub(crate) sy: i32,
    pub(crate) angle: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Quadratic {
    pub(crate) x1: f64,
    pub(crate) y1: f64,
    pub(crate) x2: f64,
    pub(crate) y2: f64,
    pub(crate) x3: f64,
    pub(crate) y3: f64,
    pub(crate) width: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RotatedEllipse {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) rx: f64,
    pub(crate) ry: f64,
    pub(crate) angle: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Polygon {
    pub(crate) order: usize,
    pub(crate) x: [f64; 4],
    pub(crate) y: [f64; 4],
}

impl Shape {
    #[must_use]
    pub(crate) fn random<R: Rng>(
        kind: ShapeKind,
        worker: &mut WorkerCtx<R>,
        round: &SearchRound<'_>,
    ) -> Self {
        let kind = match kind {
            ShapeKind::Any => {
                ShapeKind::all_kinds()[worker.rng.random_range(0..ShapeKind::all_kinds().len())]
            }
            other => other,
        };

        match kind {
            ShapeKind::Triangle => Self::Triangle(Triangle::random(worker, round)),
            ShapeKind::Rectangle => Self::Rectangle(Rectangle::random(worker, round)),
            ShapeKind::Ellipse => Self::Ellipse(Ellipse::random(worker, round)),
            ShapeKind::Circle => Self::Circle(Circle::random(worker, round)),
            ShapeKind::RotatedRectangle => {
                Self::RotatedRectangle(RotatedRectangle::random(worker, round))
            }
            ShapeKind::Quadratic => Self::Quadratic(Quadratic::random(worker, round)),
            ShapeKind::RotatedEllipse => {
                Self::RotatedEllipse(RotatedEllipse::random(worker, round))
            }
            ShapeKind::Polygon => Self::Polygon(Polygon::random(worker, round, 4)),
            ShapeKind::Any => unreachable!("ShapeKind::Any is resolved before shape creation"),
        }
    }

    pub(crate) fn rasterize<'a, R: Rng>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        match self {
            Self::Triangle(shape) => shape.rasterize(worker),
            Self::Rectangle(shape) => shape.rasterize(worker),
            Self::Ellipse(shape) => shape.rasterize(worker),
            Self::Circle(shape) => shape.rasterize(worker),
            Self::RotatedRectangle(shape) => shape.rasterize(worker),
            Self::Quadratic(shape) => shape.rasterize(worker),
            Self::RotatedEllipse(shape) => shape.rasterize(worker),
            Self::Polygon(shape) => shape.rasterize(worker),
        }
    }

    pub(crate) fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        match self {
            Self::Triangle(shape) => shape.mutate(worker),
            Self::Rectangle(shape) => shape.mutate(worker),
            Self::Ellipse(shape) => shape.mutate(worker),
            Self::Circle(shape) => shape.mutate(worker),
            Self::RotatedRectangle(shape) => shape.mutate(worker),
            Self::Quadratic(shape) => shape.mutate(worker),
            Self::RotatedEllipse(shape) => shape.mutate(worker),
            Self::Polygon(shape) => shape.mutate(worker),
        }
    }

    /// The shape's geometry in continuous canvas coordinates, where pixel
    /// `(i, j)` is the unit square `[i, i + 1) × [j, j + 1)`.
    ///
    /// This is the single place where the engine's pixel convention is
    /// mapped onto output geometry. Each mapping follows the kind's
    /// working-resolution rasterizer, so the geometry covers the pixels the
    /// engine optimised.
    #[must_use]
    pub(crate) fn geometry(&self) -> Geometry {
        match self {
            Self::Triangle(shape) => shape.geometry(),
            Self::Rectangle(shape) => shape.geometry(),
            Self::Ellipse(shape) => ellipse_geometry(shape.x, shape.y, shape.rx, shape.ry),
            Self::Circle(shape) => ellipse_geometry(shape.x, shape.y, shape.r, shape.r),
            Self::RotatedRectangle(shape) => shape.geometry(),
            Self::Quadratic(shape) => shape.geometry(),
            Self::RotatedEllipse(shape) => shape.geometry(),
            Self::Polygon(shape) => shape.geometry(),
        }
    }
}

impl ShapeKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            ShapeKind::Any => "any",
            ShapeKind::Triangle => "triangle",
            ShapeKind::Rectangle => "rectangle",
            ShapeKind::Ellipse => "ellipse",
            ShapeKind::Circle => "circle",
            ShapeKind::RotatedRectangle => "rotated-rectangle",
            ShapeKind::Quadratic => "quadratic",
            ShapeKind::RotatedEllipse => "rotated-ellipse",
            ShapeKind::Polygon => "polygon",
        }
    }

    pub(crate) const fn all_kinds() -> &'static [ShapeKind] {
        &[
            ShapeKind::Triangle,
            ShapeKind::Rectangle,
            ShapeKind::Ellipse,
            ShapeKind::Circle,
            ShapeKind::RotatedRectangle,
            ShapeKind::Quadratic,
            ShapeKind::RotatedEllipse,
            ShapeKind::Polygon,
        ]
    }
}

impl FromStr for ShapeKind {
    type Err = ParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "any" => Ok(Self::Any),
            "triangle" => Ok(Self::Triangle),
            "rectangle" => Ok(Self::Rectangle),
            "ellipse" => Ok(Self::Ellipse),
            "circle" => Ok(Self::Circle),
            "rotated-rectangle" => Ok(Self::RotatedRectangle),
            "quadratic" => Ok(Self::Quadratic),
            "rotated-ellipse" => Ok(Self::RotatedEllipse),
            "polygon" => Ok(Self::Polygon),
            _ => Err(ParseError::new(
                "shape must be one of: any, triangle, rectangle, ellipse, circle, \
                 rotated-rectangle, quadratic, rotated-ellipse, polygon",
            )),
        }
    }
}

impl Triangle {
    /// Integer vertices are pixel centres (the rasterizer samples each row at
    /// its integer `y`), so they map to `v + 0.5`.
    fn geometry(&self) -> Geometry {
        Geometry::Polygon(vec![
            pixel_centre(self.x1, self.y1),
            pixel_centre(self.x2, self.y2),
            pixel_centre(self.x3, self.y3),
        ])
    }

    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> Self {
        let (x1, y1) = worker.sample_xy(round);
        let x2 = x1 + worker.rng.random_range(0..31) - 15;
        let y2 = y1 + worker.rng.random_range(0..31) - 15;
        let x3 = x1 + worker.rng.random_range(0..31) - 15;
        let y3 = y1 + worker.rng.random_range(0..31) - 15;
        let mut triangle = Self {
            x1,
            y1,
            x2,
            y2,
            x3,
            y3,
        };
        triangle.mutate(worker);
        triangle
    }

    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        const MIN_DEGREES: f64 = 15.0;

        fn angle(ax: i32, ay: i32, bx: i32, by: i32) -> Option<f64> {
            let ax = ax as f64;
            let ay = ay as f64;
            let bx = bx as f64;
            let by = by as f64;
            let da = (ax * ax + ay * ay).sqrt();
            let db = (bx * bx + by * by).sqrt();
            if da == 0.0 || db == 0.0 {
                return None;
            }
            let dot = ((ax / da) * (bx / db) + (ay / da) * (by / db)).clamp(-1.0, 1.0);
            Some(degrees(dot.acos()))
        }

        let Some(a1) = angle(
            self.x2 - self.x1,
            self.y2 - self.y1,
            self.x3 - self.x1,
            self.y3 - self.y1,
        ) else {
            return false;
        };
        let Some(a2) = angle(
            self.x1 - self.x2,
            self.y1 - self.y2,
            self.x3 - self.x2,
            self.y3 - self.y2,
        ) else {
            return false;
        };
        let a3 = 180.0 - a1 - a2;
        a1 > MIN_DEGREES && a2 > MIN_DEGREES && a3 > MIN_DEGREES
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        worker.lines.clear();
        rasterize_triangle(
            [(self.x1, self.y1), (self.x2, self.y2), (self.x3, self.y3)],
            worker.height,
            &mut worker.lines,
        );
        crate::scanline::crop_scanlines(&mut worker.lines, worker.width, worker.height);
        &worker.lines
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        const MARGIN: i32 = 16;
        loop {
            match worker.rng.random_range(0..3) {
                0 => {
                    self.x1 = (self.x1 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                        .clamp(-MARGIN, worker.width - 1 + MARGIN);
                    self.y1 = (self.y1 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                        .clamp(-MARGIN, worker.height - 1 + MARGIN);
                }
                1 => {
                    self.x2 = (self.x2 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                        .clamp(-MARGIN, worker.width - 1 + MARGIN);
                    self.y2 = (self.y2 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                        .clamp(-MARGIN, worker.height - 1 + MARGIN);
                }
                _ => {
                    self.x3 = (self.x3 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                        .clamp(-MARGIN, worker.width - 1 + MARGIN);
                    self.y3 = (self.y3 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                        .clamp(-MARGIN, worker.height - 1 + MARGIN);
                }
            }
            if self.is_valid() {
                break;
            }
        }
    }
}

impl Rectangle {
    /// Inclusive pixel bounds `x1..=x2` cover `[x1, x2 + 1)`.
    fn geometry(&self) -> Geometry {
        let (x1, y1, x2, y2) = self.bounds();
        Geometry::Rect {
            x: f64::from(x1),
            y: f64::from(y1),
            width: f64::from(x2 - x1 + 1),
            height: f64::from(y2 - y1 + 1),
        }
    }

    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> Self {
        let (x1, y1) = worker.sample_xy(round);
        let x2 = (x1 + worker.rng.random_range(1..33)).clamp(0, worker.width - 1);
        let y2 = (y1 + worker.rng.random_range(1..33)).clamp(0, worker.height - 1);
        Self { x1, y1, x2, y2 }
    }

    #[must_use]
    fn bounds(&self) -> (i32, i32, i32, i32) {
        let (mut x1, mut y1, mut x2, mut y2) = (self.x1, self.y1, self.x2, self.y2);
        if x1 > x2 {
            std::mem::swap(&mut x1, &mut x2);
        }
        if y1 > y2 {
            std::mem::swap(&mut y1, &mut y2);
        }
        (x1, y1, x2, y2)
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        let (x1, y1, x2, y2) = self.bounds();
        worker.lines.clear();
        for y in y1..=y2 {
            worker.lines.push(Scanline {
                y,
                x1,
                x2,
                alpha: 0xFFFF,
            });
        }
        &worker.lines
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        match worker.rng.random_range(0..2) {
            0 => {
                self.x1 = (self.x1 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.width - 1);
                self.y1 = (self.y1 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.height - 1);
            }
            _ => {
                self.x2 = (self.x2 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.width - 1);
                self.y2 = (self.y2 + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.height - 1);
            }
        }
    }
}

impl Ellipse {
    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> Self {
        let (x, y) = worker.sample_xy(round);
        Self {
            x,
            y,
            rx: worker.rng.random_range(1..33),
            ry: worker.rng.random_range(1..33),
        }
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        rasterize_ellipse(worker, self.x, self.y, self.rx, self.ry)
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        match worker.rng.random_range(0..3) {
            0 => {
                self.x = (self.x + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.width - 1);
                self.y = (self.y + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.height - 1);
            }
            1 => {
                self.rx = (self.rx + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(1, worker.width - 1)
            }
            _ => {
                self.ry = (self.ry + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(1, worker.height - 1)
            }
        }
    }
}

impl Circle {
    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> Self {
        let (x, y) = worker.sample_xy(round);
        Self {
            x,
            y,
            r: worker.rng.random_range(1..33),
        }
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        rasterize_ellipse(worker, self.x, self.y, self.r, self.r)
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        match worker.rng.random_range(0..3) {
            0 => {
                self.x = (self.x + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.width - 1);
                self.y = (self.y + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.height - 1);
            }
            _ => {
                self.r = (self.r + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(1, worker.width.min(worker.height) - 1)
            }
        }
    }
}

impl RotatedRectangle {
    /// `(x, y)` is a continuous coordinate, and the rasterizer fills the
    /// pixels whose centres lie inside the exact rotated corners.
    fn geometry(&self) -> Geometry {
        Geometry::Polygon(
            self.corners()
                .into_iter()
                .map(|(x, y)| Point::new(x, y))
                .collect(),
        )
    }

    /// The exact corners, rotated by `angle` degrees about `(x, y)`.
    fn corners(&self) -> [(f64, f64); 4] {
        let half_x = f64::from(self.sx) / 2.0;
        let half_y = f64::from(self.sy) / 2.0;
        let (sin_a, cos_a) = radians(f64::from(self.angle)).sin_cos();
        [
            (-half_x, -half_y),
            (half_x, -half_y),
            (half_x, half_y),
            (-half_x, half_y),
        ]
        .map(|(x, y)| {
            let (rx, ry) = rotate_sc(x, y, sin_a, cos_a);
            (rx + f64::from(self.x), ry + f64::from(self.y))
        })
    }

    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> Self {
        let (x, y) = worker.sample_xy(round);
        let mut rect = Self {
            x,
            y,
            sx: worker.rng.random_range(1..33),
            sy: worker.rng.random_range(1..33),
            angle: worker.rng.random_range(0..360),
        };
        rect.mutate(worker);
        rect
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        worker.lines.clear();
        crate::raster::fill_convex_at_pixel_centres(
            &mut worker.lines,
            &self.corners(),
            worker.width,
            worker.height,
        );
        &worker.lines
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        match worker.rng.random_range(0..3) {
            0 => {
                self.x = (self.x + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.width - 1);
                self.y = (self.y + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(0, worker.height - 1);
            }
            1 => {
                self.sx = (self.sx + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(1, worker.width - 1);
                self.sy = (self.sy + gaussian_sample(&mut worker.rng, POSITION_SIGMA) as i32)
                    .clamp(1, worker.height - 1);
            }
            _ => self.angle += gaussian_sample(&mut worker.rng, ANGLE_SIGMA) as i32,
        }
    }
}

impl Quadratic {
    const MUTATE_MARGIN: f64 = 16.0;
    const MAX_MUTATE_ATTEMPTS: u32 = 6;

    /// The stroke rasterizer measures distances in continuous coordinates,
    /// so the control points map unchanged.
    fn geometry(&self) -> Geometry {
        Geometry::Quadratic {
            start: Point::new(self.x1, self.y1),
            control: Point::new(self.x2, self.y2),
            end: Point::new(self.x3, self.y3),
            width: self.width,
        }
    }

    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> Self {
        let (x1, y1) = worker.sample_xy_float(round);
        let x2 = x1 + worker.rng.random::<f64>() * 40.0 - 20.0;
        let y2 = y1 + worker.rng.random::<f64>() * 40.0 - 20.0;
        let x3 = x2 + worker.rng.random::<f64>() * 40.0 - 20.0;
        let y3 = y2 + worker.rng.random::<f64>() * 40.0 - 20.0;
        let mut quadratic = Self {
            x1,
            y1,
            x2,
            y2,
            x3,
            y3,
            width: 0.5,
        };
        quadratic.mutate(worker);
        quadratic
    }

    #[must_use]
    pub(crate) fn is_valid(&self) -> bool {
        let dx12 = self.x1 - self.x2;
        let dy12 = self.y1 - self.y2;
        let dx23 = self.x2 - self.x3;
        let dy23 = self.y2 - self.y3;
        let dx13 = self.x1 - self.x3;
        let dy13 = self.y1 - self.y3;
        let d12 = dx12 * dx12 + dy12 * dy12;
        let d23 = dx23 * dx23 + dy23 * dy23;
        let d13 = dx13 * dx13 + dy13 * dy13;
        d13 > d12 && d13 > d23
    }

    /// Repair an endpoint mutation by binary-lerping between the old and new
    /// endpoint positions while the old shape remains a valid anchor.
    ///
    /// Falls back to control-point repair if the old state was also invalid
    /// (e.g. during `random()` initialization).
    ///
    /// `choice`: 0 = p1, 2 = p3.
    fn repair_endpoint(&mut self, old: &Quadratic, choice: u32, width: i32, height: i32) {
        // If the old state was already invalid, endpoint lerp has no valid
        // anchor — fall back to control-point repair.
        if !old.is_valid() {
            self.repair_control_point(width, height);
            return;
        }

        let min_coord = -Self::MUTATE_MARGIN;
        let max_x = f64::from(width - 1) + Self::MUTATE_MARGIN;
        let max_y = f64::from(height - 1) + Self::MUTATE_MARGIN;

        let (old_x, old_y, new_x, new_y) = match choice {
            0 => (old.x1, old.y1, self.x1, self.y1),
            _ => (old.x3, old.y3, self.x3, self.y3),
        };

        // Binary search: lo=0 is the old position (valid), hi=1 is the new (invalid).
        let mut lo = 0.0_f64;
        let mut hi = 1.0_f64;

        for _ in 0..10 {
            let mid = (lo + hi) * 0.5;
            let test_x = (old_x + (new_x - old_x) * mid).clamp(min_coord, max_x);
            let test_y = (old_y + (new_y - old_y) * mid).clamp(min_coord, max_y);

            let mut test = *self;
            match choice {
                0 => {
                    test.x1 = test_x;
                    test.y1 = test_y;
                }
                _ => {
                    test.x3 = test_x;
                    test.y3 = test_y;
                }
            }

            if test.is_valid() {
                lo = mid;
            } else {
                hi = mid;
            }
        }

        let final_x = (old_x + (new_x - old_x) * lo).clamp(min_coord, max_x);
        let final_y = (old_y + (new_y - old_y) * lo).clamp(min_coord, max_y);
        match choice {
            0 => {
                self.x1 = final_x;
                self.y1 = final_y;
            }
            _ => {
                self.x3 = final_x;
                self.y3 = final_y;
            }
        }

        if !self.is_valid() {
            self.repair_control_point(width, height);
        }
    }

    /// Repair a control-point mutation by projecting the control point back
    /// inside the validity region defined by the current endpoints.
    /// Also ensures the chord is at least 2px long (spreading endpoints if needed).
    fn repair_control_point(&mut self, width: i32, height: i32) {
        let min_coord = -Self::MUTATE_MARGIN;
        let max_x = f64::from(width - 1) + Self::MUTATE_MARGIN;
        let max_y = f64::from(height - 1) + Self::MUTATE_MARGIN;

        let mut dx = self.x3 - self.x1;
        let mut dy = self.y3 - self.y1;
        let mut length = (dx * dx + dy * dy).sqrt();

        // Ensure endpoints are far enough apart for a meaningful chord.
        if length < 2.0 {
            if self.x1 + 2.0 <= max_x {
                self.x3 = self.x1 + 2.0;
                self.y3 = self.y1;
            } else if self.x1 - 2.0 >= min_coord {
                self.x3 = self.x1 - 2.0;
                self.y3 = self.y1;
            } else if self.y1 + 2.0 <= max_y {
                self.x3 = self.x1;
                self.y3 = self.y1 + 2.0;
            } else {
                self.x3 = self.x1;
                self.y3 = self.y1 - 2.0;
            }
            dx = self.x3 - self.x1;
            dy = self.y3 - self.y1;
            length = (dx * dx + dy * dy).sqrt();
        }

        let mid_x = (self.x1 + self.x3) * 0.5;
        let mid_y = (self.y1 + self.y3) * 0.5;

        let ux = dx / length;
        let uy = dy / length;
        let px = -uy;
        let py = ux;
        let rel_x = self.x2 - mid_x;
        let rel_y = self.y2 - mid_y;
        let tangent = rel_x * ux + rel_y * uy;
        let normal = rel_x * px + rel_y * py;
        let tangent_ratio = tangent.abs() / length;
        let normal_ratio = normal.abs() / length;
        let ratio_sq = tangent_ratio * tangent_ratio + normal_ratio * normal_ratio;

        let scale = if ratio_sq > 0.0 {
            let boundary = tangent_ratio * tangent_ratio + 3.0 * ratio_sq;
            ((-tangent_ratio + boundary.sqrt()) / (2.0 * ratio_sq)).min(1.0)
        } else {
            0.0
        };
        self.x2 = (mid_x + tangent * scale * ux + normal * scale * px).clamp(min_coord, max_x);
        self.y2 = (mid_y + tangent * scale * uy + normal * scale * py).clamp(min_coord, max_y);

        if !self.is_valid() {
            self.x2 = mid_x.clamp(min_coord, max_x);
            self.y2 = mid_y.clamp(min_coord, max_y);
        }
        if !self.is_valid() {
            self.force_valid_geometry(width, height);
        }
    }

    fn force_valid_geometry(&mut self, width: i32, height: i32) {
        let min_coord = -Self::MUTATE_MARGIN;
        let max_x = f64::from(width - 1) + Self::MUTATE_MARGIN;
        let max_y = f64::from(height - 1) + Self::MUTATE_MARGIN;

        self.x1 = 0.0_f64.clamp(min_coord, max_x);
        self.y1 = 0.0_f64.clamp(min_coord, max_y);
        self.x3 = (self.x1 + 2.0).clamp(min_coord, max_x);
        self.y3 = self.y1;

        if (self.x3 - self.x1).abs() < 2.0 {
            self.x3 = self.x1;
            self.y3 = (self.y1 + 2.0).clamp(min_coord, max_y);
        }

        self.x2 = ((self.x1 + self.x3) * 0.5).clamp(min_coord, max_x);
        self.y2 = ((self.y1 + self.y3) * 0.5).clamp(min_coord, max_y);
    }

    fn rasterize<'a, R: Rng>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        crate::raster::stroke_quadratic_direct(
            worker,
            self.x1,
            self.y1,
            self.x2,
            self.y2,
            self.x3,
            self.y3,
            self.width / 2.0,
        )
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        let min_coord = -Self::MUTATE_MARGIN;
        let max_x = f64::from(worker.width - 1) + Self::MUTATE_MARGIN;
        let max_y = f64::from(worker.height - 1) + Self::MUTATE_MARGIN;
        let old = *self;
        let choice = worker.rng.random_range(0..3u32);

        for _ in 0..Self::MAX_MUTATE_ATTEMPTS {
            match choice {
                0 => {
                    self.x1 = (self.x1 + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                        .clamp(min_coord, max_x);
                    self.y1 = (self.y1 + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                        .clamp(min_coord, max_y);
                }
                1 => {
                    self.x2 = (self.x2 + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                        .clamp(min_coord, max_x);
                    self.y2 = (self.y2 + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                        .clamp(min_coord, max_y);
                }
                _ => {
                    self.x3 = (self.x3 + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                        .clamp(min_coord, max_x);
                    self.y3 = (self.y3 + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                        .clamp(min_coord, max_y);
                }
            }

            if self.is_valid() {
                return;
            }
        }

        match choice {
            0 | 2 => self.repair_endpoint(&old, choice, worker.width, worker.height),
            _ => self.repair_control_point(worker.width, worker.height),
        }
        debug_assert!(self.is_valid());
    }
}

impl RotatedEllipse {
    /// The rasterizer works in continuous coordinates, so the centre maps
    /// unchanged.
    fn geometry(&self) -> Geometry {
        Geometry::Ellipse {
            cx: self.x,
            cy: self.y,
            rx: self.rx,
            ry: self.ry,
            rotation: self.angle,
        }
    }

    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>) -> Self {
        let (x, y) = worker.sample_xy_float(round);
        Self {
            x,
            y,
            rx: worker.rng.random::<f64>() * 32.0 + 1.0,
            ry: worker.rng.random::<f64>() * 32.0 + 1.0,
            angle: worker.rng.random::<f64>() * 360.0,
        }
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        crate::raster::fill_rotated_ellipse_direct(
            &mut worker.lines,
            self.x,
            self.y,
            self.rx,
            self.ry,
            radians(self.angle),
            worker.width,
            worker.height,
        );
        &worker.lines
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        match worker.rng.random_range(0..3) {
            0 => {
                self.x = (self.x + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                    .clamp(0.0, f64::from(worker.width - 1));
                self.y = (self.y + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                    .clamp(0.0, f64::from(worker.height - 1));
            }
            1 => {
                self.rx = (self.rx + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                    .clamp(1.0, f64::from(worker.width - 1));
                self.ry = (self.ry + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                    .clamp(1.0, f64::from(worker.height - 1));
            }
            _ => self.angle += gaussian_sample(&mut worker.rng, ANGLE_SIGMA),
        }
    }
}

impl Polygon {
    /// The rasterizer works in continuous coordinates, so vertices map
    /// unchanged.
    fn geometry(&self) -> Geometry {
        Geometry::Polygon(
            (0..self.order)
                .map(|i| Point::new(self.x[i], self.y[i]))
                .collect(),
        )
    }

    fn random<R: Rng>(worker: &mut WorkerCtx<R>, round: &SearchRound<'_>, order: usize) -> Self {
        let mut x = [0.0; 4];
        let mut y = [0.0; 4];
        let (x0, y0) = worker.sample_xy_float(round);
        x[0] = x0;
        y[0] = y0;
        for i in 1..order {
            x[i] = x0 + worker.rng.random::<f64>() * 40.0 - 20.0;
            y[i] = y0 + worker.rng.random::<f64>() * 40.0 - 20.0;
        }
        let mut polygon = Self { order, x, y };
        polygon.mutate(worker);
        polygon
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        let vertices: [(f64, f64); 4] = [
            (self.x[0], self.y[0]),
            (self.x[1], self.y[1]),
            (self.x[2], self.y[2]),
            (self.x[3], self.y[3]),
        ];
        crate::raster::fill_polygon_direct(
            &mut worker.lines,
            &vertices[..self.order],
            worker.width,
            worker.height,
        );
        &worker.lines
    }

    fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>) {
        const MARGIN: f64 = 16.0;
        if worker.rng.random::<f64>() < 0.25 {
            let i = worker.rng.random_range(0..self.order);
            let j = worker.rng.random_range(0..self.order);
            self.x.swap(i, j);
            self.y.swap(i, j);
        } else {
            let i = worker.rng.random_range(0..self.order);
            self.x[i] = (self.x[i] + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                .clamp(-MARGIN, f64::from(worker.width - 1) + MARGIN);
            self.y[i] = (self.y[i] + gaussian_sample(&mut worker.rng, POSITION_SIGMA))
                .clamp(-MARGIN, f64::from(worker.height - 1) + MARGIN);
        }
    }
}

fn gaussian_sample<R: Rng>(rng: &mut R, sigma: f64) -> f64 {
    let sample: f64 = StandardNormal.sample(rng);
    sample * sigma
}

/// The integer-centred ellipse rasterizer treats `(x, y)` as a pixel centre
/// and keeps the pixels whose centres fall inside the radii.
fn ellipse_geometry(x: i32, y: i32, rx: i32, ry: i32) -> Geometry {
    let centre = pixel_centre(x, y);
    Geometry::Ellipse {
        cx: centre.x,
        cy: centre.y,
        rx: f64::from(rx),
        ry: f64::from(ry),
        rotation: 0.0,
    }
}

/// Maps an integer pixel coordinate to the centre of that pixel.
fn pixel_centre(x: i32, y: i32) -> Point {
    Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5)
}

fn rasterize_ellipse<R>(
    worker: &mut WorkerCtx<R>,
    x: i32,
    y: i32,
    rx: i32,
    ry: i32,
) -> &[Scanline] {
    worker.lines.clear();
    let aspect = rx as f64 / ry as f64;
    for dy in 0..ry {
        let y1 = y - dy;
        let y2 = y + dy;
        if (y1 < 0 || y1 >= worker.height) && (y2 < 0 || y2 >= worker.height) {
            continue;
        }
        let span = (((ry * ry - dy * dy) as f64).sqrt() * aspect) as i32;
        let x1 = (x - span).max(0);
        let x2 = (x + span).min(worker.width - 1);
        if y1 >= 0 && y1 < worker.height {
            worker.lines.push(Scanline {
                y: y1,
                x1,
                x2,
                alpha: 0xFFFF,
            });
        }
        if y2 >= 0 && y2 < worker.height && dy > 0 {
            worker.lines.push(Scanline {
                y: y2,
                x1,
                x2,
                alpha: 0xFFFF,
            });
        }
    }
    &worker.lines
}

/// Fills a triangle whose integer vertices are pixel centres.
///
/// Each row is sampled at its pixel centres: row `y` covers the pixels whose
/// centres lie inside the triangle. The edge crossings are exact rationals,
/// so the rounding is exact too. A centre exactly on an edge belongs to the
/// triangle only on its left or top edges (the usual top-left rule), so a
/// pixel cut in half by an edge is counted on one side, not both. Only rows
/// inside `0..height` are emitted; columns are left for the caller to crop.
fn rasterize_triangle(mut vertices: [(i32, i32); 3], height: i32, lines: &mut Vec<Scanline>) {
    vertices.sort_unstable_by_key(|&(_, y)| y);
    let [top, middle, bottom] = vertices;
    let (start, end) = (top.1.max(0), bottom.1.min(height));
    if start >= end {
        // Off the canvas, or collinear vertices on one row (which
        // `Triangle::is_valid` rejects).
        return;
    }
    let mut long = EdgeWalker::new(top, bottom, start);
    let upper = top.1 < middle.1 && start <= middle.1;
    let mut short = if upper {
        EdgeWalker::new(top, middle, start)
    } else {
        EdgeWalker::new(middle, bottom, start)
    };
    for y in start..end {
        if upper && y == middle.1 + 1 {
            short = EdgeWalker::new(middle, bottom, y);
        }
        // Left crossings round up; a centre exactly on the right edge is out.
        let x1 = long.first.min(short.first) as i32;
        let x2 = (long.first.max(short.first) - 1) as i32;
        if x1 <= x2 {
            lines.push(Scanline {
                y,
                x1,
                x2,
                alpha: 0xFFFF,
            });
        }
        long.step();
        short.step();
    }
}

/// Walks the edge from `a` to `b` (with `a.1 < b.1`) row by row, tracking
/// the first column at or right of its crossing exactly: the crossing is
/// `numerator / dy`, and `first · dy - numerator` stays in `0..dy`.
struct EdgeWalker {
    first: i64,
    excess: i64,
    dy: i64,
    step_quotient: i64,
    step_remainder: i64,
}

impl EdgeWalker {
    fn new(a: (i32, i32), b: (i32, i32), y: i32) -> Self {
        let dy = i64::from(b.1 - a.1);
        let dx = i64::from(b.0 - a.0);
        let numerator = i64::from(a.0) * dy + i64::from(y - a.1) * dx;
        // The crossing's ceiling, from an exact floor division.
        let first = -(-numerator).div_euclid(dy);
        Self {
            first,
            excess: first * dy - numerator,
            dy,
            step_quotient: dx.div_euclid(dy),
            step_remainder: dx.rem_euclid(dy),
        }
    }

    /// Moves to the next row, where the numerator grows by `dx`.
    #[inline]
    fn step(&mut self) {
        self.first += self.step_quotient;
        self.excess -= self.step_remainder;
        if self.excess < 0 {
            self.first += 1;
            self.excess += self.dy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::make_test_round;

    fn round(w: u32, h: u32) -> (WorkerCtx<rand_chacha::ChaCha8Rng>, SearchRound<'static>) {
        make_test_round(w, h, 7)
    }

    #[test]
    fn shape_kind_round_trips_public_names() {
        let cases = [
            (ShapeKind::Any, "any"),
            (ShapeKind::Triangle, "triangle"),
            (ShapeKind::Rectangle, "rectangle"),
            (ShapeKind::Ellipse, "ellipse"),
            (ShapeKind::Circle, "circle"),
            (ShapeKind::RotatedRectangle, "rotated-rectangle"),
            (ShapeKind::Quadratic, "quadratic"),
            (ShapeKind::RotatedEllipse, "rotated-ellipse"),
            (ShapeKind::Polygon, "polygon"),
        ];

        for (kind, value) in cases {
            assert_eq!(kind.as_str(), value);
            assert_eq!(value.parse::<ShapeKind>().expect("shape kind"), kind);
        }
    }

    #[test]
    fn shape_kind_rejects_unknown_name() {
        assert_eq!(
            "hexagon".parse::<ShapeKind>(),
            Err(ParseError::new(
                "shape must be one of: any, triangle, rectangle, ellipse, circle, \
                 rotated-rectangle, quadratic, rotated-ellipse, polygon"
            ))
        );
    }

    #[test]
    fn triangle_validity_rejects_collinear_points() {
        let triangle = Triangle {
            x1: 0,
            y1: 0,
            x2: 1,
            y2: 1,
            x3: 2,
            y3: 2,
        };
        assert!(!triangle.is_valid());
    }

    #[test]
    fn rectangle_rasterize_matches_bounds() {
        let (mut worker, _) = round(8, 8);
        let shape = Shape::Rectangle(Rectangle {
            x1: 4,
            y1: 3,
            x2: 2,
            y2: 1,
        });
        let lines = shape.rasterize(&mut worker);
        assert_eq!(
            lines,
            &[
                Scanline {
                    y: 1,
                    x1: 2,
                    x2: 4,
                    alpha: 0xFFFF
                },
                Scanline {
                    y: 2,
                    x1: 2,
                    x2: 4,
                    alpha: 0xFFFF
                },
                Scanline {
                    y: 3,
                    x1: 2,
                    x2: 4,
                    alpha: 0xFFFF
                },
            ]
        );
    }

    #[test]
    fn triangle_rasterize_fills_pixel_centres_by_the_top_left_rule() {
        let (mut worker, _) = round(8, 8);
        let shape = Shape::Triangle(Triangle {
            x1: 0,
            y1: 0,
            x2: 4,
            y2: 0,
            x3: 0,
            y3: 4,
        });
        // Centres on the top and left edges are inside, centres on the
        // hypotenuse (`x + y == 4`) are not.
        let line = |y, x2| Scanline {
            y,
            x1: 0,
            x2,
            alpha: 0xFFFF,
        };
        assert_eq!(
            shape.rasterize(&mut worker),
            &[line(0, 3), line(1, 2), line(2, 1), line(3, 0)]
        );
    }

    #[test]
    fn ellipse_rasterize_matches_expected_scanlines() {
        let (mut worker, _) = round(11, 11);
        let shape = Shape::Ellipse(Ellipse {
            x: 5,
            y: 5,
            rx: 3,
            ry: 2,
        });
        let lines = shape.rasterize(&mut worker);
        assert_eq!(
            lines,
            &[
                Scanline {
                    y: 5,
                    x1: 2,
                    x2: 8,
                    alpha: 0xFFFF
                },
                Scanline {
                    y: 4,
                    x1: 3,
                    x2: 7,
                    alpha: 0xFFFF
                },
                Scanline {
                    y: 6,
                    x1: 3,
                    x2: 7,
                    alpha: 0xFFFF
                },
            ]
        );
    }

    #[test]
    fn mutate_keeps_circle_radius_equal() {
        let (mut worker, _) = round(32, 32);
        let mut shape = Shape::Circle(Circle { x: 10, y: 10, r: 4 });
        shape.mutate(&mut worker);
        match shape {
            Shape::Circle(circle) => assert!(circle.r >= 1),
            _ => panic!("expected circle"),
        }
    }

    #[test]
    fn quadratic_rasterizes_non_empty() {
        let (mut worker, _) = round(32, 32);
        let shape = Shape::Quadratic(Quadratic {
            x1: 4.0,
            y1: 4.0,
            x2: 10.0,
            y2: 12.0,
            x3: 20.0,
            y3: 6.0,
            width: 2.0,
        });
        assert!(!shape.rasterize(&mut worker).is_empty());
    }

    #[test]
    fn quadratic_repair_restores_validity() {
        let mut quadratic = Quadratic {
            x1: 4.0,
            y1: 4.0,
            x2: 30.0,
            y2: 30.0,
            x3: 8.0,
            y3: 4.0,
            width: 0.5,
        };

        assert!(!quadratic.is_valid());
        quadratic.repair_control_point(32, 32);
        assert!(quadratic.is_valid());
    }

    #[test]
    fn quadratic_mutate_always_yields_valid_shape() {
        // Run many mutations from the same starting state with different seeds.
        // Every call must produce a valid shape, through retries or repair.
        for seed in 0..500_u64 {
            let mut worker = WorkerCtx::new(32, 32, crate::rng::create_rng(seed));
            let mut quadratic = Quadratic {
                x1: 8.0,
                y1: 8.0,
                x2: 16.0,
                y2: 18.0,
                x3: 24.0,
                y3: 8.0,
                width: 0.5,
            };

            quadratic.mutate(&mut worker);
            assert!(quadratic.is_valid(), "seed {seed} produced invalid shape");
        }
    }

    #[test]
    fn quadratic_endpoint_repair_preserves_control_point() {
        // Directly test that repair_endpoint never modifies the control point.
        let old = Quadratic {
            x1: 16.0,
            y1: 16.0,
            x2: 32.0,
            y2: 40.0,
            x3: 48.0,
            y3: 16.0,
            width: 0.5,
        };
        assert!(old.is_valid());

        // Test p1 repair: move p1 close to p2 (invalidates shape).
        let mut q = old;
        q.x1 = 31.0;
        q.y1 = 39.0;
        assert!(!q.is_valid());
        q.repair_endpoint(&old, 0, 64, 64);
        assert!(q.is_valid());
        assert_eq!(q.x2, old.x2, "p1 repair must not touch control point x");
        assert_eq!(q.y2, old.y2, "p1 repair must not touch control point y");

        // Test p3 repair: move p3 very close to p2 (invalidates shape).
        let mut q = old;
        q.x3 = 31.0;
        q.y3 = 39.0;
        assert!(!q.is_valid());
        q.repair_endpoint(&old, 2, 64, 64);
        assert!(q.is_valid());
        assert_eq!(q.x2, old.x2, "p3 repair must not touch control point x");
        assert_eq!(q.y2, old.y2, "p3 repair must not touch control point y");
    }

    #[test]
    fn quadratic_repair_endpoint_lerps_back() {
        // An endpoint mutation that breaks validity should lerp the endpoint
        // back toward its old position, not snap it somewhere arbitrary.
        let mut quadratic = Quadratic {
            x1: 16.0,
            y1: 16.0,
            x2: 32.0,
            y2: 40.0,
            x3: 48.0,
            y3: 16.0,
            width: 0.5,
        };
        assert!(quadratic.is_valid());

        let old = quadratic;
        // Force an invalid endpoint position: move x1 very close to x2.
        quadratic.x1 = 31.0;
        quadratic.y1 = 39.0;
        assert!(!quadratic.is_valid());

        quadratic.repair_endpoint(&old, 0, 64, 64);
        assert!(quadratic.is_valid());
        // The control point must be unchanged.
        assert_eq!(quadratic.x2, old.x2);
        assert_eq!(quadratic.y2, old.y2);
        // The repaired endpoint should be between old and attempted position.
        assert!(quadratic.x1 >= old.x1.min(31.0) && quadratic.x1 <= old.x1.max(31.0));
        assert!(quadratic.y1 >= old.y1.min(39.0) && quadratic.y1 <= old.y1.max(39.0));
    }

    #[test]
    fn quadratic_control_point_repair_preserves_endpoints() {
        let mut quadratic = Quadratic {
            x1: 16.0,
            y1: 16.0,
            x2: 32.0,
            y2: 40.0,
            x3: 48.0,
            y3: 16.0,
            width: 0.5,
        };
        assert!(quadratic.is_valid());

        let old = quadratic;
        quadratic.x2 = 60.0;
        quadratic.y2 = 60.0;
        assert!(!quadratic.is_valid());

        quadratic.repair_control_point(64, 64);
        assert!(quadratic.is_valid());
        assert_eq!(quadratic.x1, old.x1);
        assert_eq!(quadratic.y1, old.y1);
        assert_eq!(quadratic.x3, old.x3);
        assert_eq!(quadratic.y3, old.y3);
    }

    #[test]
    fn quadratic_is_valid_uses_f64_precision() {
        // Sub-pixel differences must not be lost to integer truncation.
        let q = Quadratic {
            x1: 0.0,
            y1: 0.0,
            x2: 0.5,
            y2: 0.0,
            x3: 0.9,
            y3: 0.0,
            width: 0.5,
        };
        assert!(
            q.is_valid(),
            "sub-pixel quadratic should be valid with f64 precision"
        );
    }

    /// Checks every rasterizer's output contract on `lines`: each scanline
    /// lies inside the canvas with `x1 <= x2` and alpha `<= 0xFFFF`, and no
    /// pixel is emitted twice (energy, colour and drawing blend each line
    /// once, so a repeated pixel would be weighted twice).
    fn assert_lines_well_formed(lines: &[Scanline], width: i32, height: i32, context: &str) {
        let mut seen = vec![false; (width * height) as usize];
        for line in lines {
            assert!(
                (0..height).contains(&line.y)
                    && 0 <= line.x1
                    && line.x1 <= line.x2
                    && line.x2 < width
                    && line.alpha <= 0xFFFF,
                "{context}: malformed {line:?} on {width}x{height}"
            );
            for x in line.x1..=line.x2 {
                let i = (line.y * width + x) as usize;
                assert!(
                    !seen[i],
                    "{context}: pixel ({x}, {}) emitted twice on {width}x{height}",
                    line.y
                );
                seen[i] = true;
            }
        }
    }

    #[test]
    fn every_rasterizer_emits_each_pixel_at_most_once_and_in_bounds() {
        for (index, (width, height)) in [(2, 2), (3, 7), (64, 48), (257, 255)]
            .into_iter()
            .enumerate()
        {
            let (mut worker, round) = make_test_round(width, height, 100 + index as u64);
            for &kind in ShapeKind::all_kinds() {
                for sample in 0..150 {
                    let mut shape = Shape::random(kind, &mut worker, &round);
                    for step in 0..4 {
                        let context = format!("{kind:?} sample {sample} step {step}");
                        let lines = shape.rasterize(&mut worker).to_vec();
                        assert_lines_well_formed(&lines, width as i32, height as i32, &context);
                        shape.mutate(&mut worker);
                    }
                }
            }
        }
    }

    #[test]
    fn rotated_rectangle_left_of_the_canvas_emits_nothing() {
        let (mut worker, _) = round(16, 16);
        let shape = Shape::RotatedRectangle(RotatedRectangle {
            x: -20,
            y: 8,
            sx: 6,
            sy: 4,
            angle: 30,
        });
        assert_eq!(shape.rasterize(&mut worker), &[]);
    }

    #[test]
    fn rotated_ellipse_mutate_clamps_ry_to_the_height() {
        let (mut worker, _) = round(64, 8);
        let mut shape = Shape::RotatedEllipse(RotatedEllipse {
            x: 32.0,
            y: 4.0,
            rx: 10.0,
            ry: 3.0,
            angle: 0.0,
        });
        for _ in 0..200 {
            shape.mutate(&mut worker);
            let Shape::RotatedEllipse(ellipse) = &shape else {
                panic!("expected a rotated ellipse");
            };
            assert!(
                (1.0..=63.0).contains(&ellipse.rx) && (1.0..=7.0).contains(&ellipse.ry),
                "{ellipse:?}"
            );
        }
    }
}

#[cfg(test)]
mod geometry_tests;
