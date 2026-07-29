//! Lightweight LaTeX-to-SVG rendering used by the desktop chat view.
//!
//! The implementation is derived from the MIT-licensed math and font modules
//! in Markie 0.3.0. Keeping this focused crate avoids pulling Markie's unrelated
//! CLI, PDF export, syntax-highlighting, HTTP, and rasterization dependencies.

pub mod fonts;
pub mod math;
mod xml;
