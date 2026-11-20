use crate::transcript::Transcript;
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

fn reads_transcript(entry: &Path, example: &str) {
    let checked = std::process::Command::new(entry)
        .arg(crate::transcript::CHECK)
        .env_remove("ARTISAN_FAKE")
        .output()
        .unwrap_or_else(|error| panic!("run the replaying engine {}: {error}", entry.display()));
    assert!(
        checked.status.success(),
        "the replaying engine built from the `{example}` example can't read this test's transcript, so it is older than the test: {}\nrebuild it with `cargo build --example {example}` in the test's package; `cargo test` doesn't relink the copy in target/<profile>/examples",
        String::from_utf8_lossy(&checked.stderr).trim()
    );
}

#[must_use]
fn transcript(engine: EngineId, case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../forge-engines/transcripts/replay")
        .join(engine.to_string())
        .join(format!("{case}.jsonl"))
}

impl ReplayingEngine {
    #[must_use]
    pub fn install(forge_data: &Path, example: &str, engine: EngineId, case: &str) -> Self {
        let transcript = transcript(engine, case);
        let text = std::fs::read_to_string(&transcript).unwrap_or_else(|error| {
            panic!("read the transcript {}: {error}", transcript.display())
        });
        Self::install_text(forge_data, example, engine, &text)
    }

    #[must_use]
    pub fn install_transcript(
        forge_data: &Path,
        example: &str,
        engine: EngineId,
        transcript: &Transcript,
    ) -> Self {
        Self::install_text(forge_data, example, engine, &transcript.render())
    }

    fn install_text(forge_data: &Path, example: &str, engine: EngineId, text: &str) -> Self {
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
        crate::executable::install(&replayer(example), &entry);
        std::fs::write(folder.join("transcript.jsonl"), text).expect("write the transcript");
        std::fs::write(engine_dir.join("current"), version).expect("select the replayed version");
        reads_transcript(&entry, example);
        Self { folder }
    }

    #[must_use]
    pub fn launches(&self) -> usize {
        match std::fs::read_to_string(self.folder.join("launches")) {
            Ok(count) => count
                .trim()
                .parse()
                .expect("the launch counter holds a number"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("read the replaying engine's launch counter: {error}"),
        }
    }

    #[must_use]
    pub fn gate(&self, name: &str) -> Gate {
        let path = self.folder.join(format!("{name}.gate"));
        let file = std::fs::File::create(&path)
            .unwrap_or_else(|error| panic!("create the gate {}: {error}", path.display()));
        file.lock()
            .unwrap_or_else(|error| panic!("close the gate {}: {error}", path.display()));
        Gate { file }
    }

    #[must_use]
    pub fn log(&self) -> String {
        std::fs::read_to_string(self.folder.join("replay.log"))
            .unwrap_or_else(|error| format!("no replay log: {error}"))
    }
}

#[derive(Debug)]
pub struct Gate {
    file: std::fs::File,
}

impl Gate {
    pub fn open(self) {
        self.file.unlock().expect("the test opens a gate it holds");
    }
}

impl std::fmt::Debug for ReplayingEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReplayingEngine")
            .field("folder", &self.folder)
            .finish()
    }
}
