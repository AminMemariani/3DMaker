//! Shape designs: a language model describes one object as a list of
//! coloured primitives, and this module turns that list into a Gaussian
//! splat PLY locally. No image or 3D model is involved, so any text model
//! (Anthropic, OpenAI, a custom OpenAI-compatible server, Claude Code,
//! Ollama) can produce objects.
//!
//! Parsing is deliberately forgiving about *form* (code fences, prose around
//! the JSON, shape aliases, colour notations) and strict about *values*
//! (finite numbers, bounded sizes, a bounded part count), because the input
//! is model output.

use crate::fixtures::{PlyOptions, Splat, encode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::f32::consts::{PI, TAU};

/// Upper bounds on what a design may ask for.
pub const MAX_PARTS: usize = 160;
const MAX_COORD: f32 = 50.0;
const MIN_SIZE: f32 = 0.002;
const MAX_SIZE: f32 = 50.0;

/// Default number of splats for a whole object.
pub const DEFAULT_SPLATS: usize = 90_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Shape {
    Box,
    Ellipsoid,
    Cylinder,
    Cone,
    Torus,
}

impl Shape {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "box" | "cube" | "cuboid" | "block" | "rect" | "rectangle" | "slab" => Self::Box,
            "ellipsoid" | "sphere" | "ball" | "spheroid" | "egg" | "dome" => Self::Ellipsoid,
            "cylinder" | "tube" | "rod" | "disc" | "disk" | "pipe" => Self::Cylinder,
            "cone" | "pyramid" | "spike" => Self::Cone,
            "torus" | "ring" | "donut" | "doughnut" | "loop" => Self::Torus,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Part {
    pub shape: Shape,
    pub center: [f32; 3],
    /// Full extents along local x, y, z. Cylinders and cones run along y;
    /// a torus lies in the xz plane with `size[1]` as its tube thickness.
    pub size: [f32; 3],
    /// XYZ Euler angles in degrees.
    pub rotation: [f32; 3],
    /// Linear-ish sRGB, 0..1.
    pub color: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Design {
    pub name: String,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DesignError {
    #[error("the answer contained no JSON object")]
    NoJson,
    #[error("the JSON could not be read: {0}")]
    BadJson(String),
    #[error("the design has no parts")]
    Empty,
    #[error("the design has {0} parts; at most {MAX_PARTS} are allowed")]
    TooManyParts(usize),
    #[error("part {index}: {problem}")]
    BadPart { index: usize, problem: String },
}

/// Instructions given to every model. The user's description is sent
/// separately and verbatim.
pub fn system_prompt() -> String {
    format!(
        r##"You are a 3D modeller. You build exactly ONE physical object, described by the user, out of simple coloured primitives. Reply with a single JSON object and nothing else: no prose, no code fences.

Schema:
{{
  "name": "short object name",
  "parts": [
    {{
      "label": "what this part is",
      "shape": "box" | "ellipsoid" | "cylinder" | "cone" | "torus",
      "center": [x, y, z],
      "size": [sx, sy, sz],
      "rotation": [rx, ry, rz],
      "color": "#rrggbb"
    }}
  ]
}}

Rules:
- Units are metres-like; Y is up; the object stands on y = 0 and is centred on x = 0, z = 0. Fit the whole object roughly inside a 2 x 2 x 2 box.
- "size" is the FULL extent along the part's own x, y, z axes before rotation.
- cylinder and cone run along their own y axis (cone tip at +y). Use a short wide cylinder for discs, wheels and lids.
- torus lies flat in its own xz plane: size[0] and size[2] are the outer diameters, size[1] is the tube thickness. Rotate it 90 degrees about x or z to stand a ring or loop handle upright.
- "rotation" is in degrees, applied about x, then y, then z. Omit or use [0, 0, 0] when not needed.
- Parts should touch or overlap so the object reads as one connected piece.
- Use 6 to 60 parts. Model every feature the user mentions, with the requested colours and counts. Add a few plausible details if helpful.
- Colours are hex strings. Pick realistic colours for anything the user does not specify.
- At most {MAX_PARTS} parts."##
    )
}

/// Instructions for changing an existing design.
pub fn edit_system_prompt() -> String {
    format!(
        "{}\n\nYou are editing an existing design. The user sends the current design JSON and a change request. Reply with the COMPLETE updated design in the same schema: keep every part that the request doesn't affect exactly as it is, and change, add or remove only what the request asks for.",
        system_prompt()
    )
}

/// The current design plus the requested change (verbatim).
pub fn edit_user_prompt(current: &Design, instruction: &str) -> String {
    format!(
        "Current design:\n{}\n\nChange request:\n{}",
        serde_json::to_string(current).unwrap_or_default(),
        instruction.trim()
    )
}

/// Wraps the user's description; the original wording is kept verbatim.
pub fn user_prompt(description: &str) -> String {
    format!("Object to build:\n{}", description.trim())
}

/// Finds the outermost JSON object in model output (tolerates code fences
/// and surrounding prose).
fn extract_json(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

fn num(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    }
    .filter(|f| f.is_finite())
}

fn vec3(v: Option<&Value>, default: Option<[f32; 3]>) -> Result<[f32; 3], String> {
    match v {
        None | Some(Value::Null) => default.ok_or_else(|| "is missing".to_string()),
        Some(Value::Array(a)) if a.len() == 3 => {
            let mut out = [0.0; 3];
            for (o, x) in out.iter_mut().zip(a) {
                *o = num(x).ok_or("has a value that is not a number")?;
            }
            Ok(out)
        }
        Some(Value::Object(o)) => {
            let g = |k: &str| o.get(k).and_then(num);
            match (g("x"), g("y"), g("z")) {
                (Some(x), Some(y), Some(z)) => Ok([x, y, z]),
                _ => Err("needs x, y and z".into()),
            }
        }
        // A single number means the same value on all three axes.
        Some(v) => num(v)
            .map(|f| [f; 3])
            .ok_or_else(|| "must be three numbers".into()),
    }
}

fn color(v: Option<&Value>) -> Result<[f32; 3], String> {
    let Some(v) = v else {
        return Ok([0.7, 0.7, 0.7]);
    };
    if let Value::String(s) = v {
        let h = s.trim().trim_start_matches('#');
        let hex = |s: &str| u8::from_str_radix(s, 16).ok();
        let rgb = match h.len() {
            6 => (hex(&h[0..2]), hex(&h[2..4]), hex(&h[4..6])),
            3 => (
                hex(&h[0..1].repeat(2)),
                hex(&h[1..2].repeat(2)),
                hex(&h[2..3].repeat(2)),
            ),
            _ => (None, None, None),
        };
        return match rgb {
            (Some(r), Some(g), Some(b)) => {
                Ok([r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0])
            }
            _ => Err(format!("colour `{s}` is not a #rrggbb value")),
        };
    }
    let c = vec3(Some(v), None).map_err(|e| format!("colour {e}"))?;
    // Accept 0..1 or 0..255.
    let k = if c.iter().any(|&x| x > 1.0) {
        255.0
    } else {
        1.0
    };
    Ok(c.map(|x| (x / k).clamp(0.0, 1.0)))
}

/// Parses and validates a model's answer.
pub fn parse(text: &str) -> Result<Design, DesignError> {
    let json = extract_json(text).ok_or(DesignError::NoJson)?;
    let root: Value =
        serde_json::from_str(json).map_err(|e| DesignError::BadJson(e.to_string()))?;
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("object")
        .chars()
        .take(80)
        .collect::<String>();
    let parts = root
        .get("parts")
        .or_else(|| root.get("primitives"))
        .or_else(|| root.get("shapes"))
        .and_then(Value::as_array)
        .ok_or(DesignError::Empty)?;
    if parts.is_empty() {
        return Err(DesignError::Empty);
    }
    if parts.len() > MAX_PARTS {
        return Err(DesignError::TooManyParts(parts.len()));
    }
    let mut out = Vec::with_capacity(parts.len());
    for (index, p) in parts.iter().enumerate() {
        let bad = |problem: String| DesignError::BadPart { index, problem };
        let shape_name = p
            .get("shape")
            .or_else(|| p.get("type"))
            .and_then(Value::as_str)
            .ok_or_else(|| bad("has no shape".into()))?;
        let shape =
            Shape::parse(shape_name).ok_or_else(|| bad(format!("unknown shape `{shape_name}`")))?;
        let center = vec3(p.get("center").or_else(|| p.get("position")), None)
            .map_err(|e| bad(format!("center {e}")))?;
        let size = vec3(p.get("size").or_else(|| p.get("scale")), None)
            .map_err(|e| bad(format!("size {e}")))?;
        let rotation =
            vec3(p.get("rotation"), Some([0.0; 3])).map_err(|e| bad(format!("rotation {e}")))?;
        let color = color(p.get("color").or_else(|| p.get("colour"))).map_err(bad)?;
        if center.iter().any(|c| c.abs() > MAX_COORD) {
            return Err(bad(format!("center is outside ±{MAX_COORD}")));
        }
        if size
            .iter()
            .any(|&s| !(MIN_SIZE..=MAX_SIZE).contains(&s.abs()))
        {
            return Err(bad(format!(
                "size must be between {MIN_SIZE} and {MAX_SIZE}"
            )));
        }
        let size = size.map(f32::abs);
        if shape == Shape::Torus && size[1] * 1.001 >= size[0].min(size[2]) {
            return Err(bad("torus tube is as thick as the ring itself".into()));
        }
        let label = p
            .get("label")
            .or_else(|| p.get("name"))
            .and_then(Value::as_str)
            .map(|s| s.chars().take(60).collect());
        out.push(Part {
            shape,
            center,
            size,
            rotation,
            color,
            label,
        });
    }
    Ok(Design { name, parts: out })
}

// ---------- geometry ----------

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 1e-12 {
        scale(a, 1.0 / l)
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Rotation matrix for XYZ Euler degrees (x applied first).
pub(crate) fn rot_matrix(deg: V3) -> [V3; 3] {
    let [x, y, z] = deg.map(f32::to_radians);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    // R = Rz * Ry * Rx
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}
pub(crate) fn mul(m: &[V3; 3], v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// Quaternion (w, x, y, z) rotating +z onto `n`.
fn quat_z_to(n: V3) -> [f32; 4] {
    let z = [0.0, 0.0, 1.0];
    let d = dot(z, n);
    if d < -0.99999 {
        return [0.0, 1.0, 0.0, 0.0];
    }
    let c = cross(z, n);
    let q = [1.0 + d, c[0], c[1], c[2]];
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    q.map(|v| v / l)
}

/// Small deterministic generator so designs rebuild identically.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn f(&mut self) -> f32 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Approximate surface area of a part in world units.
fn area(p: &Part) -> f32 {
    let [hx, hy, hz] = scale(p.size, 0.5);
    match p.shape {
        Shape::Box => 8.0 * (hx * hy + hy * hz + hx * hz),
        Shape::Ellipsoid => {
            // Knud Thomsen's approximation.
            let k = 1.6075f32;
            let s = ((hx * hy).powf(k) + (hx * hz).powf(k) + (hy * hz).powf(k)) / 3.0;
            4.0 * PI * s.powf(1.0 / k)
        }
        Shape::Cylinder => {
            let r = (hx + hz) * 0.5;
            TAU * r * 2.0 * hy + 2.0 * PI * r * r
        }
        Shape::Cone => {
            let r = (hx + hz) * 0.5;
            PI * r * (r + (r * r + 4.0 * hy * hy).sqrt())
        }
        Shape::Torus => {
            let t = p.size[1] * 0.5;
            let big = ((p.size[0] + p.size[2]) * 0.25 - t).max(t);
            4.0 * PI * PI * big * t
        }
    }
}

/// One surface sample in the part's unit space: point and normal, before the
/// part's scale. Unit shapes span -1..1 on every axis.
fn sample_unit(shape: Shape, half: V3, r: &mut Rng) -> (V3, V3) {
    let u = r.f();
    let v = r.f();
    match shape {
        Shape::Box => {
            let [hx, hy, hz] = half;
            let areas = [hy * hz, hx * hz, hx * hy];
            let total = areas.iter().sum::<f32>() * 2.0;
            let mut pick = r.f() * total;
            let sign = if r.f() < 0.5 { -1.0 } else { 1.0 };
            let mut axis = 2;
            for (i, a) in areas.iter().enumerate() {
                if pick < a * 2.0 {
                    axis = i;
                    break;
                }
                pick -= a * 2.0;
            }
            let (a, b) = (u * 2.0 - 1.0, v * 2.0 - 1.0);
            let (p, n) = match axis {
                0 => ([sign, a, b], [sign, 0.0, 0.0]),
                1 => ([a, sign, b], [0.0, sign, 0.0]),
                _ => ([a, b, sign], [0.0, 0.0, sign]),
            };
            (p, n)
        }
        Shape::Ellipsoid => {
            let y = u * 2.0 - 1.0;
            let s = (1.0 - y * y).max(0.0).sqrt();
            let a = v * TAU;
            let d = [s * a.cos(), y, s * a.sin()];
            (d, d)
        }
        Shape::Cylinder => {
            let r_avg = (half[0] + half[2]) * 0.5;
            let side = TAU * r_avg * 2.0 * half[1];
            let caps = 2.0 * PI * r_avg * r_avg;
            let a = v * TAU;
            if r.f() * (side + caps) < side {
                let p = [a.cos(), u * 2.0 - 1.0, a.sin()];
                (p, [a.cos(), 0.0, a.sin()])
            } else {
                let rr = u.sqrt();
                let y = if r.f() < 0.5 { -1.0 } else { 1.0 };
                ([rr * a.cos(), y, rr * a.sin()], [0.0, y, 0.0])
            }
        }
        Shape::Cone => {
            let r_avg = (half[0] + half[2]) * 0.5;
            let slant = (r_avg * r_avg + 4.0 * half[1] * half[1]).sqrt();
            let side = PI * r_avg * slant;
            let base = PI * r_avg * r_avg;
            let a = v * TAU;
            if r.f() * (side + base) < side {
                // Density proportional to radius: f = 1 - sqrt(u) from base to tip.
                let f = 1.0 - r.f().sqrt();
                let rad = 1.0 - f;
                let p = [rad * a.cos(), -1.0 + 2.0 * f, rad * a.sin()];
                // In unit space the side slope is radius 1 over height 2.
                (p, norm([a.cos(), 0.5, a.sin()]))
            } else {
                let rr = u.sqrt();
                ([rr * a.cos(), -1.0, rr * a.sin()], [0.0, -1.0, 0.0])
            }
        }
        Shape::Torus => unreachable!("torus is sampled in world units"),
    }
}

/// Torus sample in world units (local frame, before rotation/translation).
fn sample_torus(size: V3, r: &mut Rng) -> (V3, V3) {
    let t = size[1] * 0.5;
    let a = (size[0] * 0.5 - t).max(t * 0.1);
    let c = (size[2] * 0.5 - t).max(t * 0.1);
    let th = r.f() * TAU;
    let ph = r.f() * TAU;
    let ring = [a * th.cos(), 0.0, c * th.sin()];
    // Outward direction of an ellipse at angle th.
    let out = norm([c * th.cos(), 0.0, a * th.sin()]);
    let n = add(scale(out, ph.cos()), [0.0, ph.sin(), 0.0]);
    (add(ring, scale(n, t)), n)
}

/// Simple baked lighting so flat colours still read as 3D shapes.
fn shade(color: V3, n: V3) -> V3 {
    let key = norm([0.45, 0.8, 0.55]);
    let fill = norm([-0.6, 0.25, -0.4]);
    let l = 0.42 + 0.48 * dot(n, key).max(0.0) + 0.16 * dot(n, fill).max(0.0);
    color.map(|c| (c * l).clamp(0.0, 1.0))
}

/// Samples every part's surface into splats. `budget` is the approximate
/// total; each part gets a share proportional to its area (at least 150).
pub fn to_splats(d: &Design, budget: usize) -> Vec<Splat> {
    let areas: Vec<f32> = d.parts.iter().map(area).collect();
    let total: f32 = areas.iter().sum::<f32>().max(1e-6);
    let mut out = Vec::with_capacity(budget + d.parts.len() * 150);
    for (i, (p, a)) in d.parts.iter().zip(&areas).enumerate() {
        let n = ((budget as f32 * a / total) as usize).max(150);
        // Splat footprint covers the part's area without gaps.
        let spacing = (a / n as f32).sqrt();
        let tangent = spacing * 0.75;
        let thick = (spacing * 0.18).max(1e-4);
        let m = rot_matrix(p.rotation);
        let half = scale(p.size, 0.5);
        let mut rng = Rng::new(i as u64 + 1);
        for _ in 0..n {
            let (lp, ln) = if p.shape == Shape::Torus {
                sample_torus(p.size, &mut rng)
            } else {
                let (up, un) = sample_unit(p.shape, half, &mut rng);
                let lp = [up[0] * half[0], up[1] * half[1], up[2] * half[2]];
                // Normals transform by the inverse scale.
                let ln = norm([un[0] / half[0], un[1] / half[1], un[2] / half[2]]);
                (lp, ln)
            };
            let pos = add(mul(&m, lp), p.center);
            let nrm = norm(mul(&m, ln));
            out.push(Splat {
                pos,
                scale: [tangent, tangent, thick],
                rot: quat_z_to(nrm),
                color: shade(p.color, nrm),
                alpha: 0.97,
            });
        }
    }
    out
}

/// Builds the final PLY bytes for a design.
pub fn to_ply(d: &Design, budget: usize) -> Vec<u8> {
    encode(
        &to_splats(d, budget),
        &PlyOptions {
            comment: Some("generated by 3D Maker from a shape design".into()),
            ..PlyOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ply::{PlyLimits, validate_reader};

    const TEAPOT: &str = r##"Here you go:
```json
{"name":"teapot","parts":[
 {"label":"body","shape":"sphere","center":[0,0.5,0],"size":[1,0.8,1],"color":"#1f4fd1"},
 {"label":"lid","shape":"cylinder","center":[0,0.95,0],"size":[0.4,0.08,0.4],"color":"#b5892e"},
 {"label":"spout","shape":"cone","center":[0.6,0.6,0],"size":[0.12,0.5,0.12],"rotation":[0,0,-50],"color":"#1f4fd1"},
 {"label":"handle","shape":"torus","center":[-0.55,0.5,0],"size":[0.45,0.06,0.45],"rotation":[90,0,0],"color":"#1f4fd1"},
 {"label":"base","shape":"box","center":[0,0.08,0],"size":[0.6,0.06,0.6],"color":[30,60,200]}
]}
```
Enjoy!"##;

    #[test]
    fn parses_fenced_answer_with_aliases() {
        let d = parse(TEAPOT).unwrap();
        assert_eq!(d.name, "teapot");
        assert_eq!(d.parts.len(), 5);
        assert_eq!(d.parts[0].shape, Shape::Ellipsoid);
        assert_eq!(d.parts[3].shape, Shape::Torus);
        assert_eq!(d.parts[1].rotation, [0.0; 3]);
        assert!((d.parts[4].color[2] - 200.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_bad_designs_with_reasons() {
        assert_eq!(parse("no json here"), Err(DesignError::NoJson));
        assert_eq!(parse(r#"{"parts":[]}"#), Err(DesignError::Empty));
        let e = parse(r#"{"parts":[{"shape":"blob","center":[0,0,0],"size":1}]}"#).unwrap_err();
        assert!(e.to_string().contains("unknown shape `blob`"), "{e}");
        let e =
            parse(r#"{"parts":[{"shape":"box","center":[0,0,0],"size":[1,0,1]}]}"#).unwrap_err();
        assert!(e.to_string().contains("size"), "{e}");
        let e = parse(r#"{"parts":[{"shape":"box","center":[0,1e9,0],"size":1}]}"#).unwrap_err();
        assert!(e.to_string().contains("center"), "{e}");
        let many = format!(
            r#"{{"parts":[{}]}}"#,
            vec![r#"{"shape":"box","center":[0,0,0],"size":1}"#; MAX_PARTS + 1].join(",")
        );
        assert_eq!(parse(&many), Err(DesignError::TooManyParts(MAX_PARTS + 1)));
    }

    #[test]
    fn braces_inside_strings_do_not_confuse_extraction() {
        let d = parse(r#"{"name":"odd } name","parts":[{"shape":"box","center":[0,0,0],"size":1,"label":"{"}]}"#).unwrap();
        assert_eq!(d.name, "odd } name");
    }

    #[test]
    fn splats_are_valid_deterministic_and_on_the_surface() {
        let d = parse(TEAPOT).unwrap();
        let a = to_ply(&d, 20_000);
        assert_eq!(
            a,
            to_ply(&d, 20_000),
            "same design must rebuild identically"
        );
        let s = validate_reader(
            std::io::Cursor::new(&a),
            a.len() as u64,
            &PlyLimits::default(),
        )
        .unwrap();
        // Per-part shares round down, so the total may fall a few short.
        assert!((19_990..21_000).contains(&s.count), "count {}", s.count);
        assert_eq!(s.sh_degree, 0);
        // Everything lies within the parts' extents (body top ≈ 0.9, lid ≈ 1.0).
        assert!(
            s.bounds.max[1] < 1.1 && s.bounds.min[1] > -0.05,
            "{:?}",
            s.bounds
        );

        // Ellipsoid samples sit on the surface.
        let body = Design {
            name: "b".into(),
            parts: vec![d.parts[0].clone()],
        };
        for sp in to_splats(&body, 2_000) {
            let q = [sp.pos[0] / 0.5, (sp.pos[1] - 0.5) / 0.4, sp.pos[2] / 0.5];
            assert!((dot(q, q) - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn rotation_and_torus_geometry() {
        // A 90° turn about z maps +x to +y.
        let m = rot_matrix([0.0, 0.0, 90.0]);
        let v = mul(&m, [1.0, 0.0, 0.0]);
        assert!((v[1] - 1.0).abs() < 1e-6 && v[0].abs() < 1e-6);
        // Torus samples keep their tube distance from the ring.
        let mut r = Rng::new(7);
        for _ in 0..500 {
            let (p, _) = sample_torus([1.0, 0.2, 1.0], &mut r);
            let ring = (p[0] * p[0] + p[2] * p[2]).sqrt();
            let d = ((ring - 0.4).powi(2) + p[1] * p[1]).sqrt();
            assert!((d - 0.1).abs() < 1e-4, "{d}");
        }
        // The quaternion really turns +z onto the normal.
        let n = norm([0.3, -0.5, 0.8]);
        let q = quat_z_to(n);
        let (w, x, y, z) = (q[0], q[1], q[2], q[3]);
        let rz = [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ];
        assert!((dot(rz, n) - 1.0).abs() < 1e-5);
    }
}
