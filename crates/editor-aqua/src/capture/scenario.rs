use crate::core::{Core, EditorFiles, HostId, OpenCoreError, Screen, SettingsPage};
use crate::qualification::fixture::{
    Fixture, ForgeServer, InvalidFixture, PairingError, SeedError, StartForgeError,
};
use crate::qualification::sign_in::{JoinForgeError, join_forge};
use artisan_api::{EngineId, ProjectId, ThreadId};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum RouteError {
    #[error("unknown route: {route}")]
    Route { route: String },
    #[error("unknown settings page: {page}")]
    SettingsPage { page: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ScenarioError {
    #[error("create a capture directory")]
    Directory(#[source] std::io::Error),
    #[error("read the Editor directory {}", path.display())]
    ListEditor {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("capture requires an empty Editor directory, but {} holds files", path.display())]
    EditorNotEmpty { path: PathBuf },
    #[error("read the capture fixture {}", path.display())]
    ReadFixture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode the capture fixture {}", path.display())]
    DecodeFixture {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("check the capture fixture")]
    InvalidFixture(#[source] InvalidFixture),
    #[error("create {}", path.display())]
    CreateDirectory {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("start the capture Forge")]
    Start(#[source] StartForgeError),
    #[error("seed the capture Forge")]
    Seed(#[source] SeedError),
    #[error("invite the capture Editor to Forge")]
    Invite(#[source] PairingError),
    #[error("start the latency proxy")]
    Proxy(#[source] std::io::Error),
    #[error("sign the capture Editor in to its Forge")]
    Join(#[source] JoinForgeError),
    #[error("open the capture Editor's core")]
    Core(#[source] OpenCoreError),
}

#[derive(Debug, thiserror::Error)]
#[error("the capture fixture has no thread to show")]
pub struct NoThread;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Thread(Option<String>),
    NewThread(Option<String>),
    Settings(SettingsPage),
    Marketplace,
    Usage,
}

impl Route {
    pub fn parse(text: &str) -> Result<Self, RouteError> {
        let (head, tail) = text
            .split_once(':')
            .map_or((text, None), |(head, tail)| (head, Some(tail)));
        match (head, tail) {
            ("thread", thread) => Ok(Self::Thread(thread.map(str::to_owned))),
            ("new-thread", project) => Ok(Self::NewThread(project.map(str::to_owned))),
            ("marketplace", None) => Ok(Self::Marketplace),
            ("usage", None) => Ok(Self::Usage),
            ("settings", Some(page)) => Ok(Self::Settings(settings_page(page)?)),
            _ => Err(RouteError::Route {
                route: text.to_owned(),
            }),
        }
    }
}

fn settings_page(name: &str) -> Result<SettingsPage, RouteError> {
    Ok(match name {
        "general" => SettingsPage::General,
        "threads" => SettingsPage::Threads,
        "hosts" => SettingsPage::Hosts,
        "about" => SettingsPage::About,
        "prompt" => SettingsPage::GlobalPrompt,
        "extensions" => SettingsPage::Extensions,
        "account" => SettingsPage::Account,
        "claude" => SettingsPage::Engine(EngineId::Claude),
        "codex" => SettingsPage::Engine(EngineId::Codex),
        _ => {
            return Err(RouteError::SettingsPage {
                page: name.to_owned(),
            });
        }
    })
}

pub(crate) struct Scenario {
    pub host: HostId,
    pub fixture: Fixture,
    forge: Option<ForgeServer>,
    _directory: Option<tempfile::TempDir>,
}

impl Scenario {
    pub fn start(
        fixture_path: &Path,
        latency: Option<Duration>,
    ) -> Result<(Self, Core), ScenarioError> {
        let directory = tempfile::tempdir().map_err(ScenarioError::Directory)?;
        let editor_dir = directory.path().join("editor");
        let (mut scenario, files) =
            Self::launch(fixture_path, directory.path(), &editor_dir, latency)?;
        scenario._directory = Some(directory);
        let core = Core::for_tests(files).map_err(ScenarioError::Core)?;
        Ok((scenario, core))
    }

    pub(crate) fn serve(
        fixture_path: &Path,
        root: &Path,
        editor_dir: &Path,
    ) -> Result<Self, ScenarioError> {
        if editor_dir.exists()
            && std::fs::read_dir(editor_dir)
                .map_err(|source| ScenarioError::ListEditor {
                    path: editor_dir.to_owned(),
                    source,
                })?
                .next()
                .is_some()
        {
            return Err(ScenarioError::EditorNotEmpty {
                path: editor_dir.to_owned(),
            });
        }
        Self::launch(fixture_path, root, editor_dir, None).map(|(scenario, _)| scenario)
    }

    fn launch(
        fixture_path: &Path,
        root: &Path,
        editor_dir: &Path,
        latency: Option<Duration>,
    ) -> Result<(Self, EditorFiles), ScenarioError> {
        let bytes = std::fs::read(fixture_path).map_err(|source| ScenarioError::ReadFixture {
            path: fixture_path.to_owned(),
            source,
        })?;
        let fixture: Fixture =
            serde_json::from_slice(&bytes).map_err(|source| ScenarioError::DecodeFixture {
                path: fixture_path.to_owned(),
                source,
            })?;
        fixture.validate().map_err(ScenarioError::InvalidFixture)?;
        let forge_dir = root.join("forge");
        for path in [&forge_dir, editor_dir] {
            std::fs::create_dir_all(path).map_err(|source| ScenarioError::CreateDirectory {
                path: path.to_owned(),
                source,
            })?;
        }
        let runtime = crate::core::runtime::runtime();
        let loopback = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0));
        let server = runtime
            .block_on(ForgeServer::start(forge_dir, loopback))
            .map_err(ScenarioError::Start)?;
        runtime
            .block_on(fixture.seed(&server.store))
            .map_err(ScenarioError::Seed)?;
        let mut pairing = runtime
            .block_on(server.pairing_input())
            .map_err(ScenarioError::Invite)?;
        if let Some(delay) = latency {
            pairing.address = runtime
                .block_on(super::latency::proxy(server.address, delay))
                .map_err(ScenarioError::Proxy)?
                .to_string();
        }
        let files = EditorFiles {
            dir: editor_dir.to_path_buf(),
        };
        let host = join_forge(&files, &pairing).map_err(ScenarioError::Join)?;
        Ok((
            Self {
                host,
                fixture,
                forge: Some(server),
                _directory: None,
            },
            files,
        ))
    }

    pub fn screen(&self, route: &Route) -> Result<Screen, NoThread> {
        Ok(match route {
            Route::Thread(thread) => {
                let thread = match thread {
                    Some(thread) => thread.clone(),
                    None => self
                        .fixture
                        .threads
                        .first()
                        .ok_or(NoThread)?
                        .id
                        .as_str()
                        .to_owned(),
                };
                Screen::Thread {
                    host: self.host.clone(),
                    thread: ThreadId::from(thread),
                }
            }
            Route::NewThread(project) => Screen::NewThread {
                host: self.host.clone(),
                project: project.clone().map(ProjectId::from),
            },
            Route::Settings(page) => Screen::Settings(*page),
            Route::Marketplace => Screen::Marketplace,
            Route::Usage => Screen::Usage,
        })
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(server) = self.forge.take() {
            if let Err(error) = crate::core::runtime::runtime().block_on(server.shutdown()) {
                tracing::warn!(
                    error = %::diagnostics::chain(&error),
                    "could not stop the capture Forge"
                );
            }
        }
    }
}
