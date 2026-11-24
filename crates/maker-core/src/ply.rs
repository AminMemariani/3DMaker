//! Validation of 3D Gaussian Splatting PLY files.
//!
//! The interchange format is the de-facto 3DGS layout written by the reference
//! implementation (and by TRELLIS / SAM 3D): an uncompressed
//! `binary_little_endian` PLY with one `vertex` element whose properties are
//! read **by name and type** (never by position):
//!
//! * `x y z` — centre
//! * `scale_0..2` — natural-log scale per axis (negative values are normal)
//! * `rot_0..3` — quaternion, `rot_0` = w (3DGS convention)
//! * `opacity` — logit (pre-sigmoid) opacity
//! * `f_dc_0..2` — degree-0 spherical-harmonic colour
//! * `f_rest_*` — optional higher-order SH: 9, 24 or 45 coefficients (degree 1–3)
//! * anything else (normals, producer extras) is preserved but not interpreted.
//!
//! Validation streams the payload in bounded chunks, so memory use does not
//! depend on file size, and computes the SHA-256 of the exact bytes on the way.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// Upper bounds that make hostile or broken inputs fail fast.
#[derive(Debug, Clone, Copy)]
pub struct PlyLimits {
    pub max_file_bytes: u64,
    pub max_splats: u64,
    pub max_header_bytes: usize,
}

impl Default for PlyLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
            max_splats: 20_000_000,
            max_header_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScalarType {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl ScalarType {
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return None,
        })
    }
    pub fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }
    fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    pub name: String,
    pub ty: ScalarType,
    pub offset: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub center: [f32; 3],
    pub radius: f32,
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GaussianSummary {
    pub count: u64,
    pub sh_degree: u8,
    pub stride: usize,
    pub header_bytes: usize,
    pub file_bytes: u64,
    pub sha256: String,
    pub has_normals: bool,
    /// Properties that are carried through untouched (normals, producer extras).
    pub extra_properties: Vec<String>,
    pub bounds: Bounds,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum PlyError {
    #[error("This isn't a PLY file (it doesn't start with the PLY signature).")]
    NotPly,
    #[error(
        "This PLY is stored as {0}. 3D Maker needs uncompressed binary little-endian Gaussian splat files."
    )]
    UnsupportedFormat(String),
    #[error("The file header is damaged: {0}")]
    BadHeader(String),
    #[error(
        "The file is a 3D mesh, not a Gaussian splat. Export or request a Gaussian splat (.ply) instead."
    )]
    MeshOnly,
    #[error(
        "The file is a plain coloured point cloud (positions and colours only). It has no splat shape data ({0}), so it can't be shown as Gaussian splats."
    )]
    PointCloudOnly(String),
    #[error("The file is missing Gaussian splat data: {0}.")]
    MissingProperties(String),
    #[error("Property `{0}` has type {1:?}; Gaussian attributes must be floating point.")]
    WrongType(String, ScalarType),
    #[error(
        "This looks like a compressed or packed splat file (element `{0}`). Use an uncompressed .ply."
    )]
    CompressedOrUnknownElement(String),
    #[error("The file contains no splats.")]
    Empty,
    #[error("The file has {0} splats, more than the supported maximum of {1}.")]
    TooManySplats(u64, u64),
    #[error("The file is {0} bytes, larger than the supported maximum of {1} bytes.")]
    TooLarge(u64, u64),
    #[error(
        "The file is incomplete: expected {expected} bytes but found {found}. The download or copy was probably interrupted."
    )]
    Truncated { expected: u64, found: u64 },
    #[error("The file has {0} unexpected bytes after the splat data.")]
    TrailingBytes(u64),
    #[error(
        "Splat {index} has an invalid value in `{property}` (not a finite number). The file is corrupt."
    )]
    NonFinite { index: u64, property: String },
    #[error("Splat {0} has an all-zero rotation, which is invalid. The file is corrupt.")]
    DegenerateRotation(u64),
    #[error(
        "Higher-order colour data has {0} coefficients; expected 9, 24 or 45 named f_rest_0…f_rest_N."
    )]
    BadShCoefficients(usize),
    #[error("Could not read the file: {0}")]
    Io(String),
}

impl From<std::io::Error> for PlyError {
    fn from(e: std::io::Error) -> Self {
        PlyError::Io(e.to_string())
    }
}

const REQUIRED: [&str; 14] = [
    "x", "y", "z", "scale_0", "scale_1", "scale_2", "rot_0", "rot_1", "rot_2", "rot_3", "opacity",
    "f_dc_0", "f_dc_1", "f_dc_2",
];

#[derive(Debug)]
struct Header {
    header_bytes: usize,
    count: u64,
    props: Vec<Property>,
    stride: usize,
}

/// Parses the ASCII header. Reads at most `max_header_bytes`.
fn parse_header<R: BufRead>(r: &mut R, limits: &PlyLimits) -> Result<Header, PlyError> {
    let mut consumed = 0usize;
    let mut line = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    loop {
        line.clear();
        let budget = limits.max_header_bytes.saturating_sub(consumed) as u64;
        let n = (&mut *r).take(budget).read_until(b'\n', &mut line)?;
        if n == 0 {
            return Err(if lines.is_empty() {
                PlyError::NotPly
            } else {
                PlyError::BadHeader("no end_header line within the first 64 KB".into())
            });
        }
        consumed += n;
        if line.last() != Some(&b'\n') {
            return Err(if lines.is_empty() {
                PlyError::NotPly
            } else {
                PlyError::BadHeader("no end_header line within the first 64 KB".into())
            });
        }
        let text = std::str::from_utf8(&line)
            .map_err(|_| {
                if lines.is_empty() {
                    PlyError::NotPly
                } else {
                    PlyError::BadHeader("header is not text".into())
                }
            })?
            .trim_end_matches(['\n', '\r'])
            .to_string();
        if lines.is_empty() && text != "ply" {
            return Err(PlyError::NotPly);
        }
        let end = text.trim() == "end_header";
        lines.push(text);
        if end {
            break;
        }
    }

    let mut format_ok = false;
    let mut in_vertex = false;
    let mut seen_vertex = false;
    let mut count: Option<u64> = None;
    let mut props: Vec<Property> = Vec::new();
    let mut offset = 0usize;
    let mut has_faces = false;

    for l in lines.iter().skip(1) {
        let mut parts = l.split_whitespace();
        let Some(kw) = parts.next() else { continue };
        match kw {
            "format" => {
                let fmt = parts.next().unwrap_or("");
                match fmt {
                    "binary_little_endian" => format_ok = true,
                    "ascii" => return Err(PlyError::UnsupportedFormat("ASCII text".into())),
                    "binary_big_endian" => {
                        return Err(PlyError::UnsupportedFormat("big-endian binary".into()));
                    }
                    other => return Err(PlyError::BadHeader(format!("unknown format `{other}`"))),
                }
            }
            "comment" | "obj_info" | "end_header" => {}
            "element" => {
                let name = parts.next().unwrap_or("");
                let n: u64 = parts
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| PlyError::BadHeader(format!("bad element line `{l}`")))?;
                if name == "vertex" {
                    if seen_vertex {
                        return Err(PlyError::BadHeader("more than one vertex element".into()));
                    }
                    seen_vertex = true;
                    in_vertex = true;
                    count = Some(n);
                } else {
                    in_vertex = false;
                    if n == 0 {
                        continue; // harmless empty element
                    }
                    if name == "face" || name == "edge" {
                        has_faces = true;
                        continue;
                    }
                    return Err(PlyError::CompressedOrUnknownElement(name.to_string()));
                }
            }
            "property" => {
                if !in_vertex {
                    continue; // properties of empty / face elements
                }
                let ty = parts.next().unwrap_or("");
                if ty == "list" {
                    return Err(PlyError::BadHeader(
                        "list property inside the vertex element".into(),
                    ));
                }
                let ty = ScalarType::parse(ty).ok_or_else(|| {
                    PlyError::BadHeader(format!("unknown property type in `{l}`"))
                })?;
                let name = parts.next().ok_or_else(|| {
                    PlyError::BadHeader(format!("property without a name: `{l}`"))
                })?;
                if props.iter().any(|p| p.name == name) {
                    return Err(PlyError::BadHeader(format!("duplicate property `{name}`")));
                }
                props.push(Property {
                    name: name.to_string(),
                    ty,
                    offset,
                });
                offset = offset
                    .checked_add(ty.size())
                    .ok_or_else(|| PlyError::BadHeader("vertex record too large".into()))?;
            }
            other => {
                return Err(PlyError::BadHeader(format!(
                    "unexpected header keyword `{other}`"
                )));
            }
        }
    }
    if !format_ok {
        return Err(PlyError::BadHeader("missing format line".into()));
    }
    let has = |n: &str| props.iter().any(|p| p.name == n);
    let gaussian_props = ["scale_0", "rot_0", "opacity", "f_dc_0"]
        .iter()
        .any(|n| has(n));
    if has_faces && !gaussian_props {
        return Err(PlyError::MeshOnly);
    }
    if has_faces {
        // Faces would add payload after the vertices that splat viewers don't expect.
        return Err(PlyError::CompressedOrUnknownElement("face".into()));
    }
    let count = count.ok_or(PlyError::MissingProperties("no vertex element".into()))?;
    let missing: Vec<&str> = REQUIRED.iter().copied().filter(|n| !has(n)).collect();
    if !missing.is_empty() {
        let colour_only =
            has("x") && (has("red") || has("r") || has("diffuse_red")) && !gaussian_props;
        if colour_only || (has("x") && !gaussian_props) {
            return Err(PlyError::PointCloudOnly(missing.join(", ")));
        }
        return Err(PlyError::MissingProperties(missing.join(", ")));
    }
    for p in &props {
        let gaussian = REQUIRED.contains(&p.name.as_str()) || p.name.starts_with("f_rest_");
        if gaussian && !p.ty.is_float() {
            return Err(PlyError::WrongType(p.name.clone(), p.ty));
        }
    }
    if count == 0 {
        return Err(PlyError::Empty);
    }
    if count > limits.max_splats {
        return Err(PlyError::TooManySplats(count, limits.max_splats));
    }
    Ok(Header {
        header_bytes: consumed,
        count,
        props,
        stride: offset,
    })
}

fn sh_degree(props: &[Property]) -> Result<u8, PlyError> {
    let n = props
        .iter()
        .filter(|p| p.name.starts_with("f_rest_"))
        .count();
    let degree = match n {
        0 => 0,
        9 => 1,
        24 => 2,
        45 => 3,
        other => return Err(PlyError::BadShCoefficients(other)),
    };
    for i in 0..n {
        let name = format!("f_rest_{i}");
        if !props.iter().any(|p| p.name == name) {
            return Err(PlyError::BadShCoefficients(n));
        }
    }
    Ok(degree)
}

#[inline]
fn read_f64(buf: &[u8], p: &Property) -> f64 {
    let b = &buf[p.offset..p.offset + p.ty.size()];
    match p.ty {
        ScalarType::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        ScalarType::F64 => f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
        ScalarType::I8 => b[0] as i8 as f64,
        ScalarType::U8 => b[0] as f64,
        ScalarType::I16 => i16::from_le_bytes([b[0], b[1]]) as f64,
        ScalarType::U16 => u16::from_le_bytes([b[0], b[1]]) as f64,
        ScalarType::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
        ScalarType::U32 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f64,
    }
}

/// A validated splat file loaded into memory as float rows, for editing.
#[derive(Debug, Clone)]
pub struct SplatTable {
    /// Property names in file order.
    pub names: Vec<String>,
    /// Row-major values, `names.len()` per splat.
    pub data: Vec<f32>,
}

impl SplatTable {
    pub fn width(&self) -> usize {
        self.names.len()
    }
    pub fn len(&self) -> usize {
        self.data.len() / self.width().max(1)
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn col(&self, name: &str) -> Option<usize> {
        self.names.iter().position(|n| n == name)
    }
    /// Encodes as binary little-endian float PLY (all properties as `float`).
    pub fn encode(&self, comment: Option<&str>) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.data.len() * 4 + 1024);
        out.extend_from_slice(b"ply\nformat binary_little_endian 1.0\n");
        if let Some(c) = comment {
            out.extend_from_slice(format!("comment {c}\n").as_bytes());
        }
        out.extend_from_slice(format!("element vertex {}\n", self.len()).as_bytes());
        for n in &self.names {
            out.extend_from_slice(format!("property float {n}\n").as_bytes());
        }
        out.extend_from_slice(b"end_header\n");
        for v in &self.data {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// Validates `path` and loads every splat. Memory use is about 4 bytes per
/// property per splat, so callers should bound the file size.
pub fn load_table(path: &Path, limits: &PlyLimits) -> Result<SplatTable, PlyError> {
    let summary = validate_file(path, limits)?;
    let mut r = std::io::BufReader::new(std::fs::File::open(path)?);
    let header = parse_header(&mut r, limits)?;
    let names: Vec<String> = header.props.iter().map(|p| p.name.clone()).collect();
    let mut data = Vec::with_capacity(summary.count as usize * names.len());
    let mut row = vec![0u8; header.stride];
    for _ in 0..summary.count {
        r.read_exact(&mut row)?;
        data.extend(header.props.iter().map(|p| read_f64(&row, p) as f32));
    }
    Ok(SplatTable { names, data })
}

/// Number of positions sampled for the robust bounds (bounded memory).
const BOUNDS_SAMPLES: usize = 200_000;

/// Validates a Gaussian PLY at `path` and returns its summary (incl. SHA-256).
pub fn validate_file(path: &Path, limits: &PlyLimits) -> Result<GaussianSummary, PlyError> {
    let file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    validate_reader(file, len, limits)
}

/// Validates a Gaussian PLY from any reader whose total length is known.
pub fn validate_reader<R: Read>(
    reader: R,
    file_len: u64,
    limits: &PlyLimits,
) -> Result<GaussianSummary, PlyError> {
    if file_len > limits.max_file_bytes {
        return Err(PlyError::TooLarge(file_len, limits.max_file_bytes));
    }
    let mut hasher = Sha256::new();
    let mut r = BufReader::with_capacity(
        1 << 16,
        HashingReader {
            inner: reader,
            hasher: &mut hasher,
        },
    );
    let header = parse_header(&mut r, limits)?;
    let degree = sh_degree(&header.props)?;

    let payload = header
        .count
        .checked_mul(header.stride as u64)
        .ok_or(PlyError::TooManySplats(header.count, limits.max_splats))?;
    let expected = payload
        .checked_add(header.header_bytes as u64)
        .ok_or(PlyError::TooLarge(u64::MAX, limits.max_file_bytes))?;
    if expected > file_len {
        return Err(PlyError::Truncated {
            expected,
            found: file_len,
        });
    }
    if expected < file_len {
        return Err(PlyError::TrailingBytes(file_len - expected));
    }

    let get = |n: &str| {
        header
            .props
            .iter()
            .find(|p| p.name == n)
            .cloned()
            .expect("checked")
    };
    let pos = [get("x"), get("y"), get("z")];
    let rot = [get("rot_0"), get("rot_1"), get("rot_2"), get("rot_3")];
    let opacity = get("opacity");
    // Every interpreted float must be finite.
    let checked: Vec<Property> = header
        .props
        .iter()
        .filter(|p| REQUIRED.contains(&p.name.as_str()) || p.name.starts_with("f_rest_"))
        .cloned()
        .collect();

    let stride = header.stride;
    let per_chunk = ((1usize << 20) / stride.max(1)).max(1);
    let mut chunk = vec![0u8; per_chunk * stride];
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let sample_every = (header.count / BOUNDS_SAMPLES as u64).max(1);
    let mut samples: [Vec<f32>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut index: u64 = 0;
    while index < header.count {
        let n = ((header.count - index) as usize).min(per_chunk);
        let buf = &mut chunk[..n * stride];
        r.read_exact(buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                PlyError::Truncated {
                    expected,
                    found: file_len,
                }
            } else {
                PlyError::Io(e.to_string())
            }
        })?;
        for k in 0..n {
            let rec = &buf[k * stride..(k + 1) * stride];
            let i = index + k as u64;
            for p in &checked {
                if !read_f64(rec, p).is_finite() {
                    return Err(PlyError::NonFinite {
                        index: i,
                        property: p.name.clone(),
                    });
                }
            }
            let q: f64 = rot.iter().map(|p| read_f64(rec, p).powi(2)).sum();
            if q < 1e-16 {
                return Err(PlyError::DegenerateRotation(i));
            }
            let mut xyz = [0f32; 3];
            for a in 0..3 {
                let v = read_f64(rec, &pos[a]) as f32;
                xyz[a] = v;
                min[a] = min[a].min(v);
                max[a] = max[a].max(v);
            }
            // Robust framing ignores nearly transparent floaters.
            let alpha = 1.0 / (1.0 + (-read_f64(rec, &opacity)).exp());
            if i % sample_every == 0 && alpha > 0.05 && samples[0].len() < BOUNDS_SAMPLES * 2 {
                for a in 0..3 {
                    samples[a].push(xyz[a]);
                }
            }
        }
        index += n as u64;
    }
    // Drain to EOF so the hash covers every byte (there should be none left).
    let mut rest = Vec::new();
    r.read_to_end(&mut rest)?;
    if !rest.is_empty() {
        return Err(PlyError::TrailingBytes(rest.len() as u64));
    }
    drop(r);

    let bounds = robust_bounds(&mut samples, min, max);
    let extra_properties = header
        .props
        .iter()
        .filter(|p| !REQUIRED.contains(&p.name.as_str()) && !p.name.starts_with("f_rest_"))
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    let has_normals = ["nx", "ny", "nz"]
        .iter()
        .all(|n| extra_properties.iter().any(|p| p == n));
    Ok(GaussianSummary {
        count: header.count,
        sh_degree: degree,
        stride,
        header_bytes: header.header_bytes,
        file_bytes: file_len,
        sha256: hex::encode(hasher.finalize()),
        has_normals,
        extra_properties,
        bounds,
    })
}

fn robust_bounds(samples: &mut [Vec<f32>; 3], min: [f32; 3], max: [f32; 3]) -> Bounds {
    if samples[0].is_empty() {
        let center = [0, 1, 2].map(|a| (min[a] + max[a]) * 0.5);
        let r = (0..3)
            .map(|a| (max[a] - min[a]).powi(2))
            .sum::<f32>()
            .sqrt()
            * 0.5;
        return Bounds {
            center,
            radius: r.max(1e-3),
            min,
            max,
        };
    }
    let mut lo = [0f32; 3];
    let mut hi = [0f32; 3];
    for a in 0..3 {
        let s = &mut samples[a];
        s.sort_by(|x, y| x.total_cmp(y));
        let n = s.len();
        let pick = |q: f32| s[(((n - 1) as f32) * q).round() as usize];
        lo[a] = pick(0.02);
        hi[a] = pick(0.98);
    }
    let center = [0, 1, 2].map(|a| (lo[a] + hi[a]) * 0.5);
    let radius = (0..3).map(|a| (hi[a] - lo[a]).powi(2)).sum::<f32>().sqrt() * 0.5;
    Bounds {
        center,
        radius: radius.max(1e-3),
        min,
        max,
    }
}

struct HashingReader<'h, R> {
    inner: R,
    hasher: &'h mut Sha256,
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        Ok(n)
    }
}

/// Quick sniff used to give a better error for non-PLY provider outputs
/// (preview videos, GLB meshes, images, HTML error pages).
pub fn sniff_non_ply(prefix: &[u8]) -> Option<&'static str> {
    if prefix.starts_with(b"ply") {
        return None;
    }
    if prefix.len() >= 12 && &prefix[4..8] == b"ftyp" {
        return Some("a preview video (MP4)");
    }
    if prefix.starts_with(b"glTF") {
        return Some("a 3D mesh (GLB)");
    }
    if prefix.starts_with(&[0x89, b'P', b'N', b'G']) || prefix.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("an image");
    }
    if prefix.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("a preview video (WebM)");
    }
    if prefix.starts_with(b"PK\x03\x04") {
        return Some("a ZIP archive");
    }
    let lower = String::from_utf8_lossy(&prefix[..prefix.len().min(64)]).to_ascii_lowercase();
    if lower.trim_start().starts_with('<') {
        return Some("a web page or error message");
    }
    if lower.trim_start().starts_with('{') {
        return Some("a JSON message");
    }
    Some("an unrecognised file")
}
