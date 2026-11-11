use std::path::{Path, PathBuf};

pub const FOLDER: &str = "Artisan Street";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Editor,
    Install { arguments: Vec<String> },
    Setup,
}

#[must_use]
pub fn entry(carries_installer: bool, runs_installed: bool, arguments: &[String]) -> Entry {
    match (carries_installer, arguments.split_first(), runs_installed) {
        (false, _, _) | (true, _, true) => Entry::Editor,
        (true, Some((first, rest)), false) if first == "--install" => Entry::Install {
            arguments: rest.to_vec(),
        },
        (true, _, false) => Entry::Setup,
    }
}

#[must_use]
pub fn same_program(running: &Path, installed: &Path) -> bool {
    match (running.canonicalize(), installed.canonicalize()) {
        (Ok(running), Ok(installed)) => running == installed,
        _ => running.to_string_lossy().to_lowercase() == installed.to_string_lossy().to_lowercase(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Existing {
    Absent,
    Older(String),
    Current,
}

#[must_use]
pub fn existing(installed: Option<&str>, ours: &str) -> Existing {
    let Some(installed) = installed else {
        return Existing::Absent;
    };
    let newer = match (
        semver::Version::parse(ours),
        semver::Version::parse(installed),
    ) {
        (Ok(ours), Ok(installed)) => ours.cmp_precedence(&installed).is_gt(),
        _ => false,
    };
    if newer {
        Existing::Older(installed.to_owned())
    } else {
        Existing::Current
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Machine {
    ThisPc,
    Wsl(String),
}

impl Machine {
    #[must_use]
    pub fn title(&self) -> String {
        match self {
            Self::ThisPc => "This PC".to_owned(),
            Self::Wsl(distro) => format!("{distro} (WSL)"),
        }
    }

    #[must_use]
    pub const fn detail(&self) -> &'static str {
        match self {
            Self::ThisPc => "Agents work in Windows, on your files here.",
            Self::Wsl(_) => "Agents work inside this Linux distribution.",
        }
    }

    fn place(&self) -> String {
        match self {
            Self::ThisPc => "here".to_owned(),
            Self::Wsl(distro) => format!("wsl:{distro}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Fresh,
    Update { from: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Choosing,
    Installing { line: String },
    Failed { message: String },
    Done,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Selected(usize),
    ChangeLocation,
    Located(PathBuf),
    DistrosFound(Vec<String>),
    Started,
    Progressed(String),
    Finished(Result<(), String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    PickFolder,
    Install(Vec<String>),
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    pub kind: Kind,
    pub version: String,
    pub default_location: PathBuf,
    pub location: PathBuf,
    pub machines: Vec<Machine>,
    pub selected: usize,
    pub phase: Phase,
}

impl Setup {
    #[must_use]
    pub fn fresh(version: &str, default_location: PathBuf) -> Self {
        Self {
            kind: Kind::Fresh,
            version: version.to_owned(),
            location: default_location.clone(),
            default_location,
            machines: vec![Machine::ThisPc],
            selected: 0,
            phase: Phase::Choosing,
        }
    }

    #[must_use]
    pub fn update(version: &str, from: String, location: PathBuf) -> Self {
        Self {
            kind: Kind::Update { from },
            ..Self::fresh(version, location)
        }
    }

    #[must_use]
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "install".to_owned(),
            "--apply".to_owned(),
            "--editor".to_owned(),
            "here".to_owned(),
            "--forge".to_owned(),
        ];
        match &self.kind {
            Kind::Update { .. } => arguments.push("keep".to_owned()),
            Kind::Fresh => {
                arguments.push(self.machines[self.selected].place());
                if self.location != self.default_location {
                    arguments.push("--location".to_owned());
                    arguments.push(self.location.display().to_string());
                }
            }
        }
        arguments
    }

    pub fn apply(&mut self, event: Event) -> Option<Effect> {
        match (&self.phase, event) {
            (Phase::Choosing, Event::Selected(index)) if index < self.machines.len() => {
                self.selected = index;
                None
            }
            (Phase::Choosing, Event::ChangeLocation) => Some(Effect::PickFolder),
            (Phase::Choosing, Event::Located(folder)) => {
                self.location = if folder.ends_with(FOLDER) {
                    folder
                } else {
                    folder.join(FOLDER)
                };
                None
            }
            (Phase::Choosing, Event::DistrosFound(distros)) => {
                let chosen = self.machines[self.selected].clone();
                self.machines = std::iter::once(Machine::ThisPc)
                    .chain(
                        distros
                            .into_iter()
                            .filter(|distro| !distro.starts_with("docker-desktop"))
                            .map(Machine::Wsl),
                    )
                    .collect();
                self.selected = self
                    .machines
                    .iter()
                    .position(|machine| *machine == chosen)
                    .unwrap_or(0);
                None
            }
            (Phase::Choosing | Phase::Failed { .. }, Event::Started) => {
                self.phase = Phase::Installing {
                    line: String::new(),
                };
                Some(Effect::Install(self.arguments()))
            }
            (Phase::Installing { .. }, Event::Progressed(line)) => {
                self.phase = Phase::Installing { line };
                None
            }
            (Phase::Installing { .. }, Event::Finished(Ok(()))) => {
                self.phase = Phase::Done;
                Some(Effect::Quit)
            }
            (Phase::Installing { .. }, Event::Finished(Err(message))) => {
                self.phase = Phase::Failed { message };
                None
            }
            _ => None,
        }
    }
}
