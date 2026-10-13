//! OxShot core: document model, geometry, rendering and project files. No GUI dependencies.

pub mod edit;
pub mod geom;
pub mod model;
pub mod ops;
pub mod project;
pub mod render;

pub use geom::{IRect, Point, Rect};
pub use model::*;
