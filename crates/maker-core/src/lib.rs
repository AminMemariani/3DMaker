//! Core of 3D Maker: provider adapters, job engine, Gaussian PLY validation,
//! persistence and credential handling. Independent of Tauri so it can be
//! tested headlessly.

pub mod adjust;
pub mod catalog;
pub mod design;
pub mod detect;
pub mod engine;
pub mod errors;
pub mod fixtures;
pub mod library;
pub mod mesh;
pub mod model;
pub mod net;
pub mod ply;
pub mod providers;
pub mod redact;
pub mod secrets;
pub mod store;
