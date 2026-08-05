//! Native LaTeX-to-SVG rendering for reader and chat formulas.
//!
//! `RaTeX` owns parsing and TeX layout. This crate only adapts its display list to
//! Torto's baseline-oriented SVG contract and normalizes a narrow set of OCR
//! artifacts before parsing.

pub mod math;
