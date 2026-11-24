//! Deterministic, redistributable Gaussian PLY fixtures for tests, the fake
//! provider and the renderer-correctness harness. All data is synthetic.

pub const SH_C0: f32 = 0.282_094_8;

#[derive(Debug, Clone, Copy)]
pub struct Splat {
    pub pos: [f32; 3],
    /// Linear (not log) scale per axis.
    pub scale: [f32; 3],
    /// Quaternion as (w, x, y, z).
    pub rot: [f32; 4],
    /// sRGB-ish base colour 0..1.
    pub color: [f32; 3],
    /// Linear opacity 0..1 (stored as a logit).
    pub alpha: f32,
}

impl Splat {
    pub fn sphere(pos: [f32; 3], r: f32, color: [f32; 3]) -> Self {
        Self {
            pos,
            scale: [r; 3],
            rot: [1.0, 0.0, 0.0, 0.0],
            color,
            alpha: 0.998,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PlyOptions {
    pub sh_degree: u8,
    pub normals: bool,
    /// Extra float properties appended (e.g. producer attributes).
    pub extra: Vec<String>,
    /// Write properties in a scrambled but valid order.
    pub shuffle: bool,
    pub comment: Option<String>,
}

fn logit(a: f32) -> f32 {
    let a = a.clamp(1e-6, 1.0 - 1e-6);
    (a / (1.0 - a)).ln()
}

/// Encodes splats as a standard binary little-endian 3DGS PLY.
pub fn encode(splats: &[Splat], opts: &PlyOptions) -> Vec<u8> {
    let rest = match opts.sh_degree {
        0 => 0,
        1 => 9,
        2 => 24,
        _ => 45,
    };
    let mut names: Vec<String> = vec!["x".into(), "y".into(), "z".into()];
    if opts.normals {
        names.extend(["nx", "ny", "nz"].map(String::from));
    }
    names.extend(["f_dc_0", "f_dc_1", "f_dc_2"].map(String::from));
    names.extend((0..rest).map(|i| format!("f_rest_{i}")));
    names.push("opacity".into());
    names.extend(["scale_0", "scale_1", "scale_2"].map(String::from));
    names.extend(["rot_0", "rot_1", "rot_2", "rot_3"].map(String::from));
    names.extend(opts.extra.iter().cloned());
    if opts.shuffle {
        // Deterministic permutation: reverse, then rotate by 5.
        names.reverse();
        let k = 5 % names.len();
        names.rotate_left(k);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"ply\nformat binary_little_endian 1.0\n");
    if let Some(c) = &opts.comment {
        out.extend_from_slice(format!("comment {c}\n").as_bytes());
    }
    out.extend_from_slice(format!("element vertex {}\n", splats.len()).as_bytes());
    for n in &names {
        out.extend_from_slice(format!("property float {n}\n").as_bytes());
    }
    out.extend_from_slice(b"end_header\n");
    for (i, s) in splats.iter().enumerate() {
        for n in &names {
            let v = value(s, n, i);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn value(s: &Splat, name: &str, i: usize) -> f32 {
    match name {
        "x" => s.pos[0],
        "y" => s.pos[1],
        "z" => s.pos[2],
        "nx" | "ny" | "nz" => 0.0,
        "f_dc_0" => (s.color[0] - 0.5) / SH_C0,
        "f_dc_1" => (s.color[1] - 0.5) / SH_C0,
        "f_dc_2" => (s.color[2] - 0.5) / SH_C0,
        "opacity" => logit(s.alpha),
        "scale_0" => s.scale[0].ln(),
        "scale_1" => s.scale[1].ln(),
        "scale_2" => s.scale[2].ln(),
        "rot_0" => s.rot[0],
        "rot_1" => s.rot[1],
        "rot_2" => s.rot[2],
        "rot_3" => s.rot[3],
        n if n.starts_with("f_rest_") => {
            // Small, deterministic view-dependent tint.
            let k: usize = n[7..].parse().unwrap_or(0);
            (((i * 31 + k * 7) % 17) as f32 - 8.0) * 0.01
        }
        _ => (i % 7) as f32 * 0.5, // producer extras
    }
}

/// The asymmetric renderer-correctness scene described in docs/dev/renderer-fixture.md.
pub fn renderer_scene() -> Vec<Splat> {
    let h = std::f32::consts::FRAC_PI_8; // half of 45°
    vec![
        Splat {
            pos: [0.0, 0.0, 0.0],
            scale: [0.55, 0.06, 0.06],
            rot: [h.cos(), 0.0, 0.0, h.sin()],
            color: [1.0, 0.9, 0.1],
            alpha: 0.998,
        },
        Splat::sphere([0.0, 0.55, 0.45], 0.12, [0.1, 0.9, 0.2]),
        Splat::sphere([0.0, 0.55, -0.45], 0.12, [0.9, 0.1, 0.9]),
        Splat {
            scale: [0.14, 0.14, 0.02],
            ..Splat::sphere([0.5, -0.45, 0.3], 1.0, [0.95, 0.1, 0.1])
        },
        Splat {
            scale: [0.14, 0.14, 0.02],
            ..Splat::sphere([0.5, -0.45, -0.3], 1.0, [0.1, 0.2, 0.95])
        },
        Splat::sphere([-0.6, -0.45, 0.0], 0.08, [0.95, 0.95, 0.95]),
    ]
}

/// Tiny xorshift so fixtures don't need a rand dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A teapot-like object made of `n` splats: blue body, curved spout, brass lid
/// and loop handle. Used by the fake provider so end-to-end runs look plausible.
/// Output is in the 3DGS (y-down) convention, like TRELLIS's exporter.
pub fn teapot(n: usize, seed: u64) -> Vec<Splat> {
    let mut rng = Rng(seed.max(1));
    let blue = [0.05, 0.2, 0.75];
    let brass = [0.8, 0.62, 0.25];
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let part = i % 10;
        let u = rng.next() * std::f32::consts::TAU;
        let v = rng.next() * 2.0 - 1.0;
        let s = (1.0 - v * v).sqrt();
        let (pos, color, r) = match part {
            0..=5 => ([0.5 * s * u.cos(), 0.38 * v, 0.5 * s * u.sin()], blue, 0.03),
            6 => {
                // spout: curve from the body outward and up (+x)
                let t = rng.next();
                (
                    [
                        0.45 + 0.35 * t,
                        -0.05 + 0.3 * t * t + 0.04 * u.cos(),
                        0.04 * u.sin(),
                    ],
                    blue,
                    0.02,
                )
            }
            7 => {
                // handle: loop on -x
                let a = rng.next() * std::f32::consts::PI;
                (
                    [-0.5 - 0.22 * a.sin(), 0.25 * a.cos(), 0.03 * u.sin()],
                    blue,
                    0.02,
                )
            }
            // brass lid on top
            _ => (
                [0.2 * s * u.cos(), 0.4 + 0.08 * v.abs(), 0.2 * s * u.sin()],
                brass,
                0.025,
            ),
        };
        // Parts above are modelled y-up; producers like TRELLIS write y-down.
        out.push(Splat {
            pos: [pos[0], -pos[1], pos[2]],
            scale: [r, r * 0.6, r],
            rot: [1.0, 0.0, 0.0, 0.0],
            color,
            alpha: 0.9,
        });
    }
    out
}

/// A dense pseudo-random cloud for performance measurement.
pub fn perf_cloud(n: usize) -> Vec<Splat> {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    (0..n)
        .map(|_| {
            let u = rng.next() * std::f32::consts::TAU;
            let v = rng.next() * 2.0 - 1.0;
            let r = 0.6 + 0.4 * rng.next();
            let s = (1.0 - v * v).sqrt();
            let a = rng.next() * std::f32::consts::PI;
            Splat {
                pos: [r * s * u.cos(), r * v, r * s * u.sin()],
                scale: [0.01 + 0.02 * rng.next(), 0.01, 0.01 + 0.01 * rng.next()],
                rot: [a.cos(), 0.0, a.sin(), 0.0],
                color: [rng.next(), rng.next(), rng.next()],
                alpha: 0.5 + 0.5 * rng.next(),
            }
        })
        .collect()
}
