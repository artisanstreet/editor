use super::InstallError;
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
pub enum Location {
    Fixed(PathBuf),
    Movable { default: PathBuf, chosen: PathBuf },
}

impl Location {
    #[must_use]
    pub fn path(&self) -> &Path {
        match self {
            Self::Fixed(path) | Self::Movable { chosen: path, .. } => path,
        }
    }

    fn custom(&self) -> Option<PathBuf> {
        match self {
            Self::Movable { default, chosen } if chosen != default => Some(chosen.clone()),
            Self::Fixed(_) | Self::Movable { .. } => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choices {
    location: Location,
    machines: Vec<Machine>,
    selected: Machine,
}

impl Choices {
    #[must_use]
    pub const fn location(&self) -> &Location {
        &self.location
    }

    #[must_use]
    pub fn machines(&self) -> &[Machine] {
        &self.machines
    }

    #[must_use]
    pub const fn selected(&self) -> &Machine {
        &self.selected
    }

    fn select(&mut self, index: usize) {
        if let Some(machine) = self.machines.get(index) {
            self.selected = machine.clone();
        }
    }

    fn relocate(&mut self, folder: PathBuf) {
        if let Location::Movable { chosen, .. } = &mut self.location {
            *chosen = if folder.ends_with(FOLDER) {
                folder
            } else {
                folder.join(FOLDER)
            };
        }
    }

    fn found(&mut self, distros: Vec<String>) {
        self.machines = std::iter::once(Machine::ThisPc)
            .chain(
                distros
                    .into_iter()
                    .filter(|distro| !distro.starts_with("docker-desktop"))
                    .map(Machine::Wsl),
            )
            .collect();
        if !self.machines.contains(&self.selected) {
            self.selected = Machine::ThisPc;
        }
    }

    fn plan(&self) -> InstallPlan {
        InstallPlan::Fresh {
            machine: self.selected.clone(),
            location: self.location.custom(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallPlan {
    Fresh {
        machine: Machine,
        location: Option<PathBuf>,
    },
    Update {
        from: String,
    },
}

impl InstallPlan {
    #[must_use]
    pub fn arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "install".to_owned(),
            "--apply".to_owned(),
            "--editor".to_owned(),
            "here".to_owned(),
            "--forge".to_owned(),
        ];
        match self {
            Self::Update { .. } => arguments.push("keep".to_owned()),
            Self::Fresh { machine, location } => {
                arguments.push(machine.place());
                if let Some(location) = location {
                    arguments.push("--location".to_owned());
                    arguments.push(location.display().to_string());
                }
            }
        }
        arguments
    }
}

#[derive(Debug)]
pub enum State {
    Choosing(Choices),
    Installing {
        plan: InstallPlan,
        line: String,
    },
    Failed {
        plan: InstallPlan,
        error: InstallError,
    },
    Done {
        plan: InstallPlan,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    Selected(usize),
    ChangeLocation,
    Located(PathBuf),
    Started,
}

#[derive(Debug)]
pub enum Event {
    Chose(Choice),
    DistrosFound(Vec<String>),
    Progressed(String),
    Finished(Result<(), InstallError>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    PickFolder,
    Install(Vec<String>),
    Quit,
}

#[derive(Debug)]
pub struct Setup {
    version: String,
    state: State,
}

impl Setup {
    #[must_use]
    pub fn fresh(version: &str, default_location: PathBuf, relocatable: bool) -> Self {
        let location = match relocatable {
            true => Location::Movable {
                chosen: default_location.clone(),
                default: default_location,
            },
            false => Location::Fixed(default_location),
        };
        Self {
            version: version.to_owned(),
            state: State::Choosing(Choices {
                location,
                machines: vec![Machine::ThisPc],
                selected: Machine::ThisPc,
            }),
        }
    }

    #[must_use]
    pub fn update(version: &str, from: String) -> Self {
        Self {
            version: version.to_owned(),
            state: State::Installing {
                plan: InstallPlan::Update { from },
                line: String::new(),
            },
        }
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub const fn state(&self) -> &State {
        &self.state
    }

    #[must_use]
    pub const fn installing(&self) -> bool {
        matches!(self.state, State::Installing { .. })
    }

    #[must_use]
    pub fn started(&self) -> Option<Effect> {
        match &self.state {
            State::Installing { plan, .. } => Some(Effect::Install(plan.arguments())),
            State::Choosing(_) | State::Failed { .. } | State::Done { .. } => None,
        }
    }

    pub fn apply(&mut self, event: Event) -> Option<Effect> {
        match (&mut self.state, event) {
            (State::Choosing(choices), Event::Chose(Choice::Selected(index))) => {
                choices.select(index);
                None
            }
            (State::Choosing(choices), Event::Chose(Choice::ChangeLocation)) => {
                matches!(choices.location, Location::Movable { .. }).then_some(Effect::PickFolder)
            }
            (State::Choosing(choices), Event::Chose(Choice::Located(folder))) => {
                choices.relocate(folder);
                None
            }
            (State::Choosing(choices), Event::DistrosFound(distros)) => {
                choices.found(distros);
                None
            }
            (State::Choosing(choices), Event::Chose(Choice::Started)) => {
                let plan = choices.plan();
                Some(self.install(plan))
            }
            (State::Failed { plan, .. }, Event::Chose(Choice::Started)) => {
                let plan = plan.clone();
                Some(self.install(plan))
            }
            (State::Installing { line, .. }, Event::Progressed(next)) => {
                *line = next;
                None
            }
            (State::Installing { plan, .. }, Event::Finished(Ok(()))) => {
                self.state = State::Done { plan: plan.clone() };
                Some(Effect::Quit)
            }
            (State::Installing { plan, .. }, Event::Finished(Err(error))) => {
                self.state = State::Failed {
                    plan: plan.clone(),
                    error,
                };
                None
            }
            (
                State::Choosing(_)
                | State::Installing { .. }
                | State::Failed { .. }
                | State::Done { .. },
                Event::Chose(_)
                | Event::DistrosFound(_)
                | Event::Progressed(_)
                | Event::Finished(_),
            ) => None,
        }
    }

    fn install(&mut self, plan: InstallPlan) -> Effect {
        let effect = Effect::Install(plan.arguments());
        self.state = State::Installing {
            plan,
            line: String::new(),
        };
        effect
    }
}
