//! Writes the synthetic fixtures used by tests and the renderer harness.
//! Usage: cargo run -p maker-core --example make_fixtures -- <out-dir>

use maker_core::fixtures::{self, PlyOptions};
use maker_core::ply::{PlyLimits, validate_file};
use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "fixtures".into()));
    std::fs::create_dir_all(&dir)?;
    let items: Vec<(&str, Vec<u8>)> = vec![
        (
            "renderer-asym.ply",
            fixtures::encode(
                &fixtures::renderer_scene(),
                &PlyOptions {
                    normals: true,
                    comment: Some("3D Maker renderer fixture (synthetic, CC0)".into()),
                    ..Default::default()
                },
            ),
        ),
        (
            "teapot-fake.ply",
            fixtures::encode(
                &fixtures::teapot(30_000, 7),
                &PlyOptions {
                    sh_degree: 1,
                    normals: true,
                    comment: Some("synthetic stand-in, not a model output".into()),
                    ..Default::default()
                },
            ),
        ),
        (
            "forklift-fake.ply",
            fixtures::encode(
                &fixtures::perf_cloud(8_000),
                &PlyOptions {
                    comment: Some("synthetic stand-in, not a model output".into()),
                    ..Default::default()
                },
            ),
        ),
        (
            "perf-100k.ply",
            fixtures::encode(&fixtures::perf_cloud(100_000), &PlyOptions::default()),
        ),
    ];
    for (name, bytes) in items {
        let path = dir.join(name);
        std::fs::write(&path, &bytes)?;
        let s = validate_file(&path, &PlyLimits::default()).expect("fixture must validate");
        println!(
            "{name}: {} splats, SH degree {}, {} bytes, sha256 {}",
            s.count, s.sh_degree, s.file_bytes, s.sha256
        );
    }
    Ok(())
}
