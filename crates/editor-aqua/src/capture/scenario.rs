use crate::core::{Core, EditorFiles, HostId, Screen, SettingsPage, hosts};
use crate::qualification::fixture::{Fixture, ForgeServer};
use anyhow::{Context, Result, bail};
use artisan_api::{EngineId, ProjectId, ThreadId};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Thread(Option<String>),
    NewThread(Option<String>),
    Settings(SettingsPage),
    Marketplace,
    Usage,
}

impl Route {
    pub fn parse(text: &str) -> Result<Self> {
        let (head, tail) = text
            .split_once(':')
            .map_or((text, None), |(head, tail)| (head, Some(tail)));
        match (head, tail) {
            ("thread", thread) => Ok(Self::Thread(thread.map(str::to_owned))),
            ("new-thread", project) => Ok(Self::NewThread(project.map(str::to_owned))),
            ("marketplace", None) => Ok(Self::Marketplace),
            ("usage", None) => Ok(Self::Usage),
            ("settings", Some(page)) => Ok(Self::Settings(settings_page(page)?)),
            _ => bail!("unknown route: {text}"),
        }
    }
}

fn settings_page(name: &str) -> Result<SettingsPage> {
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
        _ => bail!("unknown settings page: {name}"),
    })
}

pub struct Scenario {
    pub host: HostId,
    pub fixture: Fixture,
    forge: Option<ForgeServer>,
    _directory: Option<tempfile::TempDir>,
}

impl Scenario {
    pub fn start(fixture_path: &Path) -> Result<(Self, Core)> {
        let directory = tempfile::tempdir().context("create capture directory")?;
        let editor_dir = directory.path().join("editor");
        let (mut scenario, files) = Self::launch(fixture_path, directory.path(), &editor_dir)?;
        scenario._directory = Some(directory);
        let core = Core::for_tests(files)?;
        Ok((scenario, core))
    }

    pub fn serve(fixture_path: &Path, root: &Path, editor_dir: &Path) -> Result<Self> {
        Self::launch(fixture_path, root, editor_dir).map(|(scenario, _)| scenario)
    }

    fn launch(
        fixture_path: &Path,
        root: &Path,
        editor_dir: &Path,
    ) -> Result<(Self, EditorFiles)> {
        let fixture: Fixture = serde_json::from_slice(
            &std::fs::read(fixture_path)
                .with_context(|| format!("read {}", fixture_path.display()))?,
        )
        .context("decode capture fixture")?;
        fixture.validate()?;
        let forge_dir = root.join("forge");
        std::fs::create_dir_all(&forge_dir)?;
        std::fs::create_dir_all(editor_dir)?;
        let runtime = crate::core::runtime::runtime();
        let server = runtime.block_on(ForgeServer::start(
            forge_dir,
            "127.0.0.1:0".parse().context("loopback address")?,
        ))?;
        runtime.block_on(fixture.seed(&server.store))?;
        let pairing = runtime.block_on(server.pairing_input())?;
        let files = EditorFiles {
            dir: editor_dir.to_path_buf(),
        };
        let identity = files.identity()?;
        let host = HostId(MACHINE_ID.to_owned());
        let mut config = runtime
            .block_on(hosts::pair(
                &pairing.address,
                &pairing.code,
                &pairing.fingerprint,
                &identity,
                "capture",
                "Forge".to_owned(),
                hosts::HostKind::Local,
            ))
            .context("pair the capture Editor with its Forge")?;
        config.id = host.clone();
        files.save_hosts(&hosts::HostsFile {
            hosts: vec![config],
            last: Some(host.clone()),
        })?;
        save_signed_in_account(&files)?;
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

    pub fn screen(&self, route: &Route) -> Result<Screen> {
        Ok(match route {
            Route::Thread(thread) => {
                let thread = match thread {
                    Some(thread) => thread.clone(),
                    None => self
                        .fixture
                        .threads
                        .first()
                        .context("the fixture has no thread")?
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

    pub fn shutdown(&mut self) {
        if let Some(server) = self.forge.take() {
            let _ = crate::core::runtime::runtime().block_on(server.shutdown());
        }
    }
}

pub const CORE_URL: &str = "http://127.0.0.1:1";
pub const ACCOUNT_ID: &str = "acct_capture0000000000000000";
pub const MACHINE_ID: &str = "mch_capture0000000000000000";

fn save_signed_in_account(files: &EditorFiles) -> Result<()> {
    use artisan_account::network::{AccountProfile, Jwk, MachineEntry};
    let profile = AccountProfile {
        account_id: ACCOUNT_ID.to_owned(),
        email: "capture@example.invalid".to_owned(),
        display_name: "Capture".to_owned(),
        totp_enrolled: false,
        machines: vec![MachineEntry {
            id: MACHINE_ID.to_owned(),
            label: "Capture Forge".to_owned(),
            public_key: Jwk {
                kty: "OKP".to_owned(),
                crv: "Ed25519".to_owned(),
                x: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_owned(),
            },
            created_at: "2026-01-01T00:00:00.000Z".to_owned(),
            revoked_at: None,
            last_seen_at: None,
        }],
        latest_config_version: None,
    };
    let account = serde_json::json!({
        "base_url": CORE_URL,
        "session_token": "capture-session",
        "profile": profile,
    });
    std::fs::write(files.dir.join("account.json"), serde_json::to_vec_pretty(&account)?)
        .context("write the capture account")?;
    crate::core::save_onboarding_for_tests(files, CORE_URL, ACCOUNT_ID, MACHINE_ID)
}
