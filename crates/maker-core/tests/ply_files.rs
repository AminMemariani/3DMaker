//! Gaussian PLY validation against hand-built files (no network, no GPU).

use maker_core::fixtures::{self, PlyOptions, Splat};
use maker_core::library::{file_sha256, verified_copy};
use maker_core::ply::{PlyError, PlyLimits, validate_reader};

fn check(bytes: &[u8]) -> Result<maker_core::ply::GaussianSummary, PlyError> {
    validate_reader(bytes, bytes.len() as u64, &PlyLimits::default())
}

fn scene() -> Vec<Splat> {
    fixtures::renderer_scene()
}

/// Builds a header + raw payload by hand for malformed cases.
fn raw(header: &str, payload: &[u8]) -> Vec<u8> {
    let mut v = header.as_bytes().to_vec();
    v.extend_from_slice(payload);
    v
}

const STD_PROPS: &str = "property float x\nproperty float y\nproperty float z\nproperty float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\nproperty float opacity\nproperty float scale_0\nproperty float scale_1\nproperty float scale_2\nproperty float rot_0\nproperty float rot_1\nproperty float rot_2\nproperty float rot_3\n";

fn one_record(vals: [f32; 14]) -> Vec<u8> {
    vals.iter().flat_map(|v| v.to_le_bytes()).collect()
}

const GOOD: [f32; 14] = [
    0.0, 0.0, 0.0, 0.1, 0.2, 0.3, 2.0, -4.0, -4.5, -6.0, 1.0, 0.0, 0.0, 0.0,
];

#[test]
fn accepts_degree_zero_with_normals() {
    let s = check(&fixtures::encode(
        &scene(),
        &PlyOptions {
            normals: true,
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(s.count, 6);
    assert_eq!(s.sh_degree, 0);
    assert!(s.has_normals);
    assert_eq!(s.extra_properties, vec!["nx", "ny", "nz"]);
}

#[test]
fn accepts_higher_order_sh() {
    for d in 1..=3u8 {
        let s = check(&fixtures::encode(
            &scene(),
            &PlyOptions {
                sh_degree: d,
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(s.sh_degree, d);
        assert_eq!(s.stride, 4 * (14 + [0, 9, 24, 45][d as usize]));
    }
}

#[test]
fn reads_properties_by_name_not_position() {
    let opts = PlyOptions {
        sh_degree: 1,
        normals: true,
        shuffle: true,
        extra: vec!["producer_id".into()],
        ..Default::default()
    };
    let shuffled = check(&fixtures::encode(&scene(), &opts)).unwrap();
    let plain = check(&fixtures::encode(
        &scene(),
        &PlyOptions {
            sh_degree: 1,
            normals: true,
            extra: vec!["producer_id".into()],
            ..Default::default()
        },
    ))
    .unwrap();
    assert_eq!(
        shuffled.bounds, plain.bounds,
        "same data, different property order"
    );
    assert!(
        shuffled
            .extra_properties
            .contains(&"producer_id".to_string())
    );
}

#[test]
fn negative_log_scales_are_valid() {
    let b = raw(
        &format!("ply\nformat binary_little_endian 1.0\nelement vertex 1\n{STD_PROPS}end_header\n"),
        &one_record(GOOD),
    );
    assert!(check(&b).is_ok());
}

#[test]
fn double_precision_attributes_are_read_by_type() {
    let header = "ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty double x\nproperty double y\nproperty double z\nproperty float f_dc_0\nproperty float f_dc_1\nproperty float f_dc_2\nproperty float opacity\nproperty float scale_0\nproperty float scale_1\nproperty float scale_2\nproperty float rot_0\nproperty float rot_1\nproperty float rot_2\nproperty float rot_3\nproperty uchar flags\nend_header\n";
    let mut p: Vec<u8> = [1.5f64, -2.0, 3.25]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    p.extend(one_record(GOOD)[12..].iter());
    p.push(7);
    let s = check(&raw(header, &p)).unwrap();
    assert_eq!(s.bounds.min, [1.5, -2.0, 3.25]);
    assert_eq!(s.extra_properties, vec!["flags"]);
}

#[test]
fn rejects_truncated_and_trailing_payloads() {
    let full = fixtures::encode(&scene(), &PlyOptions::default());
    let cut = &full[..full.len() - 10];
    assert!(matches!(check(cut), Err(PlyError::Truncated { .. })));
    let mut extra = full.clone();
    extra.extend_from_slice(&[0, 0, 0]);
    assert_eq!(check(&extra), Err(PlyError::TrailingBytes(3)));
}

#[test]
fn rejects_nonfinite_values() {
    let mut v = GOOD;
    v[1] = f32::NAN;
    let b = raw(
        &format!("ply\nformat binary_little_endian 1.0\nelement vertex 1\n{STD_PROPS}end_header\n"),
        &one_record(v),
    );
    assert!(
        matches!(check(&b), Err(PlyError::NonFinite { index: 0, ref property }) if property == "y")
    );
    let mut v = GOOD;
    v[7] = f32::INFINITY;
    let b = raw(
        &format!("ply\nformat binary_little_endian 1.0\nelement vertex 1\n{STD_PROPS}end_header\n"),
        &one_record(v),
    );
    assert!(matches!(check(&b), Err(PlyError::NonFinite { .. })));
}

#[test]
fn rejects_zero_quaternion() {
    let mut v = GOOD;
    v[10] = 0.0;
    let b = raw(
        &format!("ply\nformat binary_little_endian 1.0\nelement vertex 1\n{STD_PROPS}end_header\n"),
        &one_record(v),
    );
    assert_eq!(check(&b), Err(PlyError::DegenerateRotation(0)));
}

#[test]
fn rejects_empty_and_excessive_counts() {
    let b = raw(
        &format!("ply\nformat binary_little_endian 1.0\nelement vertex 0\n{STD_PROPS}end_header\n"),
        &[],
    );
    assert_eq!(check(&b), Err(PlyError::Empty));
    let huge = raw(
        &format!(
            "ply\nformat binary_little_endian 1.0\nelement vertex 4000000000\n{STD_PROPS}end_header\n"
        ),
        &one_record(GOOD),
    );
    assert!(matches!(
        check(&huge),
        Err(PlyError::TooManySplats(4_000_000_000, _))
    ));
    // A count within limits but larger than the payload is a truncated file, not an allocation.
    let lying = raw(
        &format!(
            "ply\nformat binary_little_endian 1.0\nelement vertex 1000000\n{STD_PROPS}end_header\n"
        ),
        &one_record(GOOD),
    );
    assert!(matches!(check(&lying), Err(PlyError::Truncated { .. })));
}

#[test]
fn rejects_mesh_and_plain_point_clouds() {
    let mesh = raw(
        "ply\nformat binary_little_endian 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n",
        &[0u8; 36 + 13],
    );
    assert_eq!(check(&mesh), Err(PlyError::MeshOnly));
    let xyzrgb = raw(
        "ply\nformat binary_little_endian 1.0\nelement vertex 1\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nend_header\n",
        &[0u8; 15],
    );
    let e = check(&xyzrgb).unwrap_err();
    assert!(matches!(e, PlyError::PointCloudOnly(_)), "{e:?}");
    assert!(e.to_string().contains("point cloud"));
}

#[test]
fn rejects_unsupported_encodings() {
    assert!(matches!(
        check(b"ply\nformat ascii 1.0\nelement vertex 1\nend_header\n"),
        Err(PlyError::UnsupportedFormat(_))
    ));
    assert!(matches!(
        check(b"ply\nformat binary_big_endian 1.0\nend_header\n"),
        Err(PlyError::UnsupportedFormat(_))
    ));
    let packed = raw(
        &format!(
            "ply\nformat binary_little_endian 1.0\nelement chunk 1\nproperty float min_x\nelement vertex 1\n{STD_PROPS}end_header\n"
        ),
        &[],
    );
    assert!(matches!(
        check(&packed),
        Err(PlyError::CompressedOrUnknownElement(_))
    ));
    assert_eq!(
        check(b"\x00\x00\x00\x18ftypmp42...."),
        Err(PlyError::NotPly)
    );
    assert_eq!(
        maker_core::ply::sniff_non_ply(b"\x00\x00\x00\x18ftypmp42...."),
        Some("a preview video (MP4)")
    );
    assert_eq!(
        maker_core::ply::sniff_non_ply(b"glTF\x02\x00\x00\x00"),
        Some("a 3D mesh (GLB)")
    );
}

#[test]
fn rejects_integer_gaussian_attributes_and_bad_sh_counts() {
    let header = STD_PROPS.replace("property float opacity", "property uchar opacity");
    let b = raw(
        &format!("ply\nformat binary_little_endian 1.0\nelement vertex 1\n{header}end_header\n"),
        &[0u8; 53],
    );
    assert!(matches!(check(&b), Err(PlyError::WrongType(ref n, _)) if n == "opacity"));
    let sh: String = (0..5)
        .map(|i| format!("property float f_rest_{i}\n"))
        .collect();
    let b = raw(
        &format!(
            "ply\nformat binary_little_endian 1.0\nelement vertex 1\n{STD_PROPS}{sh}end_header\n"
        ),
        &[0u8; 76],
    );
    assert_eq!(check(&b), Err(PlyError::BadShCoefficients(5)));
}

#[test]
fn bounds_ignore_transparent_floaters() {
    let mut s = scene();
    s.push(Splat {
        alpha: 0.001,
        ..Splat::sphere([500.0, 0.0, 0.0], 0.1, [1.0, 1.0, 1.0])
    });
    let b = check(&fixtures::encode(&s, &PlyOptions::default()))
        .unwrap()
        .bounds;
    assert!(b.radius < 2.0, "radius {}", b.radius);
    assert_eq!(b.max[0], 500.0, "raw max still reported");
}

#[test]
fn checksum_matches_file_and_export_is_byte_identical() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.ply");
    let bytes = fixtures::encode(
        &fixtures::teapot(5000, 3),
        &PlyOptions {
            sh_degree: 1,
            normals: true,
            extra: vec!["extra_attr".into()],
            ..Default::default()
        },
    );
    std::fs::write(&src, &bytes).unwrap();
    let summary = maker_core::ply::validate_file(&src, &PlyLimits::default()).unwrap();
    assert_eq!(summary.sha256, file_sha256(&src).unwrap());
    let out = dir.path().join("exported").join("teapot.ply");
    let (n, sha) = verified_copy(&src, &out, Some(&summary.sha256)).unwrap();
    assert_eq!(n, bytes.len() as u64);
    assert_eq!(sha, summary.sha256);
    assert_eq!(
        std::fs::read(&out).unwrap(),
        bytes,
        "export must be byte-for-byte"
    );
    // A source modified after validation is refused, and the destination is untouched.
    std::fs::write(&src, b"tampered").unwrap();
    assert!(verified_copy(&src, &dir.path().join("x.ply"), Some(&summary.sha256)).is_err());
    assert!(!dir.path().join("x.ply").exists());
}
