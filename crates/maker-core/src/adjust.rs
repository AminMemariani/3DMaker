//! Prompt-driven adjustments for any splat (imported files and meshes).
//!
//! The model never sees the splats themselves. It gets a short summary
//! (size, main colours and where they sit) and answers with a list of edit
//! operations, which are applied here. Everything happens in the viewer's
//! Y-up space: the source's viewer orientation is baked in first, so
//! "top", "left" and "front" mean what the user sees.
//!
//! Approximations, by design: a rotation or mirror drops the view-dependent
//! colour terms (`f_rest_*`), which would otherwise need SH rotation; a
//! non-uniform scale stretches each splat along its own axes.

use crate::design::{mul, rot_matrix};
use crate::fixtures::SH_C0;
use crate::ply::SplatTable;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_OPS: usize = 40;

/// Where an operation applies. All given conditions must hold.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    /// Fractions of the current bounding box per axis, 0 = min, 1 = max
    /// (x: left→right, y: bottom→top, z: back→front).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<[f32; 2]>,
    /// Splats whose colour is near this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<[f32; 3]>,
    /// 0..1, colour distance allowed (default 0.25).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Op {
    Scale {
        factor: [f32; 3],
        #[serde(default)]
        r#where: Option<Region>,
    },
    Rotate {
        degrees: [f32; 3],
        #[serde(default)]
        r#where: Option<Region>,
    },
    /// Offset as a fraction of the object's largest dimension.
    Translate {
        offset: [f32; 3],
        #[serde(default)]
        r#where: Option<Region>,
    },
    Mirror {
        axis: char,
    },
    Color {
        color: [f32; 3],
        strength: f32,
        #[serde(default)]
        r#where: Option<Region>,
    },
    Brightness {
        factor: f32,
        #[serde(default)]
        r#where: Option<Region>,
    },
    Saturation {
        factor: f32,
        #[serde(default)]
        r#where: Option<Region>,
    },
    Opacity {
        factor: f32,
        #[serde(default)]
        r#where: Option<Region>,
    },
    Delete {
        r#where: Region,
    },
    Keep {
        r#where: Region,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub operations: Vec<Op>,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AdjustError {
    #[error("the answer contained no JSON object")]
    NoJson,
    #[error("the JSON could not be read: {0}")]
    BadJson(String),
    #[error("no operations were given")]
    Empty,
    #[error("operation {index}: {problem}")]
    BadOp { index: usize, problem: String },
    #[error("the file is missing required splat properties")]
    BadTable,
    #[error("the adjustment removed every splat")]
    NothingLeft,
}

pub fn system_prompt() -> String {
    format!(
        r##"You are a 3D splat editor. The user describes one change to an existing 3D object. You see a summary of the object, not the object itself. Reply with a single JSON object and nothing else:

{{"summary": "what you changed, in a few words", "operations": [ ... ]}}

Coordinates: Y is up, X is left→right, Z is back→front, as the user sees it.
Regions ("where") select splats. Every field is optional and all given fields must match:
  {{"x": [from, to], "y": [from, to], "z": [from, to], "color": "#rrggbb", "tolerance": 0.25}}
  x/y/z are fractions of the object's bounding box (0 = min, 1 = max), so {{"y": [0.7, 1]}} is the top 30%.
  "color" picks splats near that colour; use the colours from the summary. tolerance is 0..1.

Operations (omit "where" to affect the whole object):
  {{"op": "color", "color": "#rrggbb", "strength": 0..1, "where": ...}}   recolour, keeps shading
  {{"op": "brightness", "factor": 1.3, "where": ...}}
  {{"op": "saturation", "factor": 0.5, "where": ...}}                      0 = grey
  {{"op": "opacity", "factor": 0.5, "where": ...}}                         make see-through
  {{"op": "scale", "factor": 1.5 or [x, y, z], "where": ...}}              about the region's centre
  {{"op": "rotate", "degrees": [x, y, z], "where": ...}}                   about the region's centre
  {{"op": "translate", "offset": [x, y, z], "where": ...}}                 fractions of the object's size
  {{"op": "mirror", "axis": "x" | "y" | "z"}}
  {{"op": "delete", "where": ...}}                                         remove a part
  {{"op": "keep", "where": ...}}                                           remove everything else

Use the fewest operations that do what the user asked (at most {MAX_OPS}). If the request can't be done with these operations, do the closest reasonable thing and say so in "summary"."##
    )
}

pub fn user_prompt(summary: &str, instruction: &str) -> String {
    format!(
        "Object summary:\n{summary}\n\nChange request:\n{}",
        instruction.trim()
    )
}

// ---------- parsing ----------

fn num(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
    .filter(|f: &f32| f.is_finite())
}

fn vec3(v: &Value) -> Option<[f32; 3]> {
    match v {
        Value::Array(a) if a.len() == 3 => Some([num(&a[0])?, num(&a[1])?, num(&a[2])?]),
        Value::Object(o) => Some([num(o.get("x")?)?, num(o.get("y")?)?, num(o.get("z")?)?]),
        other => num(other).map(|f| [f; 3]),
    }
}

fn color(v: &Value) -> Option<[f32; 3]> {
    if let Some(s) = v.as_str() {
        let h = s.trim().trim_start_matches('#');
        if h.len() != 6 {
            return None;
        }
        let c = |i: usize| {
            u8::from_str_radix(&h[i..i + 2], 16)
                .ok()
                .map(|b| b as f32 / 255.0)
        };
        return Some([c(0)?, c(2)?, c(4)?]);
    }
    let c = vec3(v)?;
    let k = if c.iter().any(|&x| x > 1.0) {
        255.0
    } else {
        1.0
    };
    Some(c.map(|x| (x / k).clamp(0.0, 1.0)))
}

fn range(v: Option<&Value>) -> Result<Option<[f32; 2]>, String> {
    let Some(v) = v else { return Ok(None) };
    let a = v
        .as_array()
        .filter(|a| a.len() == 2)
        .ok_or("a region range needs [from, to]")?;
    let (lo, hi) = (
        num(&a[0]).ok_or("bad range")?,
        num(&a[1]).ok_or("bad range")?,
    );
    Ok(Some([
        lo.min(hi).clamp(-0.5, 1.5),
        lo.max(hi).clamp(-0.5, 1.5),
    ]))
}

fn region(v: Option<&Value>) -> Result<Option<Region>, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let o = v.as_object().ok_or("\"where\" must be an object")?;
    let r = Region {
        x: range(o.get("x"))?,
        y: range(o.get("y"))?,
        z: range(o.get("z"))?,
        color: match o.get("color") {
            Some(c) => Some(color(c).ok_or("region colour must be #rrggbb")?),
            None => None,
        },
        tolerance: o.get("tolerance").and_then(num).map(|t| t.clamp(0.01, 1.0)),
    };
    Ok(Some(r))
}

fn factor(v: Option<&Value>, lo: f32, hi: f32) -> Result<f32, String> {
    let f = v.and_then(num).ok_or("needs a numeric factor")?;
    if !(lo..=hi).contains(&f) {
        return Err(format!("factor must be between {lo} and {hi}"));
    }
    Ok(f)
}

pub fn parse(text: &str) -> Result<Plan, AdjustError> {
    let start = text.find('{').ok_or(AdjustError::NoJson)?;
    let end = text.rfind('}').ok_or(AdjustError::NoJson)?;
    let root: Value = serde_json::from_str(&text[start..=end])
        .map_err(|e| AdjustError::BadJson(e.to_string()))?;
    let ops = root
        .get("operations")
        .or_else(|| root.get("ops"))
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or(AdjustError::Empty)?;
    if ops.len() > MAX_OPS {
        return Err(AdjustError::BadOp {
            index: MAX_OPS,
            problem: format!("at most {MAX_OPS} operations"),
        });
    }
    let mut out = Vec::new();
    for (index, o) in ops.iter().enumerate() {
        let bad = |problem: String| AdjustError::BadOp { index, problem };
        let name = o
            .get("op")
            .or_else(|| o.get("type"))
            .and_then(Value::as_str)
            .ok_or_else(|| bad("has no \"op\"".into()))?;
        let w = region(o.get("where")).map_err(bad)?;
        let op = match name.to_ascii_lowercase().as_str() {
            "scale" | "resize" => {
                let f = o
                    .get("factor")
                    .and_then(vec3)
                    .ok_or_else(|| bad("scale needs a factor".into()))?;
                if f.iter().any(|&x| !(0.05..=20.0).contains(&x)) {
                    return Err(bad("scale factors must be between 0.05 and 20".into()));
                }
                Op::Scale {
                    factor: f,
                    r#where: w,
                }
            }
            "rotate" => Op::Rotate {
                degrees: o
                    .get("degrees")
                    .and_then(vec3)
                    .ok_or_else(|| bad("rotate needs degrees".into()))?,
                r#where: w,
            },
            "translate" | "move" => {
                let off = o
                    .get("offset")
                    .and_then(vec3)
                    .ok_or_else(|| bad("translate needs an offset".into()))?;
                Op::Translate {
                    offset: off.map(|x| x.clamp(-5.0, 5.0)),
                    r#where: w,
                }
            }
            "mirror" | "flip" => {
                let a = o
                    .get("axis")
                    .and_then(Value::as_str)
                    .and_then(|s| s.chars().next())
                    .map(|c| c.to_ascii_lowercase());
                match a {
                    Some(c @ ('x' | 'y' | 'z')) => Op::Mirror { axis: c },
                    _ => return Err(bad("mirror needs axis x, y or z".into())),
                }
            }
            "color" | "colour" | "recolor" | "tint" => Op::Color {
                color: o
                    .get("color")
                    .or_else(|| o.get("colour"))
                    .and_then(color)
                    .ok_or_else(|| bad("color needs #rrggbb".into()))?,
                strength: o
                    .get("strength")
                    .and_then(num)
                    .unwrap_or(1.0)
                    .clamp(0.0, 1.0),
                r#where: w,
            },
            "brightness" => Op::Brightness {
                factor: factor(o.get("factor"), 0.0, 5.0).map_err(bad)?,
                r#where: w,
            },
            "saturation" => Op::Saturation {
                factor: factor(o.get("factor"), 0.0, 5.0).map_err(bad)?,
                r#where: w,
            },
            "opacity" | "transparency" => Op::Opacity {
                factor: factor(o.get("factor"), 0.0, 1.0).map_err(bad)?,
                r#where: w,
            },
            "delete" | "remove" => Op::Delete {
                r#where: w.ok_or_else(|| bad("delete needs a \"where\"".into()))?,
            },
            "keep" | "crop" | "isolate" => Op::Keep {
                r#where: w.ok_or_else(|| bad("keep needs a \"where\"".into()))?,
            },
            other => return Err(bad(format!("unknown operation `{other}`"))),
        };
        out.push(op);
    }
    Ok(Plan {
        operations: out,
        summary: root
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(200)
            .collect(),
    })
}

// ---------- applying ----------

struct Cols {
    pos: [usize; 3],
    scale: [usize; 3],
    rot: [usize; 4],
    opacity: usize,
    dc: [usize; 3],
    rest: Vec<usize>,
}

fn cols(t: &SplatTable) -> Result<Cols, AdjustError> {
    let c = |n: &str| t.col(n).ok_or(AdjustError::BadTable);
    Ok(Cols {
        pos: [c("x")?, c("y")?, c("z")?],
        scale: [c("scale_0")?, c("scale_1")?, c("scale_2")?],
        rot: [c("rot_0")?, c("rot_1")?, c("rot_2")?, c("rot_3")?],
        opacity: c("opacity")?,
        dc: [c("f_dc_0")?, c("f_dc_1")?, c("f_dc_2")?],
        rest: t
            .names
            .iter()
            .enumerate()
            .filter(|(_, n)| n.starts_with("f_rest_"))
            .map(|(i, _)| i)
            .collect(),
    })
}

type Q = [f32; 4];

fn qmul(a: Q, b: Q) -> Q {
    [
        a[0] * b[0] - a[1] * b[1] - a[2] * b[2] - a[3] * b[3],
        a[0] * b[1] + a[1] * b[0] + a[2] * b[3] - a[3] * b[2],
        a[0] * b[2] - a[1] * b[3] + a[2] * b[0] + a[3] * b[1],
        a[0] * b[3] + a[1] * b[2] - a[2] * b[1] + a[3] * b[0],
    ]
}

/// Quaternion (w, x, y, z) for a rotation matrix (Shepperd's method).
fn quat_from(m: &[[f32; 3]; 3]) -> Q {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [
            0.25 * s,
            (m[2][1] - m[1][2]) / s,
            (m[0][2] - m[2][0]) / s,
            (m[1][0] - m[0][1]) / s,
        ]
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        [
            (m[2][1] - m[1][2]) / s,
            0.25 * s,
            (m[0][1] + m[1][0]) / s,
            (m[0][2] + m[2][0]) / s,
        ]
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        [
            (m[0][2] - m[2][0]) / s,
            (m[0][1] + m[1][0]) / s,
            0.25 * s,
            (m[1][2] + m[2][1]) / s,
        ]
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        [
            (m[1][0] - m[0][1]) / s,
            (m[0][2] + m[2][0]) / s,
            (m[1][2] + m[2][1]) / s,
            0.25 * s,
        ]
    };
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    q.map(|v| v / l)
}

/// Rotation matrix of a (not necessarily normalized) quaternion.
fn mat_of(q: Q) -> [[f32; 3]; 3] {
    let l = q.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-12);
    let [w, x, y, z] = q.map(|v| v / l);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

fn color_of(row: &[f32], c: &Cols) -> [f32; 3] {
    c.dc.map(|i| (0.5 + SH_C0 * row[i]).clamp(0.0, 1.0))
}
fn set_color(row: &mut [f32], c: &Cols, rgb: [f32; 3]) {
    for (k, &i) in c.dc.iter().enumerate() {
        row[i] = (rgb[k].clamp(0.0, 1.0) - 0.5) / SH_C0;
    }
}
fn lum(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Robust per-axis bounds (0.5% / 99.5% quantiles of a sample).
fn bounds(t: &SplatTable, c: &Cols, rows: &[usize]) -> ([f32; 3], [f32; 3]) {
    let step = (rows.len() / 100_000).max(1);
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for k in 0..3 {
        let mut v: Vec<f32> = rows
            .iter()
            .step_by(step)
            .map(|&r| t.data[r * t.width() + c.pos[k]])
            .collect();
        if v.is_empty() {
            continue;
        }
        v.sort_by(f32::total_cmp);
        let q = |f: f32| v[((v.len() - 1) as f32 * f).round() as usize];
        lo[k] = q(0.005);
        hi[k] = q(0.995);
    }
    (lo, hi)
}

fn all_rows(t: &SplatTable) -> Vec<usize> {
    (0..t.len()).collect()
}

fn select(t: &SplatTable, c: &Cols, r: Option<&Region>) -> Vec<usize> {
    let Some(r) = r else { return all_rows(t) };
    let (lo, hi) = bounds(t, c, &all_rows(t));
    let w = t.width();
    let tol = r.tolerance.unwrap_or(0.25);
    (0..t.len())
        .filter(|&i| {
            let row = &t.data[i * w..(i + 1) * w];
            let inside = [r.x, r.y, r.z].iter().enumerate().all(|(k, rg)| match rg {
                None => true,
                Some([a, b]) => {
                    let span = (hi[k] - lo[k]).max(1e-9);
                    let f = (row[c.pos[k]] - lo[k]) / span;
                    f >= *a && f <= *b
                }
            });
            inside
                && r.color.is_none_or(|target| {
                    let col = color_of(row, c);
                    let d = ((col[0] - target[0]).powi(2)
                        + (col[1] - target[1]).powi(2)
                        + (col[2] - target[2]).powi(2))
                    .sqrt();
                    d <= tol * 1.732
                })
        })
        .collect()
}

/// Rotates (and optionally mirrors) positions about `center` and turns each
/// splat with it. `m` must be orthogonal.
fn transform_rigid(
    t: &mut SplatTable,
    c: &Cols,
    rows: &[usize],
    m: &[[f32; 3]; 3],
    center: [f32; 3],
    mirror: Option<usize>,
) {
    let w = t.width();
    let qr = if mirror.is_none() {
        Some(quat_from(m))
    } else {
        None
    };
    for &i in rows {
        let row = &mut t.data[i * w..(i + 1) * w];
        let p = [0, 1, 2].map(|k| row[c.pos[k]] - center[k]);
        let np = mul(m, p);
        for k in 0..3 {
            row[c.pos[k]] = np[k] + center[k];
        }
        let q = c.rot.map(|k| row[k]);
        let nq = match (qr, mirror) {
            (Some(r), _) => qmul(r, q),
            // Reflecting across axis a: M' = P M P, i.e. negate the other two imaginary parts.
            (None, Some(a)) => {
                let mut v = q;
                for k in 0..3 {
                    if k != a {
                        v[k + 1] = -v[k + 1];
                    }
                }
                v
            }
            (None, None) => q,
        };
        for k in 0..4 {
            row[c.rot[k]] = nq[k];
        }
        // View-dependent colour can't follow a rotation without SH rotation.
        for &r in &c.rest {
            row[r] = 0.0;
        }
    }
}

/// Bakes the viewer orientation into the data so edits happen in Y-up space.
pub fn bake_view_rotation(t: &mut SplatTable, view_rotation: [f32; 3]) -> Result<(), AdjustError> {
    if view_rotation.iter().all(|a| a.abs() < 1e-6) {
        return Ok(());
    }
    let c = cols(t)?;
    let rows = all_rows(t);
    transform_rigid(t, &c, &rows, &rot_matrix(view_rotation), [0.0; 3], None);
    Ok(())
}

/// Applies a plan in order. Returns the number of splats left.
pub fn apply(t: &mut SplatTable, plan: &Plan) -> Result<usize, AdjustError> {
    let c = cols(t)?;
    let w = t.width();
    for op in &plan.operations {
        match op {
            Op::Scale { factor, r#where }
            | Op::Translate {
                offset: factor,
                r#where,
            }
            | Op::Rotate {
                degrees: factor,
                r#where,
            } => {
                let rows = select(t, &c, r#where.as_ref());
                if rows.is_empty() {
                    continue;
                }
                let (lo, hi) = bounds(t, &c, &rows);
                let center = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
                match op {
                    Op::Scale { .. } => {
                        let f = *factor;
                        for &i in &rows {
                            let row = &mut t.data[i * w..(i + 1) * w];
                            for k in 0..3 {
                                row[c.pos[k]] = center[k] + (row[c.pos[k]] - center[k]) * f[k];
                            }
                            // Stretch each splat along its own axes by how much
                            // the scale lengthens that axis.
                            let m = mat_of(c.rot.map(|k| row[k]));
                            for a in 0..3 {
                                let d = [m[0][a] * f[0], m[1][a] * f[1], m[2][a] * f[2]];
                                let len =
                                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt().max(1e-6);
                                row[c.scale[a]] += len.ln();
                            }
                        }
                    }
                    Op::Translate { .. } => {
                        let (glo, ghi) = bounds(t, &c, &all_rows(t));
                        let size = (0..3).map(|k| ghi[k] - glo[k]).fold(0.0f32, f32::max);
                        for &i in &rows {
                            for (p, f) in c.pos.iter().zip(factor) {
                                t.data[i * w + p] += f * size;
                            }
                        }
                    }
                    _ => transform_rigid(t, &c, &rows, &rot_matrix(*factor), center, None),
                }
            }
            Op::Mirror { axis } => {
                let a = match axis {
                    'x' => 0,
                    'y' => 1,
                    _ => 2,
                };
                let rows = all_rows(t);
                let (lo, hi) = bounds(t, &c, &rows);
                let center = [0, 1, 2].map(|k| (lo[k] + hi[k]) * 0.5);
                let mut m = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
                m[a][a] = -1.0;
                transform_rigid(t, &c, &rows, &m, center, Some(a));
            }
            Op::Color {
                color: target,
                strength,
                r#where,
            } => {
                let tl = lum(*target).max(0.05);
                for i in select(t, &c, r#where.as_ref()) {
                    let row = &mut t.data[i * w..(i + 1) * w];
                    let cur = color_of(row, &c);
                    // Keep the original shading: scale the target to this splat's brightness.
                    let k = (lum(cur) / tl).min(2.0);
                    let new = [0, 1, 2].map(|j| cur[j] + (target[j] * k - cur[j]) * strength);
                    set_color(row, &c, new);
                    for &r in &c.rest {
                        row[r] *= 1.0 - strength;
                    }
                }
            }
            Op::Brightness { factor, r#where } | Op::Saturation { factor, r#where } => {
                let sat = matches!(op, Op::Saturation { .. });
                for i in select(t, &c, r#where.as_ref()) {
                    let row = &mut t.data[i * w..(i + 1) * w];
                    let cur = color_of(row, &c);
                    let l = lum(cur);
                    let new = if sat {
                        cur.map(|x| l + (x - l) * factor)
                    } else {
                        cur.map(|x| x * factor)
                    };
                    set_color(row, &c, new);
                    for &r in &c.rest {
                        row[r] *= factor;
                    }
                }
            }
            Op::Opacity { factor, r#where } => {
                for i in select(t, &c, r#where.as_ref()) {
                    let o = &mut t.data[i * w + c.opacity];
                    let a = (1.0 / (1.0 + (-*o).exp()) * factor).clamp(1e-4, 1.0 - 1e-4);
                    *o = (a / (1.0 - a)).ln();
                }
            }
            Op::Delete { r#where } | Op::Keep { r#where } => {
                // Delete keeps everything outside the region; Keep only what's inside.
                let inside_kept = matches!(op, Op::Keep { .. });
                let mut mark = vec![!inside_kept; t.len()];
                for i in select(t, &c, Some(r#where)) {
                    mark[i] = inside_kept;
                }
                let mut data = Vec::with_capacity(t.data.len());
                for (i, keep) in mark.iter().enumerate() {
                    if *keep {
                        data.extend_from_slice(&t.data[i * w..(i + 1) * w]);
                    }
                }
                t.data = data;
            }
        }
    }
    if t.is_empty() {
        return Err(AdjustError::NothingLeft);
    }
    Ok(t.len())
}

/// A short text summary for the model: size, and the main colours with their
/// share and where they sit.
pub fn summarize(t: &SplatTable) -> Result<String, AdjustError> {
    let c = cols(t)?;
    let rows = all_rows(t);
    let (lo, hi) = bounds(t, &c, &rows);
    let w = t.width();
    let mut bins: std::collections::HashMap<[u8; 3], (usize, [f64; 3], [f64; 3])> =
        Default::default();
    let step = (t.len() / 200_000).max(1);
    let mut n = 0usize;
    for &i in rows.iter().step_by(step) {
        let row = &t.data[i * w..(i + 1) * w];
        let col = color_of(row, &c);
        let key = col.map(|x| (x * 5.0).round() as u8);
        let e = bins.entry(key).or_insert((0, [0.0; 3], [0.0; 3]));
        e.0 += 1;
        for k in 0..3 {
            e.1[k] += col[k] as f64;
            let span = (hi[k] - lo[k]).max(1e-9);
            e.2[k] += ((row[c.pos[k]] - lo[k]) / span) as f64;
        }
        n += 1;
    }
    let mut list: Vec<_> = bins.into_values().collect();
    list.sort_by_key(|b| std::cmp::Reverse(b.0));
    let size = [0, 1, 2].map(|k| hi[k] - lo[k]);
    let mut s = format!(
        "{} splats. Size (width x height x depth): {:.3} x {:.3} x {:.3}.\nMain colours (share, average position as x/y/z fractions):\n",
        t.len(),
        size[0],
        size[1],
        size[2]
    );
    for (count, csum, psum) in list.iter().take(8) {
        let k = *count as f64;
        let hex: String = csum
            .iter()
            .map(|v| format!("{:02x}", ((v / k).clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        s.push_str(&format!(
            "- #{hex}: {:.0}%, at x {:.2} y {:.2} z {:.2}\n",
            k / n.max(1) as f64 * 100.0,
            psum[0] / k,
            psum[1] / k,
            psum[2] / k
        ));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{PlyOptions, Splat, encode};

    fn table(splats: &[Splat], sh: u8) -> SplatTable {
        let bytes = encode(
            splats,
            &PlyOptions {
                sh_degree: sh,
                ..Default::default()
            },
        );
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("t.ply");
        std::fs::write(&p, bytes).unwrap();
        crate::ply::load_table(&p, &Default::default()).unwrap()
    }

    fn two_tone() -> Vec<Splat> {
        // Blue bottom half, yellow top half.
        (0..2000)
            .map(|i| {
                let y = i as f32 / 2000.0;
                let x = ((i * 37) % 100) as f32 / 100.0;
                Splat::sphere(
                    [x, y, 0.0],
                    0.01,
                    if y < 0.5 {
                        [0.1, 0.2, 0.9]
                    } else {
                        [0.9, 0.9, 0.1]
                    },
                )
            })
            .collect()
    }

    #[test]
    fn parses_aliases_and_rejects_bad_ops() {
        let p = parse(r##"Sure: {"summary":"red top","operations":[{"op":"recolor","color":"#ff0000","where":{"y":[0.5,1]}},{"op":"scale","factor":2},{"op":"flip","axis":"X"}]}"##).unwrap();
        assert_eq!(p.operations.len(), 3);
        assert!(matches!(
            p.operations[1],
            Op::Scale {
                factor: [2.0, 2.0, 2.0],
                ..
            }
        ));
        assert!(matches!(p.operations[2], Op::Mirror { axis: 'x' }));
        assert_eq!(parse("nope"), Err(AdjustError::NoJson));
        assert_eq!(parse(r#"{"operations":[]}"#), Err(AdjustError::Empty));
        assert!(
            parse(r#"{"operations":[{"op":"explode"}]}"#)
                .unwrap_err()
                .to_string()
                .contains("explode")
        );
        assert!(parse(r#"{"operations":[{"op":"delete"}]}"#).is_err());
        assert!(parse(r#"{"operations":[{"op":"scale","factor":1000}]}"#).is_err());
    }

    #[test]
    fn recolour_by_region_keeps_the_rest() {
        let mut t = table(&two_tone(), 1);
        let plan = parse(r##"{"operations":[{"op":"color","color":"#ff0000","where":{"color":"#1a33e6","tolerance":0.2}}]}"##).unwrap();
        apply(&mut t, &plan).unwrap();
        let c = cols(&t).unwrap();
        let w = t.width();
        let bottom = color_of(&t.data[10 * w..11 * w], &c);
        let top = color_of(&t.data[1900 * w..1901 * w], &c);
        assert!(bottom[0] > bottom[2], "bottom became red-ish: {bottom:?}");
        assert!(
            (top[0] - 0.9).abs() < 0.01 && (top[2] - 0.1).abs() < 0.01,
            "top unchanged: {top:?}"
        );
    }

    #[test]
    fn delete_keep_scale_rotate_mirror() {
        let base = table(&two_tone(), 0);
        let mut t = base.clone();
        apply(
            &mut t,
            &parse(r#"{"operations":[{"op":"delete","where":{"y":[0.5,1.5]}}]}"#).unwrap(),
        )
        .unwrap();
        assert!((990..1010).contains(&t.len()), "{}", t.len());
        let mut k = base.clone();
        apply(
            &mut k,
            &parse(r#"{"operations":[{"op":"keep","where":{"y":[0.5,1.5]}}]}"#).unwrap(),
        )
        .unwrap();
        assert_eq!(k.len() + t.len(), base.len());
        assert_eq!(
            apply(
                &mut base.clone(),
                &parse(r#"{"operations":[{"op":"keep","where":{"y":[2,3]}}]}"#).unwrap()
            ),
            Err(AdjustError::NothingLeft)
        );

        let mut s = base.clone();
        apply(
            &mut s,
            &parse(r#"{"operations":[{"op":"scale","factor":[1,2,1]}]}"#).unwrap(),
        )
        .unwrap();
        let c = cols(&s).unwrap();
        let (lo, hi) = bounds(&s, &c, &all_rows(&s));
        assert!((hi[1] - lo[1]) > 1.9, "height doubled: {}", hi[1] - lo[1]);

        let mut r = base.clone();
        apply(
            &mut r,
            &parse(r#"{"operations":[{"op":"rotate","degrees":[0,0,90]}]}"#).unwrap(),
        )
        .unwrap();
        // Splat 1999 sits at (0.63, 1.0); a 90° turn about z around the
        // centre (0.5, 0.5) moves it to about (0.0, 0.63).
        let p = &r.data[1999 * r.width()..];
        assert!(
            p[c.pos[0]].abs() < 0.03 && (p[c.pos[1]] - 0.63).abs() < 0.03,
            "{:?}",
            (p[c.pos[0]], p[c.pos[1]])
        );

        let mut m = base.clone();
        apply(
            &mut m,
            &parse(r#"{"operations":[{"op":"mirror","axis":"y"}]}"#).unwrap(),
        )
        .unwrap();
        let first = color_of(&m.data[0..m.width()], &c);
        let y0 = m.data[c.pos[1]];
        assert!(
            y0 > 0.9 && first[2] > 0.8,
            "the blue bottom splat is now at the top"
        );
    }

    #[test]
    fn summary_lists_main_colours_and_baking_flips_y_down() {
        let t = table(&two_tone(), 0);
        let s = summarize(&t).unwrap();
        assert!(s.contains("2000 splats") && s.contains("50%"), "{s}");
        let mut b = t.clone();
        bake_view_rotation(&mut b, [180.0, 0.0, 0.0]).unwrap();
        let c = cols(&b).unwrap();
        assert!(b.data[c.pos[1]] <= 0.0 && b.data[1999 * b.width() + c.pos[1]] < -0.9);
    }
}
