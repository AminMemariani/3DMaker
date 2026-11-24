//! Imports triangle meshes (Wavefront OBJ, STL) by sampling their surfaces
//! into Gaussian splats, so users can bring their own 3D models.
//!
//! OBJ colours come from per-vertex colours (`v x y z r g b`) or from `Kd`
//! in the referenced `.mtl`; otherwise a neutral grey is used. Textures are
//! not sampled.

use crate::fixtures::{PlyOptions, Splat, encode};
use std::collections::HashMap;
use std::path::Path;

const MAX_TRIANGLES: usize = 5_000_000;
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const GREY: [f32; 3] = [0.72, 0.72, 0.72];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshKind {
    Obj,
    Stl,
}

impl MeshKind {
    pub fn from_path(p: &Path) -> Option<Self> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "obj" => Some(Self::Obj),
            "stl" => Some(Self::Stl),
            _ => None,
        }
    }
    /// STL files are usually Z-up (CAD/printing); OBJ files are usually Y-up.
    pub fn view_rotation(self) -> [f32; 3] {
        match self {
            Self::Obj => [0.0; 3],
            Self::Stl => [-90.0, 0.0, 0.0],
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MeshError {
    #[error("the file couldn't be read: {0}")]
    Io(String),
    #[error("the file is larger than 512 MB")]
    TooLarge,
    #[error("the file contains no triangles")]
    Empty,
    #[error("the model has more than {MAX_TRIANGLES} triangles")]
    TooManyTriangles,
    #[error("line {line}: {problem}")]
    Bad { line: usize, problem: String },
}

#[derive(Debug, Clone, Copy)]
pub struct Tri {
    pub v: [[f32; 3]; 3],
    pub color: [f32; 3],
}

fn io(e: std::io::Error) -> MeshError {
    MeshError::Io(e.to_string())
}

pub fn load(path: &Path, kind: MeshKind) -> Result<Vec<Tri>, MeshError> {
    let len = std::fs::metadata(path).map_err(io)?.len();
    if len > MAX_FILE_BYTES {
        return Err(MeshError::TooLarge);
    }
    let bytes = std::fs::read(path).map_err(io)?;
    let tris = match kind {
        MeshKind::Obj => parse_obj(&String::from_utf8_lossy(&bytes), path.parent())?,
        MeshKind::Stl => parse_stl(&bytes)?,
    };
    if tris.is_empty() {
        return Err(MeshError::Empty);
    }
    Ok(tris)
}

fn floats<'a>(it: impl Iterator<Item = &'a str>) -> Vec<f32> {
    it.filter_map(|s| s.parse::<f32>().ok())
        .filter(|f| f.is_finite())
        .collect()
}

/// Reads `Kd` colours from an MTL file.
fn parse_mtl(text: &str) -> HashMap<String, [f32; 3]> {
    let mut out = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("newmtl") => current = it.next().map(String::from),
            Some("Kd") => {
                let c = floats(it);
                if let (Some(name), [r, g, b, ..]) = (&current, c.as_slice()) {
                    out.insert(name.clone(), [*r, *g, *b].map(|x| x.clamp(0.0, 1.0)));
                }
            }
            _ => {}
        }
    }
    out
}

pub fn parse_obj(text: &str, dir: Option<&Path>) -> Result<Vec<Tri>, MeshError> {
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut vcols: Vec<Option<[f32; 3]>> = Vec::new();
    let mut materials: HashMap<String, [f32; 3]> = HashMap::new();
    let mut mat_color: Option<[f32; 3]> = None;
    let mut tris = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let bad = |problem: &str| MeshError::Bad {
            line: n + 1,
            problem: problem.into(),
        };
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let f = floats(it);
                if f.len() < 3 {
                    return Err(bad("a vertex needs three numbers"));
                }
                verts.push([f[0], f[1], f[2]]);
                vcols.push((f.len() >= 6).then(|| {
                    let k = if f[3..6].iter().any(|&c| c > 1.0) {
                        255.0
                    } else {
                        1.0
                    };
                    [f[3] / k, f[4] / k, f[5] / k].map(|x| x.clamp(0.0, 1.0))
                }));
            }
            Some("mtllib") => {
                if let (Some(d), Some(name)) = (dir, it.next()) {
                    // Only a file next to the OBJ, never a path elsewhere.
                    if let Some(file) = Path::new(name).file_name() {
                        if let Ok(t) = std::fs::read_to_string(d.join(file)) {
                            materials.extend(parse_mtl(&t));
                        }
                    }
                }
            }
            Some("usemtl") => mat_color = it.next().and_then(|m| materials.get(m).copied()),
            Some("f") => {
                let mut idx = Vec::new();
                for tok in it {
                    let first = tok.split('/').next().unwrap_or("");
                    let i: i64 = first
                        .parse()
                        .map_err(|_| bad("a face index is not a number"))?;
                    let resolved = if i < 0 { verts.len() as i64 + i } else { i - 1 };
                    if resolved < 0 || resolved as usize >= verts.len() {
                        return Err(bad("a face refers to a vertex that doesn't exist"));
                    }
                    idx.push(resolved as usize);
                }
                // Fan-triangulate polygons.
                for k in 1..idx.len().saturating_sub(1) {
                    let ids = [idx[0], idx[k], idx[k + 1]];
                    let color = ids
                        .iter()
                        .filter_map(|&i| vcols[i])
                        .fold(None, |acc: Option<([f32; 3], f32)>, c| {
                            let (s, n) = acc.unwrap_or(([0.0; 3], 0.0));
                            Some(([s[0] + c[0], s[1] + c[1], s[2] + c[2]], n + 1.0))
                        })
                        .map(|(s, n)| s.map(|x| x / n))
                        .or(mat_color)
                        .unwrap_or(GREY);
                    tris.push(Tri {
                        v: ids.map(|i| verts[i]),
                        color,
                    });
                    if tris.len() > MAX_TRIANGLES {
                        return Err(MeshError::TooManyTriangles);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(tris)
}

pub fn parse_stl(bytes: &[u8]) -> Result<Vec<Tri>, MeshError> {
    // Binary STL: 80-byte header, u32 count, 50 bytes per triangle. Some
    // binary files start with "solid", so the size check decides.
    if bytes.len() >= 84 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
        if bytes.len() == 84 + n * 50 {
            if n > MAX_TRIANGLES {
                return Err(MeshError::TooManyTriangles);
            }
            let f =
                |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
            let mut tris = Vec::with_capacity(n);
            for t in 0..n {
                let o = 84 + t * 50 + 12;
                let v = [0, 1, 2].map(|k| [f(o + k * 12), f(o + k * 12 + 4), f(o + k * 12 + 8)]);
                if v.iter().flatten().all(|x| x.is_finite()) {
                    tris.push(Tri { v, color: GREY });
                }
            }
            return Ok(tris);
        }
    }
    let text = String::from_utf8_lossy(bytes);
    if !text.trim_start().starts_with("solid") {
        return Err(MeshError::Bad {
            line: 1,
            problem: "not an STL file".into(),
        });
    }
    let mut tris = Vec::new();
    let mut cur: Vec<[f32; 3]> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let mut it = line.split_whitespace();
        if it.next() == Some("vertex") {
            let f = floats(it);
            if f.len() < 3 {
                return Err(MeshError::Bad {
                    line: n + 1,
                    problem: "a vertex needs three numbers".into(),
                });
            }
            cur.push([f[0], f[1], f[2]]);
            if cur.len() == 3 {
                tris.push(Tri {
                    v: [cur[0], cur[1], cur[2]],
                    color: GREY,
                });
                cur.clear();
                if tris.len() > MAX_TRIANGLES {
                    return Err(MeshError::TooManyTriangles);
                }
            }
        }
    }
    Ok(tris)
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: [f32; 3]) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Samples triangle surfaces into about `budget` splats (area-weighted,
/// deterministic), oriented flat along each face.
pub fn to_splats(tris: &[Tri], budget: usize) -> Vec<Splat> {
    let areas: Vec<f32> = tris
        .iter()
        .map(|t| len(cross(sub(t.v[1], t.v[0]), sub(t.v[2], t.v[0]))) * 0.5)
        .collect();
    let total: f64 = areas.iter().map(|&a| a as f64).sum();
    if total <= 0.0 {
        return vec![];
    }
    let spacing = ((total / budget as f64).sqrt()) as f32;
    let tangent = spacing * 0.75;
    let thick = (spacing * 0.18).max(1e-6);
    let mut out = Vec::with_capacity(budget + 1024);
    let mut carry = 0.0f64;
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    for (t, &a) in tris.iter().zip(&areas) {
        // Fractional counts carry over so tiny triangles still add up.
        carry += a as f64 / total * budget as f64;
        let n = carry.floor() as usize;
        carry -= n as f64;
        if n == 0 {
            continue;
        }
        let nrm = cross(sub(t.v[1], t.v[0]), sub(t.v[2], t.v[0]));
        let l = len(nrm).max(1e-20);
        let nrm = nrm.map(|x| x / l);
        let rot = quat_z_to(nrm);
        for _ in 0..n {
            let (mut u, mut v) = (rnd(), rnd());
            if u + v > 1.0 {
                u = 1.0 - u;
                v = 1.0 - v;
            }
            let pos = [0, 1, 2]
                .map(|k| t.v[0][k] + u * (t.v[1][k] - t.v[0][k]) + v * (t.v[2][k] - t.v[0][k]));
            out.push(Splat {
                pos,
                scale: [tangent, tangent, thick],
                rot,
                color: t.color,
                alpha: 0.97,
            });
        }
    }
    out
}

fn quat_z_to(n: [f32; 3]) -> [f32; 4] {
    let d = n[2];
    if d < -0.99999 {
        return [0.0, 1.0, 0.0, 0.0];
    }
    let c = [-n[1], n[0], 0.0];
    let q = [1.0 + d, c[0], c[1], c[2]];
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.map(|v| v / l)
}

pub fn to_ply(tris: &[Tri], budget: usize) -> Vec<u8> {
    encode(
        &to_splats(tris, budget),
        &PlyOptions {
            comment: Some("converted from a mesh by 3D Maker".into()),
            ..PlyOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_OBJ: &str = "mtllib none.mtl\nv 0 0 0 1 0 0\nv 1 0 0 1 0 0\nv 1 1 0 1 0 0\nv 0 1 0 1 0 0\nf 1 2 3 4\nf -4/1/1 -2/2/2 -1/3/3\n";

    #[test]
    fn obj_quads_are_fanned_and_vertex_colours_kept() {
        let t = parse_obj(CUBE_OBJ, None).unwrap();
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].color, [1.0, 0.0, 0.0]);
        assert!(parse_obj("v 0 0 0\nf 1 2 3\n", None).is_err());
    }

    #[test]
    fn mtl_colours_apply() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("m.mtl"), "newmtl blue\nKd 0 0 1\n").unwrap();
        let obj = "mtllib m.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl blue\nf 1 2 3\n";
        assert_eq!(
            parse_obj(obj, Some(dir.path())).unwrap()[0].color,
            [0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn binary_and_ascii_stl() {
        let mut b = vec![0u8; 80];
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&[0u8; 12]);
        for v in [[0f32, 0., 0.], [1., 0., 0.], [0., 1., 0.]] {
            for x in v {
                b.extend_from_slice(&x.to_le_bytes());
            }
        }
        b.extend_from_slice(&[0, 0]);
        assert_eq!(parse_stl(&b).unwrap().len(), 1);
        let ascii = "solid x\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid";
        assert_eq!(parse_stl(ascii.as_bytes()).unwrap().len(), 1);
        assert!(parse_stl(b"hello").is_err());
    }

    #[test]
    fn sampling_hits_budget_and_stays_on_faces() {
        let t = parse_obj(CUBE_OBJ, None).unwrap();
        let s = to_splats(&t, 5_000);
        assert!((4_990..=5_010).contains(&s.len()), "{}", s.len());
        assert!(
            s.iter()
                .all(|p| p.pos[2].abs() < 1e-6 && (0.0..=1.0).contains(&p.pos[0]))
        );
        let bytes = to_ply(&t, 1000);
        let sum = crate::ply::validate_reader(
            std::io::Cursor::new(&bytes),
            bytes.len() as u64,
            &Default::default(),
        )
        .unwrap();
        assert!(sum.count > 900);
    }
}
