//! The Forge's extension sync against real engine homes on disk: setup
//! imports the prompt engines share, skills are linked into every installed
//! engine, engine-native skills are discovered and adopted with backups, and
//! global prompt edits outside Artisan are reported, never overwritten,
//! until the user decides.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use artisan_database::{Repository, SqliteConfig, connect};
use artisan_domain::{
    ExtensionEngineStatus, ExtensionOrigin, GlobalPromptEngineStatus, GlobalPromptResolution,
    GlobalPromptSetup,
};
use artisan_migrations::migrate_to_current;

use super::ExtensionSync;

const PDF_SKILL: &str = "---\nname: pdf\ndescription: Reads PDF files.\n---\nUse pdftotext.\n";

struct Forge {
    _root: tempfile::TempDir,
    state: PathBuf,
    sync: Arc<ExtensionSync>,
    published: Arc<AtomicUsize>,
}

impl Forge {
    async fn new(installed: &[&str]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let state = root.path().join("data");
        for engine in installed {
            fs::create_dir_all(state.join("toolchain").join(engine).join("home")).unwrap();
        }
        let database = connect(
            SqliteConfig::in_memory()
                .min_connections(1)
                .max_connections(1)
                .sqlx_logging(false),
        )
        .await
        .unwrap();
        migrate_to_current(&database).await.unwrap();
        let published = Arc::new(AtomicUsize::new(0));
        let sync = ExtensionSync::new(state.clone(), Repository::new(database), {
            let published = Arc::clone(&published);
            move || {
                published.fetch_add(1, Ordering::SeqCst);
            }
        });
        Self {
            _root: root,
            state,
            sync,
            published,
        }
    }

    fn home(&self, engine: &str) -> PathBuf {
        self.state.join("toolchain").join(engine).join("home")
    }

    fn prompt_file(&self, engine: &str) -> PathBuf {
        match engine {
            "claude" => self.home(engine).join(".claude/CLAUDE.md"),
            "codex" => self.home(engine).join(".codex/AGENTS.md"),
            _ => self.home(engine).join(".grok/AGENTS.md"),
        }
    }

    fn skills_dir(&self, engine: &str) -> PathBuf {
        match engine {
            "claude" => self.home(engine).join(".claude/skills"),
            "codex" => self.home(engine).join(".codex/skills"),
            _ => self.home(engine).join(".grok/skills"),
        }
    }

    fn backups(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.state.join("extensions/backups"))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn status(sync: &ExtensionSync, id: &str, engine: &str) -> Option<ExtensionEngineStatus> {
    sync.catalog()
        .extension(id)
        .and_then(|entry| entry.engine(engine))
        .map(|state| state.status)
}

#[tokio::test]
async fn first_party_skills_reach_every_installed_engine_and_follow_the_switch() {
    let forge = Forge::new(&["claude", "codex"]).await;
    forge.sync.reconcile().await.unwrap();
    let id = "artisan.artisan-editor";
    assert_eq!(
        status(&forge.sync, id, "claude"),
        Some(ExtensionEngineStatus::Synced)
    );
    assert_eq!(
        status(&forge.sync, id, "codex"),
        Some(ExtensionEngineStatus::Synced)
    );
    assert_eq!(
        status(&forge.sync, id, "grok"),
        Some(ExtensionEngineStatus::EngineAbsent)
    );
    assert_eq!(
        status(&forge.sync, id, "cursor"),
        Some(ExtensionEngineStatus::Unavailable)
    );
    let link = forge.skills_dir("codex").join("artisan-editor");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        link.join("SKILL.md").is_file(),
        "the link resolves to the skill"
    );
    assert!(forge.published.load(Ordering::SeqCst) >= 1);

    forge.sync.set_enabled(id, false).await.unwrap();
    assert!(
        fs::symlink_metadata(&link).is_err(),
        "disabling removes Artisan's link"
    );
    assert_eq!(
        status(&forge.sync, id, "claude"),
        Some(ExtensionEngineStatus::Off)
    );

    forge.sync.set_enabled(id, true).await.unwrap();
    assert!(link.join("SKILL.md").is_file());

    // A newly installed engine catches up on the next reconcile.
    fs::create_dir_all(forge.home("grok")).unwrap();
    forge.sync.reconcile().await.unwrap();
    assert_eq!(
        status(&forge.sync, id, "grok"),
        Some(ExtensionEngineStatus::Synced)
    );
    assert!(
        forge
            .skills_dir("grok")
            .join("artisan-editor/SKILL.md")
            .is_file()
    );
}

#[tokio::test]
async fn native_skills_are_discovered_once_and_adopted_with_backups() {
    let forge = Forge::new(&["claude", "codex"]).await;
    for engine in ["claude", "codex"] {
        write(&forge.skills_dir(engine).join("pdf/SKILL.md"), PDF_SKILL);
    }
    write(
        &forge.skills_dir("claude").join("own/SKILL.md"),
        "---\nname: own\n---\n",
    );
    forge.sync.reconcile().await.unwrap();
    let catalog = forge.sync.catalog();
    let discovered: Vec<_> = catalog
        .extensions()
        .iter()
        .filter(|entry| entry.origin == ExtensionOrigin::Discovered)
        .collect();
    assert_eq!(
        discovered
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["own", "pdf"],
        "identical copies in two engines are one entry"
    );
    let pdf = discovered.iter().find(|entry| entry.name == "pdf").unwrap();
    assert_eq!(
        pdf.engine("claude").unwrap().status,
        ExtensionEngineStatus::Native
    );
    assert_eq!(
        pdf.engine("codex").unwrap().status,
        ExtensionEngineStatus::Native
    );

    forge.sync.adopt(&pdf.id).await.unwrap();
    assert_eq!(
        status(&forge.sync, "local.pdf", "claude"),
        Some(ExtensionEngineStatus::Synced)
    );
    assert_eq!(
        status(&forge.sync, "local.pdf", "codex"),
        Some(ExtensionEngineStatus::Synced)
    );
    let link = forge.skills_dir("codex").join("pdf");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(link.join("SKILL.md")).unwrap(),
        PDF_SKILL
    );
    let backups = forge.backups();
    assert_eq!(
        backups
            .iter()
            .filter(|name| name.contains("-pdf-skill-"))
            .count(),
        2
    );
    assert!(
        forge.sync.catalog().extension(&pdf.id).is_none(),
        "an adopted skill is no longer discovered"
    );
    assert!(forge.sync.adopt(&pdf.id).await.is_err());
    assert!(
        forge.skills_dir("claude").join("own/SKILL.md").is_file(),
        "skills the user did not adopt are never touched"
    );
}

#[tokio::test]
async fn a_name_taken_by_an_engine_skill_is_a_conflict_left_in_place() {
    let forge = Forge::new(&["claude"]).await;
    write(
        &forge.skills_dir("claude").join("artisan-editor/SKILL.md"),
        "---\nname: artisan-editor\ndescription: Mine.\n---\n",
    );
    forge.sync.reconcile().await.unwrap();
    assert_eq!(
        status(&forge.sync, "artisan.artisan-editor", "claude"),
        Some(ExtensionEngineStatus::Conflict)
    );
    assert!(
        fs::symlink_metadata(forge.skills_dir("claude").join("artisan-editor"))
            .unwrap()
            .is_dir()
    );
}

#[tokio::test]
async fn setup_imports_one_shared_prompt_and_writes_it_everywhere() {
    let forge = Forge::new(&["claude", "codex"]).await;
    write(&forge.prompt_file("claude"), "Use British spelling.\n");
    forge.sync.reconcile().await.unwrap();
    let prompt = forge.sync.global_prompt();
    assert_eq!(prompt.setup(), &GlobalPromptSetup::Ready);
    assert_eq!(prompt.text(), "Use British spelling.");
    assert_eq!(
        fs::read_to_string(forge.prompt_file("codex")).unwrap(),
        "Use British spelling.\n"
    );
    for engine in ["claude", "codex"] {
        assert_eq!(
            prompt.engine(engine).unwrap().status,
            GlobalPromptEngineStatus::Synced
        );
    }
    assert_eq!(
        prompt.engine("cursor").unwrap().status,
        GlobalPromptEngineStatus::Unavailable
    );

    let saved = forge
        .sync
        .save_prompt(prompt.revision(), "Be concise.")
        .await
        .unwrap();
    assert_eq!(saved.text(), "Be concise.");
    for engine in ["claude", "codex"] {
        assert_eq!(
            fs::read_to_string(forge.prompt_file(engine)).unwrap(),
            "Be concise.\n"
        );
    }
    assert!(
        forge
            .sync
            .save_prompt(prompt.revision(), "Stale edit.")
            .await
            .is_err(),
        "a save over a newer revision is refused"
    );
}

#[tokio::test]
async fn differing_prompts_wait_for_the_users_choice() {
    let forge = Forge::new(&["claude", "codex"]).await;
    write(&forge.prompt_file("claude"), "Claude's prompt.");
    write(&forge.prompt_file("codex"), "Codex's prompt.");
    forge.sync.reconcile().await.unwrap();
    let prompt = forge.sync.global_prompt();
    let GlobalPromptSetup::ChoiceRequired(candidates) = prompt.setup() else {
        panic!("two different prompts need a choice");
    };
    assert_eq!(candidates.len(), 2);
    assert_eq!(
        prompt.engine("claude").unwrap().status,
        GlobalPromptEngineStatus::AwaitingChoice
    );
    assert_eq!(
        fs::read_to_string(forge.prompt_file("claude")).unwrap(),
        "Claude's prompt.",
        "nothing is written before the user chooses"
    );
    let codex = candidates
        .iter()
        .find(|candidate| candidate.engine_ids == ["codex"])
        .unwrap();
    forge
        .sync
        .resolve_prompt(&GlobalPromptResolution::ChooseCandidate {
            candidate_id: codex.candidate_id.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(forge.prompt_file("claude")).unwrap(),
        "Codex's prompt.\n"
    );
    assert!(
        forge
            .backups()
            .iter()
            .any(|name| name.contains("claude-home-prompt")),
        "the replaced prompt is backed up"
    );
}

#[tokio::test]
async fn edits_outside_artisan_drift_until_overwritten_or_imported() {
    let forge = Forge::new(&["claude", "codex"]).await;
    write(&forge.prompt_file("claude"), "Original.");
    forge.sync.reconcile().await.unwrap();

    write(&forge.prompt_file("codex"), "Edited in Codex.");
    forge.sync.reconcile().await.unwrap();
    let codex = forge.sync.global_prompt().engine("codex").cloned().unwrap();
    assert_eq!(codex.status, GlobalPromptEngineStatus::Drifted);
    assert_eq!(codex.external_text.as_deref(), Some("Edited in Codex."));
    assert_eq!(
        fs::read_to_string(forge.prompt_file("codex")).unwrap(),
        "Edited in Codex.",
        "drift is never overwritten silently"
    );

    forge
        .sync
        .resolve_prompt(&GlobalPromptResolution::OverwriteEngine {
            engine_id: "codex".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        fs::read_to_string(forge.prompt_file("codex")).unwrap(),
        "Original.\n"
    );

    write(&forge.prompt_file("codex"), "Better wording.");
    forge.sync.reconcile().await.unwrap();
    forge
        .sync
        .resolve_prompt(&GlobalPromptResolution::ImportEngine {
            engine_id: "codex".to_owned(),
        })
        .await
        .unwrap();
    let prompt = forge.sync.global_prompt();
    assert_eq!(prompt.text(), "Better wording.");
    assert_eq!(
        fs::read_to_string(forge.prompt_file("claude")).unwrap(),
        "Better wording.\n"
    );
    assert!(
        prompt
            .engines()
            .iter()
            .filter(|state| state.engine_id == "claude" || state.engine_id == "codex")
            .all(|state| state.status == GlobalPromptEngineStatus::Synced)
    );
}

#[tokio::test]
async fn a_codex_override_file_is_imported_and_retired() {
    let forge = Forge::new(&["codex"]).await;
    write(&forge.prompt_file("codex"), "Base.");
    write(
        &forge.home("codex").join(".codex/AGENTS.override.md"),
        "Override wins.",
    );
    forge.sync.reconcile().await.unwrap();
    assert_eq!(forge.sync.global_prompt().text(), "Override wins.");
    assert!(
        !forge
            .home("codex")
            .join(".codex/AGENTS.override.md")
            .exists()
    );
    assert_eq!(
        fs::read_to_string(forge.prompt_file("codex")).unwrap(),
        "Override wins.\n"
    );
}
