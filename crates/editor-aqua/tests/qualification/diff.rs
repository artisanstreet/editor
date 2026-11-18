use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub enum DiffError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    Decode {
        path: PathBuf,
        source: image::ImageError,
    },
    Encode {
        path: PathBuf,
        source: image::ImageError,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    Malformed {
        path: PathBuf,
        source: MalformedScreen,
    },
    Gate {
        path: PathBuf,
        source: serde_json::Error,
    },
    Manifest {
        path: PathBuf,
        line: usize,
    },
    FileName {
        path: PathBuf,
    },
}

impl fmt::Display for DiffError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "reading {}: {source}", path.display()),
            Self::Decode { path, source } => {
                write!(f, "decoding {}: {source}", path.display())
            }
            Self::Encode { path, source } => {
                write!(f, "encoding {}: {source}", path.display())
            }
            Self::Write { path, source } => write!(f, "writing {}: {source}", path.display()),
            Self::Malformed { path, source } => write!(f, "{}: {source}", path.display()),
            Self::Gate { path, source } => {
                write!(f, "tolerance gate {}: {source}", path.display())
            }
            Self::Manifest { path, line } => {
                write!(
                    f,
                    "screen manifest {} line {line}: no screen name",
                    path.display()
                )
            }
            Self::FileName { path } => write!(f, "{}: no screen name", path.display()),
        }
    }
}

impl std::error::Error for DiffError {}

#[derive(Debug, PartialEq)]
pub enum MalformedScreen {
    Empty {
        width: u32,
        height: u32,
    },
    Length {
        width: u32,
        height: u32,
        bytes: usize,
    },
}

impl fmt::Display for MalformedScreen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { width, height } => write!(f, "empty {width}x{height} screen"),
            Self::Length {
                width,
                height,
                bytes,
            } => write!(
                f,
                "{bytes} bytes do not fill a {width}x{height} RGBA screen"
            ),
        }
    }
}

pub struct Screen {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

impl Screen {
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, MalformedScreen> {
        if width == 0 || height == 0 {
            return Err(MalformedScreen::Empty { width, height });
        }
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        if expected != Some(rgba.len()) {
            return Err(MalformedScreen::Length {
                width,
                height,
                bytes: rgba.len(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    pub fn read(path: &Path) -> Result<Self, DiffError> {
        let image = image::open(path)
            .map_err(|source| DiffError::Decode {
                path: path.to_owned(),
                source,
            })?
            .into_rgba8();
        Self::new(image.width(), image.height(), image.into_raw()).map_err(|source| {
            DiffError::Malformed {
                path: path.to_owned(),
                source,
            }
        })
    }

    fn pixels(&self) -> &[[u8; 4]] {
        self.rgba.as_chunks::<4>().0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(try_from = "RawTolerance")]
pub struct Tolerance {
    channel_threshold: u8,
    max_ratio: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTolerance {
    channel_threshold: u8,
    max_ratio: f64,
}

impl TryFrom<RawTolerance> for Tolerance {
    type Error = String;

    fn try_from(raw: RawTolerance) -> Result<Self, Self::Error> {
        Self::new(raw.channel_threshold, raw.max_ratio)
    }
}

impl Tolerance {
    pub fn new(channel_threshold: u8, max_ratio: f64) -> Result<Self, String> {
        if (0.0..=1.0).contains(&max_ratio) {
            Ok(Self {
                channel_threshold,
                max_ratio,
            })
        } else {
            Err(format!(
                "max_ratio {max_ratio} is not a share between 0 and 1"
            ))
        }
    }
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
        .pixels()
        .iter()
        .zip(candidate.pixels())
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
#[serde(deny_unknown_fields)]
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
    MissingReference,
    MissingCandidate,
    Unlisted,
    Unreadable { cause: String },
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

    fn report(
        &self,
        screen: &str,
        outcome: Outcome,
        difference: Option<Difference>,
    ) -> ScreenReport {
        ScreenReport {
            screen: screen.to_owned(),
            outcome,
            tolerance: self.tolerance(screen),
            difference,
        }
    }

    pub fn judge(
        &self,
        screen: &str,
        reference: &Screen,
        candidate: Option<Result<Screen, DiffError>>,
    ) -> ScreenReport {
        let tolerance = self.tolerance(screen);
        match candidate {
            None => self.report(screen, Outcome::MissingCandidate, None),
            Some(Err(error)) => self.report(
                screen,
                Outcome::Unreadable {
                    cause: error.to_string(),
                },
                None,
            ),
            Some(Ok(candidate)) => {
                match compare(reference, &candidate, tolerance.channel_threshold) {
                    Err(_) => self.report(screen, Outcome::SizeMismatch, None),
                    Ok(difference) if difference.ratio <= tolerance.max_ratio => {
                        self.report(screen, Outcome::Pass, Some(difference))
                    }
                    Ok(difference) => self.report(screen, Outcome::OverTolerance, Some(difference)),
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
            .pixels()
            .iter()
            .zip(candidate.pixels())
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

pub fn gate_from(path: &Path) -> Result<Gate, DiffError> {
    let bytes = std::fs::read(path).map_err(|source| DiffError::Read {
        path: path.to_owned(),
        source,
    })?;
    serde_json::from_slice(&bytes).map_err(|source| DiffError::Gate {
        path: path.to_owned(),
        source,
    })
}

pub fn required_screens(manifest: &Path) -> Result<BTreeSet<String>, DiffError> {
    let text = std::fs::read_to_string(manifest).map_err(|source| DiffError::Read {
        path: manifest.to_owned(),
        source,
    })?;
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty() && !line.starts_with('#'))
        .map(
            |(index, line)| match line.split('|').next().map(str::trim) {
                Some(name) if !name.is_empty() => Ok(name.to_owned()),
                _ => Err(DiffError::Manifest {
                    path: manifest.to_owned(),
                    line: index + 1,
                }),
            },
        )
        .collect()
}

fn write_png(screen: &Screen, path: &Path) -> Result<(), DiffError> {
    image::RgbaImage::from_raw(screen.width, screen.height, screen.rgba.clone())
        .expect("invariant: a Screen's buffer fills its dimensions")
        .save(path)
        .map_err(|source| DiffError::Encode {
            path: path.to_owned(),
            source,
        })
}

fn png_stems(directory: &Path) -> Result<BTreeSet<String>, DiffError> {
    let read = |source| DiffError::Read {
        path: directory.to_owned(),
        source,
    };
    let mut stems = BTreeSet::new();
    for entry in std::fs::read_dir(directory).map_err(read)? {
        let path = entry.map_err(read)?.path();
        if path.extension().is_some_and(|extension| extension == "png") {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| DiffError::FileName { path: path.clone() })?;
            stems.insert(stem.to_owned());
        }
    }
    Ok(stems)
}

fn clear_heat_map(directory: &Path, name: &str) -> Result<(), DiffError> {
    let path = directory.join(format!("{name}.png"));
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DiffError::Write { path, source }),
    }
}

pub fn judge_directory(
    gate: &Gate,
    required: &BTreeSet<String>,
    reference: &Path,
    candidate: &Path,
    heat_maps: Option<&Path>,
) -> Result<Vec<ScreenReport>, DiffError> {
    let baselines = png_stems(reference)?;
    let mut reports = Vec::new();
    for name in required.union(&baselines) {
        if let Some(directory) = heat_maps {
            clear_heat_map(directory, name)?;
        }
        if !required.contains(name) {
            reports.push(gate.report(name, Outcome::Unlisted, None));
            continue;
        }
        if !baselines.contains(name) {
            reports.push(gate.report(name, Outcome::MissingReference, None));
            continue;
        }
        let reference_screen = Screen::read(&reference.join(format!("{name}.png")))?;
        let candidate_path = candidate.join(format!("{name}.png"));
        let candidate_screen = candidate_path
            .exists()
            .then(|| Screen::read(&candidate_path));
        let heat = match (&candidate_screen, heat_maps) {
            (Some(Ok(screen)), Some(_)) => heat_map(
                &reference_screen,
                screen,
                gate.tolerance(name).channel_threshold,
            ),
            _ => None,
        };
        let report = gate.judge(name, &reference_screen, candidate_screen);
        if let (Some(heat), Some(directory), false) =
            (heat, heat_maps, report.outcome == Outcome::Pass)
        {
            std::fs::create_dir_all(directory).map_err(|source| DiffError::Write {
                path: directory.to_owned(),
                source,
            })?;
            write_png(&heat, &directory.join(format!("{name}.png")))?;
        }
        reports.push(report);
    }
    Ok(reports)
}
