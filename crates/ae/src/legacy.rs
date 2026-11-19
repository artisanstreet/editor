use crate::{install, platform, service, sys, ui};
use artisan_paths::App;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const RECORD: &str = ".ae-migrated-from";
const PID_FILE_SLACK_SECONDS: u64 = 5;
const CODEX_HOME: &str = "engines/codex/home/.codex";
const AE: &[App] = &[App::Ae, App::Forge];
const EDITOR: &[App] = &[App::Editor];
const EVERY_APP: &[App] = &[App::Ae, App::Forge, App::Editor];

#[derive(Debug, thiserror::Error)]
pub(crate) enum MigrateError {
    #[error("find where the earlier install kept its files")]
    Locate(#[source] io::Error),
    #[error(transparent)]
    Installed(#[from] install::InstalledError),
    #[error("check what the earlier install left in {}", path.display())]
    Inspect {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("stop the earlier Forge service")]
    StopService(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("stop the earlier programs")]
    StopPrograms(#[source] platform::StopError),
    #[error("{} names {}, which is not one of the earlier install's folders", record.display(), named)]
    UnknownRecord { record: PathBuf, named: String },
    #[error("Codex's state database {} has no threads.rollout_path column; its schema is not one ae can move", path.display())]
    UnsupportedCodexSchema { path: PathBuf },
    #[error("read Codex's state database {}", path.display())]
    ReadCodexState {
        path: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("point Codex's rollouts in {} from {} to {}", path.display(), from.display(), to.display())]
    Retarget {
        path: PathBuf,
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: Box<rusqlite::Error>,
    },
    #[error("record the move of {} in {}", from.display(), record.display())]
    Record {
        from: PathBuf,
        record: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("move {} to {}", from.display(), to.display())]
    Move {
        from: PathBuf,
        to: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("copy {}", path.display())]
    Copy {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[cfg(unix)]
    #[error("repoint the link {} to {}", link.display(), target.display())]
    Relink {
        link: PathBuf,
        target: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("rewrite the Editor's hosts in {}", path.display())]
    Hosts {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("drop {} from PATH", path.display())]
    PathEntry {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

pub(crate) fn migrate(selected: &[App]) -> Result<(), MigrateError> {
    let mut done = Vec::new();
    for app in App::ALL {
        if selected.contains(&app) || install::installed(app)?.is_present() {
            done.push(app);
        }
    }
    let old = Old::here().map_err(MigrateError::Locate)?;
    let data_moves = DataMove::here(selected, &old).map_err(MigrateError::Locate)?;
    let work = Work {
        old: &old,
        done: &done,
        moves: &data_moves,
    };
    if !work.pending()? {
        return Ok(());
    }
    let progress = ui::Progress::spinner("Moving the earlier install into Artisan Street");
    match work.run() {
        Ok(notes) if notes.is_empty() => progress.done("Removed the earlier install"),
        Ok(notes) => progress.done(format!(
            "Moved {} and removed the earlier install",
            notes.join(" and ")
        )),
        Err(error) => {
            progress.fail(diagnostics::chain(&error));
            return Err(error);
        }
    }
    Ok(())
}

struct Owned {
    path: PathBuf,
    owners: &'static [App],
}

impl Owned {
    const fn new(path: PathBuf, owners: &'static [App]) -> Self {
        Self { path, owners }
    }

    fn removable(&self, done: &[App]) -> bool {
        self.owners.iter().all(|app| done.contains(app))
    }
}

struct Old {
    forge_data: Vec<PathBuf>,
    editor_data: Vec<PathBuf>,
    programs: Vec<Owned>,
    executables: Vec<Owned>,
    parents: Vec<PathBuf>,
    path_entries: Vec<Owned>,
    unit: Option<PathBuf>,
}

impl Old {
    fn here() -> io::Result<Self> {
        if cfg!(windows) {
            let local = PathBuf::from(env("LOCALAPPDATA")?);
            let roaming = PathBuf::from(env("APPDATA")?);
            Self::windows(
                &local,
                &roaming,
                &artisan_paths::home().map_err(io::Error::other)?,
                &App::Editor.dir().map_err(io::Error::other)?,
            )
        } else {
            let config = artisan_paths::xdg_config().map_err(io::Error::other)?;
            let mut old = Self::unix(
                &artisan_paths::xdg_data().map_err(io::Error::other)?,
                &config,
            )?;
            if cfg!(target_os = "linux") {
                old.unit = Some(config.join("systemd/user").join(service::UNIT));
            }
            Ok(old)
        }
    }

    fn windows(local: &Path, roaming: &Path, home: &Path, editor: &Path) -> io::Result<Self> {
        let release = local.join("Artisan");
        let dev = local.join("Artisan Dev");
        let mut executables = vec![
            Owned::new(release.join("bin").join("ae.exe"), AE),
            Owned::new(dev.join("artisan-editor.exe"), EDITOR),
            Owned::new(editor.join("artisan-editor.exe"), EDITOR),
        ];
        executables.extend(versioned(
            &release.join("versions"),
            "ae.exe",
            "artisan-editor.exe",
        )?);
        Ok(Self {
            forge_data: vec![release.join("forge")],
            editor_data: vec![roaming.join("Artisan").join("editor"), dev.join("editor")],
            programs: vec![
                Owned::new(release.join("versions"), EVERY_APP),
                Owned::new(release.join("bin"), AE),
                Owned::new(release.join("current"), EVERY_APP),
                Owned::new(release.join("previous"), EVERY_APP),
                Owned::new(dev.join("artisan-editor.exe"), EDITOR),
                Owned::new(dev.join("launch.cmd"), EDITOR),
                Owned::new(editor.join("artisan-editor.exe"), EDITOR),
                Owned::new(home.join("Desktop").join("Artisan Editor.lnk"), EDITOR),
            ],
            executables,
            parents: vec![release.clone(), dev, roaming.join("Artisan")],
            path_entries: vec![Owned::new(release.join("bin"), AE)],
            unit: None,
        })
    }

    fn unix(data: &Path, config: &Path) -> io::Result<Self> {
        let release = data.join("artisan");
        let dev = data.join("artisan-dev");
        let mut executables = vec![
            Owned::new(release.join("bin/ae"), AE),
            Owned::new(dev.join("bin/ae"), AE),
            Owned::new(dev.join("bin/artisan-editor"), EDITOR),
        ];
        executables.extend(versioned(
            &release.join("versions"),
            "ae",
            "artisan-editor",
        )?);
        Ok(Self {
            forge_data: vec![release.join("forge"), dev.join("forge")],
            editor_data: vec![config.join("Artisan").join("editor"), dev.join("editor")],
            programs: vec![
                Owned::new(release.join("versions"), EVERY_APP),
                Owned::new(release.join("bin"), AE),
                Owned::new(release.join("current"), EVERY_APP),
                Owned::new(release.join("previous"), EVERY_APP),
                Owned::new(dev.join("bin"), EVERY_APP),
                Owned::new(dev.join("wsl-env.sh"), EVERY_APP),
                Owned::new(dev.join("editor.log"), EDITOR),
            ],
            executables,
            parents: vec![release, dev, config.join("Artisan")],
            path_entries: Vec::new(),
            unit: None,
        })
    }
}

fn env(name: &str) -> io::Result<std::ffi::OsString> {
    std::env::var_os(name)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} is not set")))
}

fn versioned(versions: &Path, ae: &str, editor: &str) -> io::Result<Vec<Owned>> {
    let entries = match std::fs::read_dir(versions) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut executables = Vec::new();
    for entry in entries {
        let version = entry?.path();
        executables.push(Owned::new(version.join(ae), AE));
        executables.push(Owned::new(version.join(editor), EDITOR));
    }
    Ok(executables)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Data {
    Forge,
    Editor,
}

impl Data {
    const fn markers(self) -> &'static [&'static str] {
        match self {
            Self::Forge => &["forge.sqlite3"],
            Self::Editor => &["settings.json", "hosts.json", "identity.json"],
        }
    }

    const fn transient(self) -> &'static [&'static str] {
        match self {
            Self::Forge => &["forge.pid", "forge.lock"],
            Self::Editor => &[],
        }
    }

    const fn description(self) -> &'static str {
        match self {
            Self::Forge => "Forge's data",
            Self::Editor => "the Editor's settings",
        }
    }
}

struct DataMove {
    data: Data,
    sources: Vec<PathBuf>,
    destination: PathBuf,
}

enum Staged {
    Nothing,
    Partial,
    Complete { from: PathBuf },
}

enum Landed {
    Empty,
    Unrelated,
    Own,
    Migrated { from: PathBuf },
}

enum Outcome {
    Nothing,
    Kept,
    Moved { from: PathBuf },
    Returned { to: PathBuf },
}

impl DataMove {
    fn here(selected: &[App], old: &Old) -> io::Result<Vec<Self>> {
        let mut moves = Vec::new();
        if selected.contains(&App::Forge) {
            moves.push(Self {
                data: Data::Forge,
                sources: old.forge_data.clone(),
                destination: App::Forge.data().map_err(io::Error::other)?,
            });
        }
        if selected.contains(&App::Editor) {
            moves.push(Self {
                data: Data::Editor,
                sources: old.editor_data.clone(),
                destination: App::Editor.data().map_err(io::Error::other)?,
            });
        }
        Ok(moves)
    }

    fn staging(&self) -> PathBuf {
        let mut name = self
            .destination
            .file_name()
            .expect("invariant: an app's data folder has a name")
            .to_os_string();
        name.push(".migrating");
        self.destination.with_file_name(name)
    }

    fn newest(&self) -> Result<Option<PathBuf>, MigrateError> {
        let mut newest: Option<(SystemTime, &PathBuf)> = None;
        for dir in &self.sources {
            for marker in self.data.markers() {
                let Some(time) = modified(&dir.join(marker))? else {
                    continue;
                };
                if newest.is_none_or(|(best, _)| time > best) {
                    newest = Some((time, dir));
                }
            }
        }
        Ok(newest.map(|(_, dir)| dir.clone()))
    }

    fn staged(&self) -> Result<Staged, MigrateError> {
        let staging = self.staging();
        if !exists(&staging)? {
            return Ok(Staged::Nothing);
        }
        Ok(match self.record(&staging)? {
            Some(from) => Staged::Complete { from },
            None => Staged::Partial,
        })
    }

    fn landed(&self) -> Result<Landed, MigrateError> {
        if let Some(from) = self.record(&self.destination)? {
            return Ok(Landed::Migrated { from });
        }
        for marker in self.data.markers() {
            if exists(&self.destination.join(marker))? {
                return Ok(Landed::Own);
            }
        }
        let empty = match std::fs::read_dir(&self.destination) {
            Ok(mut entries) => entries.next().is_none(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) if error.kind() == io::ErrorKind::NotADirectory => false,
            Err(source) => {
                return Err(MigrateError::Inspect {
                    path: self.destination.clone(),
                    source,
                });
            }
        };
        Ok(if empty {
            Landed::Empty
        } else {
            Landed::Unrelated
        })
    }

    fn record(&self, dir: &Path) -> Result<Option<PathBuf>, MigrateError> {
        let record = dir.join(RECORD);
        let named = match std::fs::read_to_string(&record) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                return Err(MigrateError::Inspect {
                    path: record,
                    source,
                });
            }
        };
        match self
            .sources
            .iter()
            .find(|source| source.to_string_lossy() == named)
        {
            Some(source) => Ok(Some(source.clone())),
            None => Err(MigrateError::UnknownRecord { record, named }),
        }
    }

    fn pending(&self) -> Result<bool, MigrateError> {
        Ok(match (self.landed()?, self.staged()?) {
            (Landed::Migrated { .. }, _) | (_, Staged::Partial | Staged::Complete { .. }) => true,
            (Landed::Own, Staged::Nothing) => false,
            (Landed::Empty | Landed::Unrelated, Staged::Nothing) => self.newest()?.is_some(),
        })
    }

    fn run(&self) -> Result<Outcome, MigrateError> {
        let staged = match self.staged()? {
            Staged::Partial => {
                remove(&self.staging())?;
                Staged::Nothing
            }
            Staged::Nothing => Staged::Nothing,
            Staged::Complete { from } => Staged::Complete { from },
        };
        let landed = self.landed()?;
        let from = match (landed, staged) {
            (Landed::Own, Staged::Complete { from }) => {
                return Ok(Outcome::Returned {
                    to: self.put_back(&from)?,
                });
            }
            (Landed::Own, Staged::Nothing | Staged::Partial) => return Ok(Outcome::Kept),
            (Landed::Migrated { from }, _) => {
                self.finish(&from)?;
                return Ok(Outcome::Moved { from });
            }
            (landed, Staged::Complete { from }) => {
                self.clear_destination(&landed)?;
                from
            }
            (landed, Staged::Nothing | Staged::Partial) => {
                let Some(from) = self.newest()? else {
                    return Ok(Outcome::Nothing);
                };
                self.clear_destination(&landed)?;
                self.stage(&from)?;
                from
            }
        };
        self.prepare(&from)?;
        commit(&self.staging(), &self.destination)?;
        self.finish(&from)?;
        Ok(Outcome::Moved { from })
    }

    fn clear_destination(&self, landed: &Landed) -> Result<(), MigrateError> {
        match landed {
            Landed::Empty => match std::fs::remove_dir(&self.destination) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(MigrateError::Remove {
                    path: self.destination.clone(),
                    source,
                }),
            },
            Landed::Unrelated => {
                let aside = set_aside_name(&self.destination);
                std::fs::rename(&self.destination, &aside).map_err(|source| MigrateError::Move {
                    from: self.destination.clone(),
                    to: aside,
                    source,
                })
            }
            Landed::Own | Landed::Migrated { .. } => {
                unreachable!("invariant: only an empty or unrelated destination is cleared")
            }
        }
    }

    fn stage(&self, from: &Path) -> Result<(), MigrateError> {
        if self.data == Data::Forge {
            check_codex_schema(from)?;
        }
        let staging = self.staging();
        if let Some(parent) = staging.parent() {
            std::fs::create_dir_all(parent).map_err(|source| MigrateError::Move {
                from: from.to_owned(),
                to: parent.to_owned(),
                source,
            })?;
        }
        write_record(from, from)?;
        match std::fs::rename(from, &staging) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                remove(&from.join(RECORD))?;
                copy_tree(from, &staging)?;
                write_record(&staging, from)
            }
            Err(source) => {
                remove(&from.join(RECORD))?;
                Err(MigrateError::Move {
                    from: from.to_owned(),
                    to: staging,
                    source,
                })
            }
        }
    }

    fn prepare(
        &self,
        #[cfg_attr(
            not(unix),
            expect(unused_variables, reason = "links are repointed on Unix only")
        )]
        from: &Path,
    ) -> Result<(), MigrateError> {
        let staging = self.staging();
        if self.data == Data::Forge {
            check_codex_schema(&staging)?;
        }
        for name in self.data.transient() {
            remove(&staging.join(name))?;
        }
        if self.data == Data::Editor {
            point_hosts_at_one_forge(&staging.join("hosts.json"))?;
        }
        #[cfg(unix)]
        retarget_links(&staging, from, &self.destination)?;
        Ok(())
    }

    fn put_back(&self, from: &Path) -> Result<PathBuf, MigrateError> {
        let staging = self.staging();
        let to = if exists(from)? {
            set_aside_name(&self.destination)
        } else {
            from.to_owned()
        };
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|source| MigrateError::Move {
                from: staging.clone(),
                to: parent.to_owned(),
                source,
            })?;
        }
        let to = match std::fs::rename(&staging, &to) {
            Ok(()) => to,
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
                let aside = set_aside_name(&self.destination);
                std::fs::rename(&staging, &aside).map_err(|source| MigrateError::Move {
                    from: staging.clone(),
                    to: aside.clone(),
                    source,
                })?;
                aside
            }
            Err(source) => {
                return Err(MigrateError::Move {
                    from: staging,
                    to,
                    source,
                });
            }
        };
        remove(&to.join(RECORD))?;
        Ok(to)
    }

    fn finish(&self, from: &Path) -> Result<(), MigrateError> {
        if self.data == Data::Forge {
            retarget_codex_rollouts(from, &self.destination)?;
        }
        remove(from)?;
        remove(&self.destination.join(RECORD))
    }
}

fn set_aside_name(destination: &Path) -> PathBuf {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let mut name = destination
        .file_name()
        .expect("invariant: an app's data folder has a name")
        .to_os_string();
    name.push(format!(".before-migration-{seconds}"));
    destination.with_file_name(name)
}

fn write_record(dir: &Path, from: &Path) -> Result<(), MigrateError> {
    let record = dir.join(RECORD);
    std::fs::write(&record, from.to_string_lossy().as_bytes()).map_err(|source| {
        MigrateError::Record {
            from: from.to_owned(),
            record,
            source,
        }
    })
}

fn commit(staging: &Path, destination: &Path) -> Result<(), MigrateError> {
    std::fs::rename(staging, destination).map_err(|source| MigrateError::Move {
        from: staging.to_owned(),
        to: destination.to_owned(),
        source,
    })
}

fn point_hosts_at_one_forge(hosts: &Path) -> Result<(), MigrateError> {
    let text = match std::fs::read_to_string(hosts) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(MigrateError::Hosts {
                path: hosts.to_owned(),
                source,
            });
        }
    };
    let pointed = text
        .replace("127.0.0.1:47901", "127.0.0.1:47900")
        .replace("localhost:47901", "localhost:47900");
    if pointed == text {
        return Ok(());
    }
    std::fs::write(hosts, pointed).map_err(|source| MigrateError::Hosts {
        path: hosts.to_owned(),
        source,
    })
}

struct Work<'a> {
    old: &'a Old,
    done: &'a [App],
    moves: &'a [DataMove],
}

impl Work<'_> {
    fn pending(&self) -> Result<bool, MigrateError> {
        for data_move in self.moves {
            if data_move.pending()? {
                return Ok(true);
            }
        }
        for program in self.removable(&self.old.programs) {
            if exists(&program.path)? {
                return Ok(true);
            }
        }
        Ok(self.done.contains(&App::Forge) && self.old_unit()?.is_some())
    }

    fn removable<'b>(&self, owned: &'b [Owned]) -> impl Iterator<Item = &'b Owned> {
        let done = self.done;
        owned.iter().filter(move |owned| owned.removable(done))
    }

    fn run(&self) -> Result<Vec<String>, MigrateError> {
        if self.done.contains(&App::Forge) {
            self.stop_old_forge()?;
        }
        let executables: Vec<PathBuf> = self
            .removable(&self.old.executables)
            .map(|owned| owned.path.clone())
            .collect();
        platform::close_processes(&executables).map_err(MigrateError::StopPrograms)?;
        let mut moved = Vec::new();
        for data_move in self.moves {
            match data_move.run()? {
                Outcome::Moved { from } => moved.push(format!(
                    "{} from {}",
                    data_move.data.description(),
                    from.display()
                )),
                Outcome::Returned { to } => ui::warning(format!(
                    "{} is already in Artisan Street, so the earlier copy went back to {} instead of replacing it",
                    data_move.data.description(),
                    to.display()
                )),
                Outcome::Nothing | Outcome::Kept => {}
            }
        }
        for program in self.removable(&self.old.programs) {
            remove(&program.path)?;
        }
        for entry in self.removable(&self.old.path_entries) {
            platform::remove_path_entry(&entry.path).map_err(|error| MigrateError::PathEntry {
                path: entry.path.clone(),
                source: error.into(),
            })?;
        }
        for dir in &self.old.parents {
            remove_if_empty(dir)?;
        }
        Ok(moved)
    }

    fn old_unit(&self) -> Result<Option<&Path>, MigrateError> {
        let Some(unit) = &self.old.unit else {
            return Ok(None);
        };
        match std::fs::read_to_string(unit) {
            Ok(text) if text.contains("forge serve") => Ok(Some(unit)),
            Ok(_) => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(MigrateError::Inspect {
                path: unit.clone(),
                source,
            }),
        }
    }

    fn stop_old_forge(&self) -> Result<(), MigrateError> {
        if let Some(unit) = self.old_unit()? {
            sys::run("systemctl", &["--user", "disable", "--now", service::UNIT])
                .map_err(|error| MigrateError::StopService(error.into()))?;
            remove(unit)?;
            sys::run("systemctl", &["--user", "daemon-reload"])
                .map_err(|error| MigrateError::StopService(error.into()))?;
        }
        let mut forges = Vec::new();
        for dir in &self.old.forge_data {
            if let Some(forge) = old_forge(&dir.join("forge.pid"))? {
                forges.push(forge);
            }
        }
        platform::stop(&forges).map_err(MigrateError::StopPrograms)
    }
}

fn old_forge(pid_file: &Path) -> Result<Option<platform::Running>, MigrateError> {
    let inspect = |source| MigrateError::Inspect {
        path: pid_file.to_owned(),
        source,
    };
    let text = match std::fs::read_to_string(pid_file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(inspect(error)),
    };
    let Ok(pid) = text.trim().parse::<u32>() else {
        return Ok(None);
    };
    let written = std::fs::metadata(pid_file)
        .and_then(|metadata| metadata.modified())
        .map_err(inspect)?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    Ok(platform::running(pid).filter(|process| {
        is_forge_program(&process.executable) && process.started <= written + PID_FILE_SLACK_SECONDS
    }))
}

fn is_forge_program(executable: &Path) -> bool {
    executable.file_stem().is_some_and(|stem| {
        let stem = stem.to_string_lossy();
        ["ae", "forge"]
            .iter()
            .any(|name| stem.eq_ignore_ascii_case(name))
    })
}

fn modified(path: &Path) -> Result<Option<SystemTime>, MigrateError> {
    match std::fs::metadata(path).and_then(|metadata| metadata.modified()) {
        Ok(time) => Ok(Some(time)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(MigrateError::Inspect {
            path: path.to_owned(),
            source,
        }),
    }
}

fn exists(path: &Path) -> Result<bool, MigrateError> {
    match path.symlink_metadata() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(MigrateError::Inspect {
            path: path.to_owned(),
            source,
        }),
    }
}

fn remove(path: &Path) -> Result<(), MigrateError> {
    let removed = match path.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(error) => Err(error),
    };
    match removed {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(MigrateError::Remove {
            path: path.to_owned(),
            source,
        }),
    }
}

fn remove_if_empty(dir: &Path) -> Result<(), MigrateError> {
    match std::fs::remove_dir(dir) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(source) => Err(MigrateError::Remove {
            path: dir.to_owned(),
            source,
        }),
    }
}

#[cfg(unix)]
fn retarget_links(dir: &Path, from: &Path, to: &Path) -> Result<(), MigrateError> {
    let inspect = |path: &Path| {
        let path = path.to_owned();
        move |source| MigrateError::Inspect { path, source }
    };
    for entry in std::fs::read_dir(dir).map_err(inspect(dir))? {
        let entry = entry.map_err(inspect(dir))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(inspect(&path))?;
        if kind.is_symlink() {
            let target = std::fs::read_link(&path).map_err(inspect(&path))?;
            if let Ok(rest) = target.strip_prefix(from) {
                relink(&path, &to.join(rest))?;
            }
        } else if kind.is_dir() {
            retarget_links(&path, from, to)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn relink(link: &Path, target: &Path) -> Result<(), MigrateError> {
    let failed = |source| MigrateError::Relink {
        link: link.to_owned(),
        target: target.to_owned(),
        source,
    };
    let mut name = link
        .file_name()
        .expect("invariant: a directory entry has a name")
        .to_os_string();
    name.push(".ae-relink");
    let replacement = link.with_file_name(name);
    remove(&replacement)?;
    std::os::unix::fs::symlink(target, &replacement).map_err(failed)?;
    std::fs::rename(&replacement, link).map_err(failed)
}

fn codex_states(root: &Path) -> Result<Vec<PathBuf>, MigrateError> {
    let codex = root.join(CODEX_HOME);
    let entries = match std::fs::read_dir(&codex) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(MigrateError::Inspect {
                path: codex,
                source,
            });
        }
    };
    let mut states = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|source| MigrateError::Inspect {
            path: codex.clone(),
            source,
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("state_") && name.ends_with(".sqlite") {
            states.push(entry.path());
        }
    }
    Ok(states)
}

fn check_codex_schema(root: &Path) -> Result<(), MigrateError> {
    for path in codex_states(root)? {
        let read = |source| MigrateError::ReadCodexState {
            path: path.clone(),
            source: Box::new(source),
        };
        let db = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(read)?;
        let supported: bool = db
            .query_row(include_str!("legacy/rollout_column.sql"), [], |row| {
                row.get(0)
            })
            .map_err(read)?;
        if !supported {
            return Err(MigrateError::UnsupportedCodexSchema { path });
        }
    }
    Ok(())
}

fn retarget_codex_rollouts(from: &Path, to: &Path) -> Result<(), MigrateError> {
    let separator = std::path::MAIN_SEPARATOR_STR;
    let old = format!("{}{separator}", from.display());
    let new = format!("{}{separator}", to.display());
    for path in codex_states(to)? {
        let failed = |source| MigrateError::Retarget {
            path: path.clone(),
            from: from.to_owned(),
            to: to.to_owned(),
            source: Box::new(source),
        };
        let db = rusqlite::Connection::open(&path).map_err(failed)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(failed)?;
        db.execute(
            include_str!("legacy/retarget_rollouts.sql"),
            rusqlite::params![old, new],
        )
        .map_err(failed)?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), MigrateError> {
    let failed = |path: &Path| {
        let path = path.to_owned();
        move |source| MigrateError::Copy { path, source }
    };
    std::fs::create_dir_all(to).map_err(failed(to))?;
    for entry in std::fs::read_dir(from).map_err(failed(from))? {
        let entry = entry.map_err(failed(from))?;
        let path = entry.path();
        if entry.file_name() == RECORD {
            continue;
        }
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(failed(&path))?;
        if kind.is_symlink() {
            copy_link(&path, &target, kind).map_err(failed(&path))?;
        } else if kind.is_dir() {
            copy_tree(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(failed(&path))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_link(from: &Path, to: &Path, _kind: std::fs::FileType) -> io::Result<()> {
    std::os::unix::fs::symlink(std::fs::read_link(from)?, to)
}

#[cfg(windows)]
fn copy_link(from: &Path, to: &Path, kind: std::fs::FileType) -> io::Result<()> {
    use std::os::windows::fs::FileTypeExt;
    let target = std::fs::read_link(from)?;
    if kind.is_symlink_dir() {
        std::os::windows::fs::symlink_dir(target, to)
    } else {
        std::os::windows::fs::symlink_file(target, to)
    }
}

#[cfg(test)]
#[path = "../tests/legacy/migrate.rs"]
mod tests;
