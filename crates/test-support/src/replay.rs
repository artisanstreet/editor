use artisan_api::EngineId;
use std::path::{Path, PathBuf};

pub struct ReplayingEngine {
    folder: PathBuf,
}

const fn version(engine: EngineId) -> &'static str {
    match engine {
        EngineId::OpenCode => "2.1.0",
        EngineId::Claude | EngineId::Codex => "1.0.0",
    }
}

fn replayer(example: &str) -> PathBuf {
    let test = std::env::current_exe().expect("the test binary has a path");
    let profile = test
        .parent()
        .and_then(Path::parent)
        .expect("test binaries live in <target>/<profile>/deps");
    let replayer = profile
        .join("examples")
        .join(format!("{example}{}", std::env::consts::EXE_SUFFIX));
    assert!(
        replayer.is_file(),
        "the replaying engine {} is missing: build the `{example}` example, which `cargo test` without --lib does",
        replayer.display()
    );
    replayer
}

#[must_use]
pub fn transcript(engine: EngineId, case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../forge-engines/transcripts/replay")
        .join(engine.to_string())
        .join(format!("{case}.jsonl"))
}

impl ReplayingEngine {
    #[must_use]
    pub fn install(forge_data: &Path, example: &str, engine: EngineId, case: &str) -> Self {
        let engine_dir = forge_data.join("engines").join(engine.to_string());
        let version = version(engine);
        let entry = engine_dir.join("versions").join(version).join(
            forge_engines::host_entry(engine)
                .expect("the replayed engine has a build on this platform"),
        );
        let folder = entry
            .parent()
            .expect("the engine entry sits in a folder")
            .to_path_buf();
        std::fs::create_dir_all(&folder).expect("create the replayed engine's folder");
        std::fs::copy(replayer(example), &entry).expect("install the replaying engine");
        let transcript = transcript(engine, case);
        std::fs::copy(&transcript, folder.join("transcript.jsonl")).unwrap_or_else(|error| {
            panic!("copy the transcript {}: {error}", transcript.display())
        });
        std::fs::write(engine_dir.join("current"), version).expect("select the replayed version");
        Self { folder }
    }

    #[must_use]
    pub fn launches(&self) -> usize {
        std::fs::read_to_string(self.folder.join("launches")).map_or(0, |count| {
            count
                .trim()
                .parse()
                .expect("the launch counter holds a number")
        })
    }

    #[must_use]
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.folder.join("replay.log"))
            .unwrap_or_else(|error| format!("no replay log: {error}"))
    }
}

impl std::fmt::Debug for ReplayingEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayingEngine")
            .field("folder", &self.folder)
            .finish()
    }
}
