//! Test helpers.

use std::path::{Path, PathBuf};

use sapphire_assets::material::AnimeCharacterMaterial;
use sapphire_serialize::cooked::AlignedVec;

use crate::mesh::ImportedMaterial;

/// An imported material slot named `name`, with the default generated material.
pub(crate) fn material(name: &str) -> ImportedMaterial {
    ImportedMaterial { name: name.to_owned(), generated: AnimeCharacterMaterial::default() }
}

/// The file at `path`, in memory aligned as cooked data must be.
pub(crate) fn read_aligned(path: &Path) -> AlignedVec {
    let mut bytes = AlignedVec::new();
    bytes.extend_from_slice(&std::fs::read(path).expect("read a cooked file"));
    bytes
}

/// A uniquely named scratch directory that is removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("sapphire_pipeline-{}-{name}", std::process::id()));
        // A previous run with the same PID may have left it behind.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create scratch directory");
        Self(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether two f32s generated on different platforms agree: within a millionth, relative to
/// their size but at least 1 (generated geometry is in meters, normals, colors and texture
/// coordinates within 0 to 1). Look-dev's generated content holds to it.
///
/// Generated content is built and painted with `sin`, `cos`, `exp`, `atan2` and `powf`, which each
/// platform's libm rounds its own way (glibc's and MSVC's against Apple's), so tests that compare
/// committed generated files with their generator's output allow float rounding and nothing more:
/// they take this, or a tolerance of their own that their content needs, as `agree`.
pub(crate) fn floats_agree(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * a.abs().max(b.abs()).max(1.0)
}

/// Asserts two JSON values are the same but for float rounding (by `agree`): integers, strings
/// and the structure exactly.
pub(crate) fn assert_json_matches(
    path: &str,
    committed: &serde_json::Value,
    generated: &serde_json::Value,
    agree: fn(f64, f64) -> bool,
) {
    use serde_json::Value;
    match (committed, generated) {
        (Value::Number(a), Value::Number(b)) if a.is_f64() || b.is_f64() => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!(agree(a, b), "{path}: committed {a}, generated {b}");
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: array length");
            for (index, (a, b)) in a.iter().zip(b).enumerate() {
                assert_json_matches(&format!("{path}[{index}]"), a, b, agree);
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert!(a.keys().eq(b.keys()), "{path}: keys {:?} against {:?}", a.keys(), b.keys());
            for (key, a) in a {
                assert_json_matches(&format!("{path}.{key}"), a, &b[key], agree);
            }
        }
        (a, b) => assert_eq!(a, b, "{path}"),
    }
}

/// Asserts two GLBs hold the same glTF but for float rounding: the JSON chunk alike but for
/// its floats (the accessors' bounds), and the binary chunk alike byte for byte but in the
/// elements of `F32` accessors, whose floats may differ as `agree` allows. Indices and every
/// other integer match exactly. A failure names how many floats disagree and the furthest off,
/// with its byte and accessor, so one CI run shows the whole spread.
pub(crate) fn assert_glb_matches(
    name: &str,
    committed: &[u8],
    generated: &[u8],
    agree: fn(f64, f64) -> bool,
) {
    let (a, b) = (gltf::Glb::from_slice(committed).unwrap(), gltf::Glb::from_slice(generated).unwrap());
    let json = |glb: &gltf::Glb<'_>| serde_json::from_slice::<serde_json::Value>(&glb.json).unwrap();
    assert_json_matches(name, &json(&a), &json(&b), agree);
    let (a, b) = (a.bin.expect("a binary chunk"), b.bin.expect("a binary chunk"));
    assert_eq!(a.len(), b.len(), "{name}: binary chunk length");
    // Which accessor's floats each byte holds, by the generated file's accessors (the JSON
    // matched).
    let mut float = vec![None; b.len()];
    for accessor in gltf::Gltf::from_slice(generated).unwrap().accessors() {
        if accessor.data_type() != gltf::accessor::DataType::F32 {
            continue;
        }
        let view = accessor.view().expect("generated accessors are dense");
        let stride = view.stride().unwrap_or(accessor.size());
        for element in 0..accessor.count() {
            let start = view.offset() + accessor.offset() + element * stride;
            float[start..start + accessor.size()].fill(Some(accessor.index()));
        }
    }
    // F32 elements are 4-byte aligned in the chunk, so its words line up with their floats.
    let (mut disagree, mut furthest) = (0, None::<(f64, usize, f64, f64)>);
    for (index, (a, b)) in a.chunks(4).zip(b.chunks(4)).enumerate() {
        if a == b {
            continue;
        }
        let at = index * 4;
        assert!(
            float[at..at + a.len()].iter().all(Option::is_some),
            "{name}: byte {at} differs outside the floats"
        );
        let word = |bytes: &[u8]| f64::from(f32::from_le_bytes(bytes.try_into().unwrap()));
        let (a, b) = (word(a), word(b));
        if !agree(a, b) {
            disagree += 1;
            if furthest.is_none_or(|(off, ..)| (a - b).abs() > off) {
                furthest = Some(((a - b).abs(), at, a, b));
            }
        }
    }
    if let Some((off, at, a, b)) = furthest {
        let accessor = float[at].expect("checked above");
        panic!(
            "{name}: {disagree} floats disagree; the furthest, {off:e} off, at byte {at} (accessor \
             {accessor}): committed {a}, generated {b}"
        );
    }
}

/// Asserts two PNGs hold the same image to within one code value a channel: an f32 a few
/// ulps off can round to the next 8-bit value.
pub(crate) fn assert_png_matches(name: &str, committed: &[u8], generated: &[u8]) {
    let decode = |bytes: &[u8]| image::load_from_memory(bytes).unwrap().to_rgba8();
    let (a, b) = (decode(committed), decode(generated));
    assert_eq!(a.dimensions(), b.dimensions(), "{name}: size");
    let offs: Vec<u8> = a.as_raw().iter().zip(b.as_raw()).map(|(&a, &b)| a.abs_diff(b)).collect();
    let off = offs.iter().copied().max().unwrap_or(0);
    let beyond = offs.iter().filter(|&&off| off > 1).count();
    assert!(off <= 1, "{name}: {beyond} channels more than one code value off, the furthest {off}");
}

/// Asserts two text documents (RON) are the same but for float rounding: the text between their
/// numbers exactly, and the numbers as `agree` allows.
pub(crate) fn assert_text_matches(
    name: &str,
    committed: &[u8],
    generated: &[u8],
    agree: fn(f64, f64) -> bool,
) {
    let (a, b) = (std::str::from_utf8(committed).unwrap(), std::str::from_utf8(generated).unwrap());
    let (a, b) = (numbers_and_text(a), numbers_and_text(b));
    for (index, (a, b)) in a.iter().zip(&b).enumerate() {
        match (a.parse::<f64>(), b.parse::<f64>()) {
            (Ok(x), Ok(y)) => assert!(agree(x, y), "{name}: number {index}: committed {a}, generated {b}"),
            _ => assert_eq!(a, b, "{name}: the documents differ"),
        }
    }
    assert_eq!(a.len(), b.len(), "{name}: one document goes on where the other ends");
}

/// `text` cut into numbers and the text between them. A number starts at a digit, or a minus
/// sign before one, that doesn't continue a word (`h_010`, a UUID's `8a3d`).
fn numbers_and_text(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let (mut pieces, mut start, mut index) = (Vec::new(), 0, 0);
    while index < bytes.len() {
        let after_word = index > 0 && (bytes[index - 1].is_ascii_alphanumeric() || bytes[index - 1] == b'_');
        let digit_at = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
        let starts = !after_word && (digit_at(index) || (bytes[index] == b'-' && digit_at(index + 1)));
        if !starts {
            index += 1;
            continue;
        }
        pieces.push(&text[start..index]);
        let number = index;
        index += 1;
        while index < bytes.len() && (bytes[index].is_ascii_digit() || b".eE+-".contains(&bytes[index])) {
            // A sign only right after an exponent's `e`.
            if b"+-".contains(&bytes[index]) && !b"eE".contains(&bytes[index - 1]) {
                break;
            }
            index += 1;
        }
        pieces.push(&text[number..index]);
        start = index;
    }
    pieces.push(&text[start..]);
    pieces
}

#[test]
fn text_matches_up_to_float_rounding() {
    let committed = br#"(color: (0.45, -0.4, 1e-5), id: "4f6b2c1e-8a3d", mask: "masks/h_010.png")"#;
    let rounded =
        br#"(color: (0.45000002, -0.39999998, 0.00001), id: "4f6b2c1e-8a3d", mask: "masks/h_010.png")"#;
    assert_text_matches("rounded", committed, rounded, floats_agree);
    let edits: [&[u8]; 4] = [
        br#"(color: (0.46, -0.4, 1e-5), id: "4f6b2c1e-8a3d", mask: "masks/h_010.png")"#,
        br#"(color: (0.45, -0.4, 1e-5), id: "4f6b2c1e-8a3e", mask: "masks/h_010.png")"#,
        br#"(color: (0.45, -0.4, 1e-5), id: "4f6b2c1e-8a3d", mask: "masks/h_011.png")"#,
        br#"(color: (0.45, -0.4, 1e-5), id: "4f6b2c1e-8a3d", mask: "masks/h_010.png", extra: 1)"#,
    ];
    for edited in edits {
        let caught =
            std::panic::catch_unwind(|| assert_text_matches("edited", committed, edited, floats_agree));
        assert!(caught.is_err(), "{}", String::from_utf8_lossy(edited));
    }
}
