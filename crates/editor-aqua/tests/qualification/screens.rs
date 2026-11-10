use super::diff::{
    Gate, Outcome, Screen, ScreenReport, Tolerance, compare, gate_from, heat_map, judge_directory,
    passed, table,
};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn solid(width: u32, height: u32, pixel: [u8; 4]) -> Screen {
    Screen {
        width,
        height,
        rgba: pixel.repeat((width * height) as usize),
    }
}

fn gate(default_ratio: f64, threshold: u8) -> Gate {
    Gate {
        default: Tolerance {
            channel_threshold: threshold,
            max_ratio: default_ratio,
        },
        screens: BTreeMap::new(),
    }
}

#[test]
fn identical_screens_have_no_difference() {
    let reference = solid(4, 4, [10, 20, 30, 255]);
    let difference = compare(&reference, &solid(4, 4, [10, 20, 30, 255]), 0).unwrap();
    assert_eq!(difference.differing, 0);
    assert_eq!(difference.ratio, 0.0);
    assert_eq!(difference.bounds, None);
}

#[test]
fn ratio_counts_pixels_and_bounds_the_changed_region() {
    let reference = solid(10, 10, [0, 0, 0, 255]);
    let mut candidate = solid(10, 10, [0, 0, 0, 255]);
    for (x, y) in [(2usize, 3usize), (5, 7)] {
        candidate.rgba[(y * 10 + x) * 4] = 200;
    }
    let difference = compare(&reference, &candidate, 0).unwrap();
    assert_eq!(difference.differing, 2);
    assert!((difference.ratio - 0.02).abs() < 1e-12);
    assert_eq!(difference.max_channel_delta, 200);
    let bounds = difference.bounds.unwrap();
    assert_eq!(
        (bounds.left, bounds.top, bounds.right, bounds.bottom),
        (2, 3, 5, 7)
    );
}

#[test]
fn channel_threshold_forgives_small_deltas_only() {
    let reference = solid(2, 2, [100, 100, 100, 255]);
    let mut candidate = solid(2, 2, [100, 100, 100, 255]);
    candidate.rgba[0] = 103;
    candidate.rgba[4] = 140;
    assert_eq!(compare(&reference, &candidate, 0).unwrap().differing, 2);
    assert_eq!(compare(&reference, &candidate, 3).unwrap().differing, 1);
    assert_eq!(compare(&reference, &candidate, 40).unwrap().differing, 0);
}

#[test]
fn different_sizes_never_compare() {
    let mismatch = compare(&solid(2, 2, [0; 4]), &solid(3, 2, [0; 4]), 0).unwrap_err();
    assert_eq!((mismatch.reference, mismatch.candidate), ((2, 2), (3, 2)));
    assert!(heat_map(&solid(2, 2, [0; 4]), &solid(3, 2, [0; 4]), 0).is_none());
}

#[test]
fn gate_applies_per_screen_limits_and_fails_missing_or_unreadable_candidates() {
    let mut gate = gate(0.0, 0);
    gate.screens.insert(
        "loose".to_owned(),
        Tolerance {
            channel_threshold: 0,
            max_ratio: 0.5,
        },
    );
    let reference = solid(2, 2, [0, 0, 0, 255]);
    let mut candidate = solid(2, 2, [0, 0, 0, 255]);
    candidate.rgba[0] = 9;
    let changed = || {
        Some(Ok(Screen {
            width: 2,
            height: 2,
            rgba: candidate.rgba.clone(),
        }))
    };
    assert_eq!(
        gate.judge("strict", &reference, changed()).outcome,
        Outcome::OverTolerance
    );
    assert_eq!(
        gate.judge("loose", &reference, changed()).outcome,
        Outcome::Pass
    );
    assert_eq!(
        gate.judge("strict", &reference, None).outcome,
        Outcome::MissingCandidate
    );
    assert_eq!(
        gate.judge("strict", &reference, Some(Err("bad".into())))
            .outcome,
        Outcome::Unreadable
    );
    assert_eq!(
        gate.judge("strict", &reference, Some(Ok(solid(3, 3, [0; 4]))))
            .outcome,
        Outcome::SizeMismatch
    );
}

#[test]
fn an_empty_report_never_passes() {
    assert!(!passed(&[]));
    let reports: Vec<ScreenReport> = Vec::new();
    assert!(table(&reports).starts_with("screen"));
}

fn directory(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/qualification")
                .join(fallback)
        },
        PathBuf::from,
    )
}

#[test]
#[ignore = "requires fresh Aqua captures; set AQUA_SCREENS_CANDIDATE"]
fn candidate_screens_stay_within_the_tolerance_gate() {
    let reference = directory("AQUA_SCREENS_REFERENCE", "baselines/aqua");
    let candidate = directory("AQUA_SCREENS_CANDIDATE", "candidate");
    let gate = gate_from(&directory("AQUA_SCREENS_TOLERANCES", "tolerances.json")).unwrap();
    let reports = judge_directory(
        &gate,
        &reference,
        &candidate,
        std::env::var_os("AQUA_SCREENS_HEATMAPS")
            .map(PathBuf::from)
            .as_deref(),
    )
    .unwrap();
    println!("{}", table(&reports));
    if let Some(path) = std::env::var_os("AQUA_SCREENS_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
    }
    assert!(passed(&reports), "screens differ beyond tolerance");
}

#[test]
fn directory_judgement_pairs_by_name_and_fails_what_the_candidate_lacks() {
    let root = tempfile::tempdir().unwrap();
    let (reference, candidate) = (root.path().join("reference"), root.path().join("candidate"));
    std::fs::create_dir_all(&reference).unwrap();
    std::fs::create_dir_all(&candidate).unwrap();
    let save = |directory: &Path, name: &str, screen: &Screen| {
        image::RgbaImage::from_raw(screen.width, screen.height, screen.rgba.clone())
            .unwrap()
            .save(directory.join(format!("{name}.png")))
            .unwrap();
    };
    save(&reference, "same", &solid(3, 3, [5, 5, 5, 255]));
    save(&candidate, "same", &solid(3, 3, [5, 5, 5, 255]));
    save(&reference, "changed", &solid(3, 3, [5, 5, 5, 255]));
    save(&candidate, "changed", &solid(3, 3, [90, 5, 5, 255]));
    save(&reference, "absent", &solid(3, 3, [5, 5, 5, 255]));
    let heat = root.path().join("heat");
    let reports = judge_directory(&gate(0.0, 0), &reference, &candidate, Some(&heat)).unwrap();
    let outcomes: Vec<_> = reports
        .iter()
        .map(|report| (report.screen.as_str(), &report.outcome))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("absent", &Outcome::MissingCandidate),
            ("changed", &Outcome::OverTolerance),
            ("same", &Outcome::Pass)
        ]
    );
    assert!(heat.join("changed.png").exists());
    assert!(!heat.join("same.png").exists());
    assert!(!passed(&reports));
}
