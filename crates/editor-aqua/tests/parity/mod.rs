mod provenance;

use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

const BASELINE_HEAD: &str = "410f02104eb35e730fc385b1b3f6b4c460524150";
const BASELINE_PATCH: &str = "4bbba9baef4de8bb460811198d5fdb1ac1a0ba0b35803638a332d8134d1c62ae";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Provenance {
    implementation: Implementation,
    reference_head: String,
    reference_patch_sha256: String,
    source_before_sha256: String,
    source_after_sha256: String,
    binary_sha256: String,
    fixture_sha256: String,
    script_sha256: String,
    fonts_sha256: String,
    checkpoint: String,
    state_sha256: String,
    platform: String,
    gpu: String,
    backend: String,
    color_encoding: String,
    alpha: String,
    scale_bits: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum Implementation {
    Gpui,
    Aqua,
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate(provenance: &Provenance) -> Result<(), &'static str> {
    if provenance.reference_head != BASELINE_HEAD
        || provenance.reference_patch_sha256 != BASELINE_PATCH
    {
        return Err("reference baseline does not match");
    }
    if ![
        &provenance.source_before_sha256,
        &provenance.source_after_sha256,
        &provenance.binary_sha256,
        &provenance.fixture_sha256,
        &provenance.script_sha256,
        &provenance.fonts_sha256,
        &provenance.state_sha256,
    ]
    .into_iter()
    .all(|value| digest(value))
    {
        return Err("invalid SHA-256");
    }
    if provenance.source_before_sha256 != provenance.source_after_sha256 {
        return Err("source changed during capture");
    }
    if [
        &provenance.checkpoint,
        &provenance.platform,
        &provenance.gpu,
        &provenance.backend,
    ]
    .into_iter()
    .any(|value| value.trim().is_empty())
    {
        return Err("missing capture identity");
    }
    if provenance.color_encoding != "sRGB" || provenance.alpha != "straight" {
        return Err("unqualified pixel representation");
    }
    if provenance.width == 0
        || provenance.height == 0
        || !f32::from_bits(provenance.scale_bits).is_finite()
        || f32::from_bits(provenance.scale_bits) <= 0.0
    {
        return Err("invalid capture geometry");
    }
    Ok(())
}

fn compatible(reference: &Provenance, candidate: &Provenance) -> Result<(), &'static str> {
    validate(reference)?;
    validate(candidate)?;
    if reference.implementation != Implementation::Gpui
        || candidate.implementation != Implementation::Aqua
    {
        return Err("expected GPUI reference and Aqua candidate");
    }
    if reference.fixture_sha256 != candidate.fixture_sha256
        || reference.script_sha256 != candidate.script_sha256
        || reference.fonts_sha256 != candidate.fonts_sha256
        || reference.checkpoint != candidate.checkpoint
        || reference.state_sha256 != candidate.state_sha256
        || reference.platform != candidate.platform
        || reference.gpu != candidate.gpu
        || reference.color_encoding != candidate.color_encoding
        || reference.alpha != candidate.alpha
        || reference.scale_bits != candidate.scale_bits
        || reference.width != candidate.width
        || reference.height != candidate.height
    {
        return Err("capture conditions or state differ");
    }
    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct Difference {
    x: u32,
    y: u32,
    channel: usize,
    reference: u8,
    candidate: u8,
    differing_channels: usize,
    differing_pixels: usize,
}

fn compare(
    reference: &image::RgbaImage,
    candidate: &image::RgbaImage,
) -> Result<Option<Difference>, &'static str> {
    if reference.dimensions() != candidate.dimensions() {
        return Err("image dimensions differ");
    }
    let mut first = None;
    let mut channels = 0;
    let mut pixels = 0;
    for (x, y, left) in reference.enumerate_pixels() {
        let right = candidate.get_pixel(x, y);
        let mut pixel_differs = false;
        for channel in 0..4 {
            if left[channel] != right[channel] {
                channels += 1;
                pixel_differs = true;
                if first.is_none() {
                    first = Some(Difference {
                        x,
                        y,
                        channel,
                        reference: left[channel],
                        candidate: right[channel],
                        differing_channels: 0,
                        differing_pixels: 0,
                    });
                }
            }
        }
        pixels += usize::from(pixel_differs);
    }
    Ok(first.map(|mut difference| {
        difference.differing_channels = channels;
        difference.differing_pixels = pixels;
        difference
    }))
}

fn load(path: &Path, provenance: &Provenance) -> Result<image::RgbaImage, String> {
    validate(provenance).map_err(str::to_owned)?;
    let reader = image::ImageReader::open(path)
        .map_err(|error| error.to_string())?
        .with_guessed_format()
        .map_err(|error| error.to_string())?;
    if reader.format() != Some(ImageFormat::Png) {
        return Err("capture is not PNG".into());
    }
    let image = reader.decode().map_err(|error| error.to_string())?;
    let DynamicImage::ImageRgba8(image) = image else {
        return Err("capture must decode as 8-bit RGBA without conversion".into());
    };
    if image.dimensions() != (provenance.width, provenance.height) {
        return Err("capture dimensions do not match provenance".into());
    }
    Ok(image)
}

#[derive(Default)]
struct RepeatedCapture {
    first: Option<(Provenance, image::RgbaImage)>,
}

impl RepeatedCapture {
    fn observe(&mut self, provenance: Provenance, image: image::RgbaImage) -> Result<bool, String> {
        validate(&provenance).map_err(str::to_owned)?;
        if image.dimensions() != (provenance.width, provenance.height) {
            return Err("capture dimensions do not match provenance".into());
        }
        match &self.first {
            None => {
                self.first = Some((provenance, image));
                Ok(false)
            }
            Some((first_provenance, first_image)) => {
                if first_provenance != &provenance {
                    return Err("repeated capture provenance changed".into());
                }
                match compare(first_image, &image).map_err(str::to_owned)? {
                    None => Ok(true),
                    Some(difference) => Err(format!("repeated capture differs: {difference:?}")),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provenance(implementation: Implementation) -> Provenance {
        Provenance {
            implementation,
            reference_head: BASELINE_HEAD.into(),
            reference_patch_sha256: BASELINE_PATCH.into(),
            source_before_sha256: "a".repeat(64),
            source_after_sha256: "a".repeat(64),
            binary_sha256: "b".repeat(64),
            fixture_sha256: "c".repeat(64),
            script_sha256: "d".repeat(64),
            fonts_sha256: "e".repeat(64),
            checkpoint: "conversation".into(),
            state_sha256: "f".repeat(64),
            platform: "windows".into(),
            gpu: "adapter".into(),
            backend: "dx12".into(),
            color_encoding: "sRGB".into(),
            alpha: "straight".into(),
            scale_bits: 1.25_f32.to_bits(),
            width: 2,
            height: 1,
        }
    }

    #[test]
    fn comparison_rejects_every_channel_difference_and_reports_location() {
        let left = image::RgbaImage::from_pixel(2, 1, image::Rgba([0, 1, 2, 3]));
        let mut right = left.clone();
        assert_eq!(compare(&left, &right), Ok(None));
        right.put_pixel(1, 0, image::Rgba([0, 2, 2, 4]));
        assert_eq!(
            compare(&left, &right),
            Ok(Some(Difference {
                x: 1,
                y: 0,
                channel: 1,
                reference: 1,
                candidate: 2,
                differing_channels: 2,
                differing_pixels: 1
            }))
        );
        assert!(compare(&left, &image::RgbaImage::new(1, 1)).is_err());
    }

    #[test]
    fn provenance_rejects_mutation_and_mismatched_state() {
        let reference = provenance(Implementation::Gpui);
        let mut candidate = provenance(Implementation::Aqua);
        assert_eq!(compatible(&reference, &candidate), Ok(()));
        candidate.state_sha256 = "0".repeat(64);
        assert!(compatible(&reference, &candidate).is_err());
        candidate = provenance(Implementation::Aqua);
        candidate.source_after_sha256 = "0".repeat(64);
        assert!(compatible(&reference, &candidate).is_err());
        candidate = provenance(Implementation::Aqua);
        candidate.scale_bits = f32::NAN.to_bits();
        assert!(compatible(&reference, &candidate).is_err());
    }

    #[test]
    fn png_loader_rejects_conversion_and_wrong_dimensions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("capture.png");
        let provenance = provenance(Implementation::Gpui);
        image::RgbaImage::new(2, 1).save(&path).unwrap();
        assert!(load(&path, &provenance).is_ok());
        image::RgbImage::new(2, 1).save(&path).unwrap();
        assert!(load(&path, &provenance).is_err());
        image::RgbaImage::new(1, 1).save(&path).unwrap();
        assert!(load(&path, &provenance).is_err());
    }

    #[test]
    fn repeat_requires_same_provenance_and_exact_pixels() {
        let provenance = provenance(Implementation::Gpui);
        let image = image::RgbaImage::new(2, 1);
        let mut repeat = RepeatedCapture::default();
        assert_eq!(repeat.observe(provenance.clone(), image.clone()), Ok(false));
        assert_eq!(repeat.observe(provenance.clone(), image.clone()), Ok(true));
        let mut changed = image;
        changed.put_pixel(0, 0, image::Rgba([0, 0, 0, 1]));
        assert!(repeat.observe(provenance.clone(), changed).is_err());
        let mut changed_provenance = provenance;
        changed_provenance.checkpoint = "other".into();
        assert!(
            repeat
                .observe(changed_provenance, image::RgbaImage::new(2, 1))
                .is_err()
        );
    }
}

#[derive(Deserialize)]
struct CaptureFile {
    provenance: Provenance,
    state: provenance::CheckpointState,
    image: std::path::PathBuf,
}

#[derive(Deserialize)]
struct SourceScope {
    root: std::path::PathBuf,
    paths: Vec<std::path::PathBuf>,
}

#[derive(Deserialize)]
struct Qualification {
    reference_source: SourceScope,
    candidate_source: SourceScope,
    reference: [CaptureFile; 2],
    candidate: [CaptureFile; 2],
}

fn repeated(
    captures: [CaptureFile; 2],
    base: &Path,
) -> Result<(Provenance, image::RgbaImage), String> {
    let mut repetition = RepeatedCapture::default();
    for capture in captures {
        if capture.state.name != capture.provenance.checkpoint
            || capture.state.digest()? != capture.provenance.state_sha256
        {
            return Err("checkpoint state does not match provenance".into());
        }
        let image = load(&base.join(&capture.image), &capture.provenance)?;
        repetition.observe(capture.provenance, image)?;
    }
    repetition
        .first
        .ok_or_else(|| "missing repeated capture".into())
}

#[test]
#[ignore = "requires explicit real-application capture manifest"]
fn qualify_captures() {
    let path = std::env::var_os("AQUA_PARITY_MANIFEST").expect("AQUA_PARITY_MANIFEST is required");
    let path = Path::new(&path);
    let bytes = std::fs::read(path).expect("read capture manifest");
    let qualification: Qualification =
        serde_json::from_slice(&bytes).expect("decode capture manifest");
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let (reference, reference_image) =
        repeated(qualification.reference, base).expect("GPUI repeated capture");
    let (candidate, candidate_image) =
        repeated(qualification.candidate, base).expect("Aqua repeated capture");
    for (scope, capture) in [
        (&qualification.reference_source, &reference),
        (&qualification.candidate_source, &candidate),
    ] {
        let snapshot = provenance::SourceSnapshot::capture(&base.join(&scope.root), &scope.paths)
            .expect("capture current source snapshot");
        assert_eq!(
            snapshot.digest().expect("hash source snapshot"),
            capture.source_after_sha256,
            "source differs from captured provenance"
        );
    }
    compatible(&reference, &candidate).expect("compatible capture provenance");
    match compare(&reference_image, &candidate_image).expect("matching image dimensions") {
        None => (),
        Some(difference) => panic!("pixel mismatch: {difference:?}"),
    }
}
