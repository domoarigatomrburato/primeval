use crate::scanline::Scanline;
use crate::util::{degrees, radians, rotate_sc};
use crate::worker::{SearchRound, WorkerCtx};
use rand::{Rng, RngExt};
use rand_distr::{Distribution, StandardNormal};
use std::str::FromStr;

const POSITION_SIGMA: f64 = 16.0;
const ANGLE_SIGMA: f64 = 32.0;

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
pub enum Shape {
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
pub struct Triangle {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
    pub x3: i32,
    pub y3: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rectangle {
    pub x1: i32,
    pub y1: i32,
    pub x2: i32,
    pub y2: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ellipse {
    pub x: i32,
    pub y: i32,
    pub rx: i32,
    pub ry: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Circle {
    pub x: i32,
    pub y: i32,
    pub r: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RotatedRectangle {
    pub x: i32,
    pub y: i32,
    pub sx: i32,
    pub sy: i32,
    pub angle: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quadratic {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub x3: f64,
    pub y3: f64,
    pub width: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotatedEllipse {
    pub x: f64,
    pub y: f64,
    pub rx: f64,
    pub ry: f64,
    pub angle: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Polygon {
    pub order: usize,
    pub convex: bool,
    pub x: [f64; 4],
    pub y: [f64; 4],
}

impl Shape {
    #[must_use]
    pub fn random<R: Rng>(
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
            ShapeKind::Polygon => Self::Polygon(Polygon::random(worker, round, 4, false)),
            ShapeKind::Any => unreachable!("ShapeKind::Any is resolved before shape creation"),
        }
    }

    pub fn rasterize<'a, R: Rng>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
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

    pub fn mutate<R: Rng>(&mut self, worker: &mut WorkerCtx<R>, _round: &SearchRound<'_>) {
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

    #[must_use]
    pub fn scaled(&self, scale: f32) -> Self {
        match self {
            Self::Triangle(shape) => Self::Triangle(shape.scaled(scale)),
            Self::Rectangle(shape) => Self::Rectangle(shape.scaled(scale)),
            Self::Ellipse(shape) => Self::Ellipse(shape.scaled(scale)),
            Self::Circle(shape) => Self::Circle(shape.scaled(scale)),
            Self::RotatedRectangle(shape) => Self::RotatedRectangle(shape.scaled(scale)),
            Self::Quadratic(shape) => Self::Quadratic(shape.scaled(scale)),
            Self::RotatedEllipse(shape) => Self::RotatedEllipse(shape.scaled(scale)),
            Self::Polygon(shape) => Self::Polygon(shape.scaled(scale)),
        }
    }

    #[must_use]
    pub fn to_svg(&self, attrs: &str) -> String {
        match self {
            Self::Triangle(shape) => shape.svg_element(attrs),
            Self::Rectangle(shape) => shape.svg_element(attrs),
            Self::Ellipse(shape) => shape.svg_element(attrs),
            Self::Circle(shape) => shape.svg_element(attrs),
            Self::RotatedRectangle(shape) => shape.svg_element(attrs),
            Self::Quadratic(shape) => shape.svg_element(attrs),
            Self::RotatedEllipse(shape) => shape.svg_element(attrs),
            Self::Polygon(shape) => shape.svg_element(attrs),
        }
    }
}

impl ShapeKind {
    #[must_use]
    pub const fn variants() -> &'static [&'static str] {
        &[
            "any",
            "triangle",
            "rectangle",
            "ellipse",
            "circle",
            "rotated-rectangle",
            "quadratic",
            "rotated-ellipse",
            "polygon",
        ]
    }

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

    const fn all_kinds() -> &'static [ShapeKind] {
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
    type Err = String;

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
            other => Err(format!("unknown shape: {other}")),
        }
    }
}

impl Triangle {
    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        Self {
            x1: scale_i32(self.x1, scale),
            y1: scale_i32(self.y1, scale),
            x2: scale_i32(self.x2, scale),
            y2: scale_i32(self.y2, scale),
            x3: scale_i32(self.x3, scale),
            y3: scale_i32(self.y3, scale),
        }
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
    pub fn is_valid(&self) -> bool {
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
            self.x1,
            self.y1,
            self.x2,
            self.y2,
            self.x3,
            self.y3,
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

    fn svg_element(&self, attrs: &str) -> String {
        format!(
            "<polygon {} points=\"{},{} {},{} {},{}\" />",
            attrs, self.x1, self.y1, self.x2, self.y2, self.x3, self.y3
        )
    }
}

impl Rectangle {
    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        Self {
            x1: scale_i32(self.x1, scale),
            y1: scale_i32(self.y1, scale),
            x2: scale_i32(self.x2, scale),
            y2: scale_i32(self.y2, scale),
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

    fn svg_element(&self, attrs: &str) -> String {
        let (x1, y1, x2, y2) = self.bounds();
        format!(
            "<rect {} x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" />",
            attrs,
            x1,
            y1,
            x2 - x1 + 1,
            y2 - y1 + 1
        )
    }
}

impl Ellipse {
    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        Self {
            x: scale_i32(self.x, scale),
            y: scale_i32(self.y, scale),
            rx: scale_i32(self.rx, scale).max(1),
            ry: scale_i32(self.ry, scale).max(1),
        }
    }

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

    fn svg_element(&self, attrs: &str) -> String {
        format!(
            "<ellipse {} cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" />",
            attrs, self.x, self.y, self.rx, self.ry
        )
    }
}

impl Circle {
    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        Self {
            x: scale_i32(self.x, scale),
            y: scale_i32(self.y, scale),
            r: scale_i32(self.r, scale).max(1),
        }
    }

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

    fn svg_element(&self, attrs: &str) -> String {
        format!(
            "<circle {} cx=\"{}\" cy=\"{}\" r=\"{}\" />",
            attrs, self.x, self.y, self.r
        )
    }
}

impl RotatedRectangle {
    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        Self {
            x: scale_i32(self.x, scale),
            y: scale_i32(self.y, scale),
            sx: scale_i32(self.sx, scale).max(1),
            sy: scale_i32(self.sy, scale).max(1),
            angle: self.angle,
        }
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

    #[must_use]
    pub fn is_valid(&self) -> bool {
        let a = self.sx.max(self.sy);
        let b = self.sx.min(self.sy);
        b > 0 && (a as f64) / (b as f64) <= 5.0
    }

    fn rasterize<'a, R>(&self, worker: &'a mut WorkerCtx<R>) -> &'a [Scanline] {
        let sx = self.sx as f64;
        let sy = self.sy as f64;
        let angle = radians(self.angle as f64);
        let (sin_a, cos_a) = angle.sin_cos();
        let (rx1, ry1) = rotate_sc(-sx / 2.0, -sy / 2.0, sin_a, cos_a);
        let (rx2, ry2) = rotate_sc(sx / 2.0, -sy / 2.0, sin_a, cos_a);
        let (rx3, ry3) = rotate_sc(sx / 2.0, sy / 2.0, sin_a, cos_a);
        let (rx4, ry4) = rotate_sc(-sx / 2.0, sy / 2.0, sin_a, cos_a);
        let x1 = rx1 as i32 + self.x;
        let y1 = ry1 as i32 + self.y;
        let x2 = rx2 as i32 + self.x;
        let y2 = ry2 as i32 + self.y;
        let x3 = rx3 as i32 + self.x;
        let y3 = ry3 as i32 + self.y;
        let x4 = rx4 as i32 + self.x;
        let y4 = ry4 as i32 + self.y;

        let min_y = y1.min(y2).min(y3).min(y4);
        let max_y = y1.max(y2).max(y3).max(y4);
        let n = (max_y - min_y + 1).max(0) as usize;
        worker.rect_min.clear();
        worker.rect_min.resize(n, worker.width);
        worker.rect_max.clear();
        worker.rect_max.resize(n, 0);

        let xs = [x1, x2, x3, x4, x1];
        let ys = [y1, y2, y3, y4, y1];
        for i in 0..4 {
            let x = xs[i] as f64;
            let y = ys[i] as f64;
            let dx = (xs[i + 1] - xs[i]) as f64;
            let dy = (ys[i + 1] - ys[i]) as f64;
            let count = (((dx * dx + dy * dy).sqrt()) as i32 * 2).max(2) as usize;
            for j in 0..count {
                let t = j as f64 / (count - 1) as f64;
                let xi = (x + dx * t) as i32;
                let yi = (y + dy * t) as i32 - min_y;
                worker.rect_min[yi as usize] = worker.rect_min[yi as usize].min(xi);
                worker.rect_max[yi as usize] = worker.rect_max[yi as usize].max(xi);
            }
        }

        worker.lines.clear();
        for i in 0..n {
            let y = min_y + i as i32;
            if y < 0 || y >= worker.height {
                continue;
            }
            let x1 = worker.rect_min[i].max(0);
            let x2 = worker.rect_max[i].min(worker.width - 1);
            if x2 >= x1 {
                worker.lines.push(Scanline {
                    y,
                    x1,
                    x2,
                    alpha: 0xFFFF,
                });
            }
        }
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

    fn svg_element(&self, attrs: &str) -> String {
        format!(
            "<g transform=\"translate({} {}) rotate({}) scale({} {})\"><rect {} x=\"-0.5\" y=\"-0.5\" width=\"1\" height=\"1\" /></g>",
            self.x, self.y, self.angle, self.sx, self.sy, attrs
        )
    }
}

impl Quadratic {
    const MUTATE_MARGIN: f64 = 16.0;
    const MAX_MUTATE_ATTEMPTS: u32 = 6;

    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        let scale = f64::from(scale);
        Self {
            x1: self.x1 * scale,
            y1: self.y1 * scale,
            x2: self.x2 * scale,
            y2: self.y2 * scale,
            x3: self.x3 * scale,
            y3: self.y3 * scale,
            width: (self.width * scale).max(0.5),
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
    pub fn is_valid(&self) -> bool {
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

            let valid = self.is_valid();
            worker.note_quadratic_mutate_attempt(valid);
            if valid {
                return;
            }
        }

        match choice {
            0 | 2 => self.repair_endpoint(&old, choice, worker.width, worker.height),
            _ => self.repair_control_point(worker.width, worker.height),
        }
        debug_assert!(self.is_valid());
    }

    fn svg_element(&self, attrs: &str) -> String {
        let attrs = attrs.replace("fill", "stroke");
        format!(
            "<path {} fill=\"none\" d=\"M {:.6} {:.6} Q {:.6} {:.6}, {:.6} {:.6}\" stroke-width=\"{:.6}\" />",
            attrs, self.x1, self.y1, self.x2, self.y2, self.x3, self.y3, self.width
        )
    }
}

impl RotatedEllipse {
    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        let scale = f64::from(scale);
        Self {
            x: self.x * scale,
            y: self.y * scale,
            rx: (self.rx * scale).max(1.0),
            ry: (self.ry * scale).max(1.0),
            angle: self.angle,
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
                    .clamp(1.0, f64::from(worker.width - 1));
            }
            _ => self.angle += gaussian_sample(&mut worker.rng, ANGLE_SIGMA),
        }
    }

    fn svg_element(&self, attrs: &str) -> String {
        format!(
            "<g transform=\"translate({:.6} {:.6}) rotate({:.6}) scale({:.6} {:.6})\"><ellipse {} cx=\"0\" cy=\"0\" rx=\"1\" ry=\"1\" /></g>",
            self.x, self.y, self.angle, self.rx, self.ry, attrs
        )
    }
}

impl Polygon {
    #[must_use]
    fn scaled(&self, scale: f32) -> Self {
        let scale = f64::from(scale);
        let mut x = self.x;
        let mut y = self.y;
        for i in 0..self.order {
            x[i] *= scale;
            y[i] *= scale;
        }
        Self {
            order: self.order,
            convex: self.convex,
            x,
            y,
        }
    }

    fn random<R: Rng>(
        worker: &mut WorkerCtx<R>,
        round: &SearchRound<'_>,
        order: usize,
        convex: bool,
    ) -> Self {
        let mut x = [0.0; 4];
        let mut y = [0.0; 4];
        let (x0, y0) = worker.sample_xy_float(round);
        x[0] = x0;
        y[0] = y0;
        for i in 1..order {
            x[i] = x0 + worker.rng.random::<f64>() * 40.0 - 20.0;
            y[i] = y0 + worker.rng.random::<f64>() * 40.0 - 20.0;
        }
        let mut polygon = Self {
            order,
            convex,
            x,
            y,
        };
        polygon.mutate(worker);
        polygon
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        if !self.convex {
            return true;
        }
        let mut sign = false;
        for a in 0..self.order {
            let i = a % self.order;
            let j = (a + 1) % self.order;
            let k = (a + 2) % self.order;
            let cross = cross3(
                self.x[i], self.y[i], self.x[j], self.y[j], self.x[k], self.y[k],
            );
            if a == 0 {
                sign = cross > 0.0;
            } else if (cross > 0.0) != sign {
                return false;
            }
        }
        true
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
        loop {
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
            if self.is_valid() {
                break;
            }
        }
    }

    fn svg_element(&self, attrs: &str) -> String {
        let points = (0..self.order)
            .map(|i| format!("{:.6},{:.6}", self.x[i], self.y[i]))
            .collect::<Vec<_>>()
            .join(" ");
        format!("<polygon {} points=\"{}\" />", attrs, points)
    }
}

fn gaussian_sample<R: Rng>(rng: &mut R, sigma: f64) -> f64 {
    let sample: f64 = StandardNormal.sample(rng);
    sample * sigma
}

fn scale_i32(value: i32, scale: f32) -> i32 {
    (f64::from(value) * f64::from(scale)).round() as i32
}

fn cross3(x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> f64 {
    let dx1 = x2 - x1;
    let dy1 = y2 - y1;
    let dx2 = x3 - x2;
    let dy2 = y3 - y2;
    dx1 * dy2 - dy1 * dx2
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

fn rasterize_triangle(
    mut x1: i32,
    mut y1: i32,
    mut x2: i32,
    mut y2: i32,
    mut x3: i32,
    mut y3: i32,
    lines: &mut Vec<Scanline>,
) {
    if y1 > y3 {
        std::mem::swap(&mut x1, &mut x3);
        std::mem::swap(&mut y1, &mut y3);
    }
    if y1 > y2 {
        std::mem::swap(&mut x1, &mut x2);
        std::mem::swap(&mut y1, &mut y2);
    }
    if y2 > y3 {
        std::mem::swap(&mut x2, &mut x3);
        std::mem::swap(&mut y2, &mut y3);
    }
    if y2 == y3 {
        rasterize_triangle_bottom(x1, y1, x2, y2, x3, y3, lines);
        return;
    }
    if y1 == y2 {
        rasterize_triangle_top(x1, y1, x2, y2, x3, y3, lines);
        return;
    }
    let x4 = x1 + (((y2 - y1) as f64 / (y3 - y1) as f64) * (x3 - x1) as f64) as i32;
    rasterize_triangle_bottom(x1, y1, x2, y2, x4, y2, lines);
    rasterize_triangle_top(x2, y2, x4, y2, x3, y3, lines);
}

fn rasterize_triangle_bottom(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    x3: i32,
    y3: i32,
    lines: &mut Vec<Scanline>,
) {
    let s1 = (x2 - x1) as f64 / (y2 - y1) as f64;
    let s2 = (x3 - x1) as f64 / (y3 - y1) as f64;
    let (mut ax, mut bx) = (x1 as f64, x1 as f64);
    for y in y1..=y2 {
        let (mut a, mut b) = (ax as i32, bx as i32);
        ax += s1;
        bx += s2;
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        lines.push(Scanline {
            y,
            x1: a,
            x2: b,
            alpha: 0xFFFF,
        });
    }
}

fn rasterize_triangle_top(
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    x3: i32,
    y3: i32,
    lines: &mut Vec<Scanline>,
) {
    let s1 = (x3 - x1) as f64 / (y3 - y1) as f64;
    let s2 = (x3 - x2) as f64 / (y3 - y2) as f64;
    let (mut ax, mut bx) = (x3 as f64, x3 as f64);
    for y in (y1 + 1..=y3).rev() {
        ax -= s1;
        bx -= s2;
        let (mut a, mut b) = (ax as i32, bx as i32);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        lines.push(Scanline {
            y,
            x1: a,
            x2: b,
            alpha: 0xFFFF,
        });
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
        assert!("hexagon".parse::<ShapeKind>().is_err());
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
    fn circle_svg_emits_circle_element() {
        let shape = Shape::Circle(Circle { x: 10, y: 20, r: 7 });
        assert_eq!(
            shape.to_svg("fill='red'"),
            "<circle fill='red' cx=\"10\" cy=\"20\" r=\"7\" />"
        );
    }

    #[test]
    fn rotated_rectangle_validity_rejects_extreme_aspect_ratio() {
        let rect = RotatedRectangle {
            x: 10,
            y: 10,
            sx: 30,
            sy: 5,
            angle: 0,
        };
        assert!(!rect.is_valid());
    }

    #[test]
    fn mutate_keeps_circle_radius_equal() {
        let (mut worker, round) = round(32, 32);
        let mut shape = Shape::Circle(Circle { x: 10, y: 10, r: 4 });
        shape.mutate(&mut worker, &round);
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
    fn quadratic_mutate_repairs_invalid_candidates_with_low_retry_budget() {
        // Run many mutations from the same starting state with different seeds.
        // Every call must produce a valid shape without reopening the old
        // high-churn invalid retry loop.
        let mut total_attempts = 0_u64;
        let mut full_budget_hits = 0_u64;

        for seed in 0..500_u64 {
            let mut worker =
                WorkerCtx::new_with_quadratic_profiling(32, 32, crate::rng::create_rng(seed), true);
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

            let stats = worker.quadratic_profile_stats().unwrap();
            total_attempts += stats.mutate_attempts;
            full_budget_hits +=
                u64::from(stats.mutate_attempts == u64::from(Quadratic::MAX_MUTATE_ATTEMPTS));
        }

        assert!(
            total_attempts < 2_000,
            "unexpected retry churn: {total_attempts}"
        );
        assert!(
            full_budget_hits < 250,
            "too many repair fallbacks: {full_budget_hits}"
        );
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

    #[test]
    fn polygon_convex_check_rejects_sign_flip() {
        let polygon = Polygon {
            order: 4,
            convex: true,
            x: [0.0, 2.0, 1.0, 0.0],
            y: [0.0, 0.0, 1.0, 2.0],
        };
        assert!(!polygon.is_valid());
    }
}
