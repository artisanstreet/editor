use super::diff::{
    Gate, MalformedScreen, Outcome, Screen, Tolerance, gate_from, judge_directory, passed,
    required_screens, table,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

fn strict() -> Gate {
    Gate {
        default: Tolerance::new(0, 0.0).expect("an exact gate is valid"),
        screens: BTreeMap::new(),
    }
}

fn save(directory: &Path, name: &str, pixel: [u8; 4]) {
    std::fs::create_dir_all(directory).expect("screen directory");
    image::RgbaImage::from_raw(3, 3, pixel.repeat(9))
        .expect("nine pixels fill a 3x3 image")
        .save(directory.join(format!("{name}.png")))
        .expect("screen saves");
}

fn required(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

#[test]
fn tolerances_outside_zero_to_one_are_rejected_where_the_gate_is_read() {
    let root = tempfile::tempdir().unwrap();
    for ratio in ["1.5", "-0.1"] {
        let path = root.path().join("tolerances.json");
        std::fs::write(
            &path,
            format!(r#"{{"default": {{"channel_threshold": 0, "max_ratio": {ratio}}}}}"#),
        )
        .unwrap();
        assert!(gate_from(&path).is_err(), "max_ratio {ratio} must not load");
    }
}

#[test]
fn buffers_that_do_not_fill_their_dimensions_are_not_screens() {
    assert_eq!(
        Screen::new(2, 2, Vec::new()).err(),
        Some(MalformedScreen::Length {
            width: 2,
            height: 2,
            bytes: 0
        })
    );
    assert_eq!(
        Screen::new(0, 0, Vec::new()).err(),
        Some(MalformedScreen::Empty {
            width: 0,
            height: 0
        })
    );
}

#[test]
fn every_listed_screen_needs_a_baseline_and_every_baseline_needs_a_listing() {
    let root = tempfile::tempdir().unwrap();
    let (reference, candidate) = (root.path().join("reference"), root.path().join("candidate"));
    save(&reference, "same", [5, 5, 5, 255]);
    save(&candidate, "same", [5, 5, 5, 255]);
    save(&reference, "changed", [5, 5, 5, 255]);
    save(&candidate, "changed", [90, 5, 5, 255]);
    save(&reference, "absent", [5, 5, 5, 255]);
    save(&reference, "forgotten", [5, 5, 5, 255]);
    save(&candidate, "forgotten", [5, 5, 5, 255]);
    let reports = judge_directory(
        &strict(),
        &required(&["absent", "changed", "deleted", "same"]),
        &reference,
        &candidate,
        None,
    )
    .unwrap();
    let outcomes: Vec<_> = reports
        .iter()
        .map(|report| (report.screen.as_str(), &report.outcome))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("absent", &Outcome::MissingCandidate),
            ("changed", &Outcome::OverTolerance),
            ("deleted", &Outcome::MissingReference),
            ("forgotten", &Outcome::Unlisted),
            ("same", &Outcome::Pass),
        ]
    );
    assert!(!passed(&reports));
    assert!(table(&reports).contains("MissingReference"));
}

#[test]
fn a_screen_that_passes_again_loses_its_old_heat_map() {
    let root = tempfile::tempdir().unwrap();
    let (reference, candidate, heat) = (
        root.path().join("reference"),
        root.path().join("candidate"),
        root.path().join("heat"),
    );
    save(&reference, "screen", [5, 5, 5, 255]);
    save(&candidate, "screen", [90, 5, 5, 255]);
    let screens = required(&["screen"]);
    judge_directory(&strict(), &screens, &reference, &candidate, Some(&heat)).unwrap();
    assert!(
        heat.join("screen.png").exists(),
        "a failure leaves a heat map"
    );
    save(&candidate, "screen", [5, 5, 5, 255]);
    let reports =
        judge_directory(&strict(), &screens, &reference, &candidate, Some(&heat)).unwrap();
    assert!(passed(&reports));
    assert!(
        !heat.join("screen.png").exists(),
        "the fixed screen's heat map is gone"
    );
}

#[test]
fn an_unreadable_candidate_reports_why() {
    let root = tempfile::tempdir().unwrap();
    let (reference, candidate) = (root.path().join("reference"), root.path().join("candidate"));
    save(&reference, "screen", [5, 5, 5, 255]);
    std::fs::create_dir_all(&candidate).unwrap();
    std::fs::write(candidate.join("screen.png"), b"\x89PNG truncated").unwrap();
    let reports = judge_directory(
        &strict(),
        &required(&["screen"]),
        &reference,
        &candidate,
        None,
    )
    .unwrap();
    let Outcome::Unreadable { cause } = &reports[0].outcome else {
        panic!(
            "expected an unreadable candidate, got {:?}",
            reports[0].outcome
        );
    };
    assert!(cause.contains("screen.png"), "{cause}");
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
#[ignore = "judges fresh WARP captures from capture.sh, as the Windows CI job does; set AQUA_SCREENS_CANDIDATE"]
fn candidate_screens_stay_within_the_tolerance_gate() {
    let reference = directory("AQUA_SCREENS_REFERENCE", "baselines/aqua");
    let candidate = directory("AQUA_SCREENS_CANDIDATE", "candidate");
    let gate = gate_from(&directory("AQUA_SCREENS_TOLERANCES", "tolerances.json")).unwrap();
    let screens = required_screens(&directory("AQUA_SCREENS_MANIFEST", "screens.txt")).unwrap();
    let reports = judge_directory(
        &gate,
        &screens,
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
