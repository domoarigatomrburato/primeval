//! Core library for the primeval image approximation engine.
//!
//! The engine takes a target [`Buffer`] and a background colour and produces
//! a [`Drawing`]: the committed shapes as engine-independent geometry.
//! Decoding input and writing output live in `primeval-render`.
//!
//! The public surface is the [`Model`] that runs the search, its options
//! ([`ModelOptions`], [`ShapeKind`], [`Alpha`]), the pixel and colour types it
//! takes ([`Buffer`], [`Color`]), the [`Drawing`] it produces, and the joint
//! optimisation of its triangles and polygons that can follow it
//! ([`joint`]).

#![warn(missing_docs)]

mod alpha;
#[cfg(feature = "bench")]
mod benches;
mod buffer;
mod coarse;
mod color;
mod drawing;
mod error;
mod error_grid;
pub mod joint;
mod model;
mod optimize;
mod prefix;
mod raster;
mod refine;
mod rng;
mod scanline;
mod score;
mod shapes;
mod state;
mod util;
mod worker;

#[cfg(test)]
mod test_util;

pub use alpha::Alpha;
pub use buffer::Buffer;
pub use color::Color;
pub use drawing::{Drawing, DrawnShape, Geometry, Point};
pub use error::ParseError;
pub use model::{Model, ModelOptions};
pub use shapes::ShapeKind;
