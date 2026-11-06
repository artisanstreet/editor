use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

pub struct Screen {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Screen {
    pub fn read(path: &Path) -> Result<Self, String> {
        let image = image::open(path)
            .map_err(|error| format!("{}: {error}", path.display()))?
            .into_rgba8();
        Ok(Self {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Tolerance {
    pub channel_threshold: u8,
    pub max_ratio: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Bounds {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Difference {
    pub pixels: u64,
    pub differing: u64,
    pub ratio: f64,
    pub mean_channel_error: f64,
    pub max_channel_delta: u8,
    pub bounds: Option<Bounds>,
}

#[derive(Debug, PartialEq)]
pub struct SizeMismatch {
    pub reference: (u32, u32),
    pub candidate: (u32, u32),
}

pub fn compare(
    reference: &Screen,
    candidate: &Screen,
    channel_threshold: u8,
) -> Result<Difference, SizeMismatch> {
    if (reference.width, reference.height) != (candidate.width, candidate.height) {
        return Err(SizeMismatch {
            reference: (reference.width, reference.height),
            candidate: (candidate.width, candidate.height),
        });
    }
    let width = reference.width as usize;
    let mut differing = 0u64;
    let mut total_error = 0u64;
    let mut max_channel_delta = 0u8;
    let mut bounds: Option<Bounds> = None;
    for (index, (left, right)) in reference
        .rgba
        .chunks_exact(4)
        .zip(candidate.rgba.chunks_exact(4))
        .enumerate()
    {
        let deltas = [0, 1, 2, 3].map(|channel| left[channel].abs_diff(right[channel]));
        let worst = deltas.into_iter().max().unwrap_or(0);
        total_error += deltas.into_iter().map(u64::from).sum::<u64>();
        max_channel_delta = max_channel_delta.max(worst);
        if worst > channel_threshold {
            differing += 1;
            let (x, y) = ((index % width) as u32, (index / width) as u32);
            bounds = Some(bounds.map_or(
                Bounds {
                    left: x,
                    top: y,
                    right: x,
                    bottom: y,
                },
                |bounds| Bounds {
                    left: bounds.left.min(x),
                    top: bounds.top.min(y),
                    right: bounds.right.max(x),
                    bottom: bounds.bottom.max(y),
                },
            ));
        }
    }
    let pixels = u64::from(reference.width) * u64::from(reference.height);
    Ok(Difference {
        pixels,
        differing,
        ratio: differing as f64 / pixels as f64,
        mean_channel_error: total_error as f64 / (pixels * 4 * 255) as f64,
        max_channel_delta,
        bounds,
    })
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Gate {
    pub default: Tolerance,
    #[serde(default)]
    pub screens: BTreeMap<String, Tolerance>,
}

#[derive(Debug, PartialEq, Serialize)]
pub enum Outcome {
    Pass,
    OverTolerance,
    SizeMismatch,
    MissingCandidate,
    Unreadable,
}

#[derive(Debug, Serialize)]
pub struct ScreenReport {
    pub screen: String,
    pub outcome: Outcome,
    pub tolerance: Tolerance,
    pub difference: Option<Difference>,
}

impl Gate {
    pub fn tolerance(&self, screen: &str) -> Tolerance {
        self.screens.get(screen).copied().unwrap_or(self.default)
    }

    pub fn judge(
        &self,
        screen: &str,
        reference: &Screen,
        candidate: Option<Result<Screen, String>>,
    ) -> ScreenReport {
        let tolerance = self.tolerance(screen);
        let report = |outcome, difference| ScreenReport {
            screen: screen.to_owned(),
            outcome,
            tolerance,
            difference,
        };
        match candidate {
            None => report(Outcome::MissingCandidate, None),
            Some(Err(_)) => report(Outcome::Unreadable, None),
            Some(Ok(candidate)) => {
                match compare(reference, &candidate, tolerance.channel_threshold) {
                    Err(_) => report(Outcome::SizeMismatch, None),
                    Ok(difference) if difference.ratio <= tolerance.max_ratio => {
                        report(Outcome::Pass, Some(difference))
                    }
                    Ok(difference) => report(Outcome::OverTolerance, Some(difference)),
                }
            }
        }
    }
}

pub fn passed(reports: &[ScreenReport]) -> bool {
    !reports.is_empty() && reports.iter().all(|report| report.outcome == Outcome::Pass)
}

pub fn table(reports: &[ScreenReport]) -> String {
    let mut lines = vec![format!(
        "{:<28} {:<14} {:>10} {:>10} {:>9} {:>9}",
        "screen", "outcome", "ratio", "limit", "mean err", "max delta"
    )];
    for report in reports {
        let (ratio, mean, delta) = report.difference.map_or_else(
            || ("-".to_owned(), "-".to_owned(), "-".to_owned()),
            |difference| {
                (
                    format!("{:.6}", difference.ratio),
                    format!("{:.6}", difference.mean_channel_error),
                    difference.max_channel_delta.to_string(),
                )
            },
        );
        lines.push(format!(
            "{:<28} {:<14} {:>10} {:>10.6} {:>9} {:>9}",
            report.screen,
            format!("{:?}", report.outcome),
            ratio,
            report.tolerance.max_ratio,
            mean,
            delta
        ));
    }
    lines.join("\n")
}

pub fn heat_map(reference: &Screen, candidate: &Screen, channel_threshold: u8) -> Option<Screen> {
    (reference.width == candidate.width && reference.height == candidate.height).then(|| Screen {
        width: reference.width,
        height: reference.height,
        rgba: reference
            .rgba
            .chunks_exact(4)
            .zip(candidate.rgba.chunks_exact(4))
            .flat_map(|(left, right)| {
                let worst = (0..4)
                    .map(|channel| left[channel].abs_diff(right[channel]))
                    .max()
                    .unwrap_or(0);
                if worst > channel_threshold {
                    [255, 0, 80, 255]
                } else {
                    let grey = left[0] / 4 + 150;
                    [grey, grey, grey, 255]
                }
            })
            .collect(),
    })
}

pub fn gate_from(path: &Path) -> Result<Gate, String> {
    serde_json::from_slice(
        &std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
    .map_err(|error| format!("{}: {error}", path.display()))
}

fn write_png(screen: &Screen, path: &Path) -> Result<(), String> {
    image::RgbaImage::from_raw(screen.width, screen.height, screen.rgba.clone())
        .ok_or_else(|| "screen buffer does not match its dimensions".to_owned())?
        .save(path)
        .map_err(|error| format!("{}: {error}", path.display()))
}

pub fn judge_directory(
    gate: &Gate,
    reference: &Path,
    candidate: &Path,
    heat_maps: Option<&Path>,
) -> Result<Vec<ScreenReport>, String> {
    let mut names: Vec<_> = std::fs::read_dir(reference)
        .map_err(|error| format!("{}: {error}", reference.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "png"))
        .collect();
    names.sort();
    let mut reports = Vec::new();
    for path in names {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .ok_or_else(|| format!("{}: no file name", path.display()))?
            .to_owned();
        let reference_screen = Screen::read(&path)?;
        let candidate_path = candidate.join(format!("{name}.png"));
        let candidate_screen = candidate_path
            .exists()
            .then(|| Screen::read(&candidate_path));
        let heat = match (&candidate_screen, heat_maps) {
            (Some(Ok(screen)), Some(_)) => heat_map(
                &reference_screen,
                screen,
                gate.tolerance(&name).channel_threshold,
            ),
            _ => None,
        };
        let report = gate.judge(&name, &reference_screen, candidate_screen);
        if let (Some(heat), Some(directory), false) =
            (heat, heat_maps, report.outcome == Outcome::Pass)
        {
            std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
            write_png(&heat, &directory.join(format!("{name}.png")))?;
        }
        reports.push(report);
    }
    Ok(reports)
}
