mod matrix;
mod metrics;
mod vector;

pub use matrix::{Matrix, multiply};
pub use metrics::{AmapMetrics, CmapMetrics};
pub use vector::{Vector, dot_product};
