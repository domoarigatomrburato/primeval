//! The engine boundary: an ordered list of committed shapes in continuous
//! canvas coordinates.
//!
//! The engine produces a [`Drawing`]; output writers (SVG, raster) consume it.
//! Nothing here depends on how the engine searched for the shapes, so another
//! engine can produce the same type.
//!
//! Coordinates are continuous canvas coordinates at working resolution: pixel
//! `(i, j)` is the unit square `[i, i + 1) × [j, j + 1)`.

use crate::Color;

/// A point in continuous canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    #[must_use]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// Engine-independent geometry of one committed shape.
#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    /// Axis-aligned filled rectangle.
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    /// Filled ellipse centred at `(cx, cy)`, rotated clockwise by `rotation`
    /// degrees (the SVG `rotate()` direction in a y-down canvas).
    Ellipse {
        cx: f64,
        cy: f64,
        rx: f64,
        ry: f64,
        rotation: f64,
    },
    /// Closed filled polygon (non-zero fill rule).
    Polygon(Vec<Point>),
    /// Stroked quadratic Bézier curve with butt caps.
    Quadratic {
        start: Point,
        control: Point,
        end: Point,
        width: f64,
    },
}

/// One committed shape: its geometry and its colour. `color.a` is the opacity.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawnShape {
    pub geometry: Geometry,
    pub color: Color,
}

/// What the engine hands to output writers.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawing {
    /// Canvas width in working-resolution units.
    pub width: u32,
    /// Canvas height in working-resolution units.
    pub height: u32,
    /// Opaque background colour (alpha 255).
    pub background: Color,
    /// Shapes in paint order.
    pub shapes: Vec<DrawnShape>,
}
