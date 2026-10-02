//! Core library for the primeval image approximation engine.
//!
//! Provides the fundamental types and algorithms for approximating images
//! through geometric primitives: pixel buffers, color representation,
//! scanline rasterization, scoring/blending, and deterministic RNG.
//!
//! The engine takes a target [`Buffer`] and a background colour and produces
//! a [`Drawing`]: the committed shapes as engine-independent geometry.
//! Decoding input and writing output live in `primeval-render`.

pub mod buffer;
pub mod color;
pub mod drawing;
pub mod error_grid;
pub mod model;
pub mod optimize;
pub mod raster;
pub mod rng;
pub mod scanline;
pub mod score;
pub mod shapes;
pub mod state;
pub mod util;
pub mod worker;

#[cfg(test)]
pub(crate) mod test_util;

pub use buffer::Buffer;
pub use color::Color;
pub use drawing::{Drawing, DrawnShape, Geometry, Point};
pub use model::{CommittedShape, Model, ModelOptions};
pub use scanline::Scanline;
