use super::fixture::{
    FixtureError, ForgeServer, ImmutableFixture, MarkerError, ProduceSnapshotError,
    RestoreSnapshotError, SeedSnapshot, StartForgeError,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    fixture: PathBuf,
    forge_dir: PathBuf,
    snapshot_dir: PathBuf,
    output_dir: PathBuf,
    source_editor_dir: PathBuf,
    candidate_editor_dir: PathBuf,
    listen: SocketAddr,
    host_id: String,
    thread_id: String,
    width: f32,
    height: f32,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedRecord {
    seed: SeedSnapshot,
    snapshot_dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum MarkedError {
    #[error("check {}", path.display())]
    Marker {
        path: PathBuf,
        #[source]
        source: MarkerError,
    },
    #[error("{} is not canonical or passes through a symlink", path.display())]
    NotCanonical { path: PathBuf },
}

#[derive(Debug, thiserror::Error)]
pub enum FreshError {
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is not fresh: it holds {}", directory.display(), entry.display())]
    NotFresh { directory: PathBuf, entry: PathBuf },
}

#[derive(Debug, thiserror::Error)]
#[error("write {}", path.display())]
pub struct WriteNewError {
    path: PathBuf,
    #[source]
    source: std::io::Error,
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareError {
    #[error("read the preparation request {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode the preparation request {}", path.display())]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("resolve the fixture {}", path.display())]
    ResolveFixture {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the fixture {} is not a canonical regular file", path.display())]
    FixturePath { path: PathBuf },
    #[error("load the fixture")]
    Fixture(#[source] FixtureError),
    #[error("check a qualification directory")]
    Marked(#[source] MarkedError),
    #[error("the fixture {} lies inside {}", fixture.display(), directory.display())]
    FixtureInside {
        fixture: PathBuf,
        directory: PathBuf,
    },
    #[error("{} and {} overlap", left.display(), right.display())]
    Overlap { left: PathBuf, right: PathBuf },
    #[error("check that a qualification directory is fresh")]
    Fresh(#[source] FreshError),
    #[error("the source and candidate Editors start with different settings")]
    SettingsDiffer,
    #[error("qualification listens on {address}, not a fixed loopback port")]
    Listen { address: SocketAddr },
    #[error("host {host:?} is empty or thread {thread} is not in the fixture")]
    Ids { host: String, thread: String },
    #[error("the qualification window {width}x{height} is too small or not finite")]
    Dimensions { width: f32, height: f32 },
    #[error("start Forge")]
    Start(#[source] StartForgeError),
    #[error("produce the seed snapshot")]
    Produce(#[source] ProduceSnapshotError),
    #[error("check that the fixture is unchanged")]
    FixtureChanged(#[source] FixtureError),
    #[error("encode {what}")]
    Encode {
        what: &'static str,
        #[source]
        source: serde_json::Error,
    },
    #[error("read the seeded database {}", path.display())]
    ReadDatabase {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write a preparation output")]
    Write(#[source] WriteNewError),
}

#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("the seed record {} is not a seed.json file in a qualification directory", path.display())]
    RecordPath { path: PathBuf },
    #[error("check the seed record's directory")]
    Marked(#[source] MarkedError),
    #[error("read the seed record {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("the seed record {} does not hash to the expected SHA-256", path.display())]
    Mismatch { path: PathBuf },
    #[error("decode the seed record {}", path.display())]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("restore the seed snapshot")]
    Restore(#[source] RestoreSnapshotError),
}

fn marked(path: &Path) -> Result<PathBuf, MarkedError> {
    let canonical =
        super::fixture::marked_directory(path).map_err(|source| MarkedError::Marker {
            path: path.to_owned(),
            source,
        })?;
    if canonical != path {
        return Err(MarkedError::NotCanonical {
            path: path.to_owned(),
        });
    }
    Ok(canonical)
}

fn fresh(path: &Path, settings: bool) -> Result<Option<String>, FreshError> {
    let read = |path: &Path| {
        let path = path.to_owned();
        move |source| FreshError::Read { path, source }
    };
    for entry in std::fs::read_dir(path).map_err(read(path))? {
        let entry = entry.map_err(read(path))?;
        let name = entry.file_name();
        let expected = name == ".artisan-qualification" || (settings && name == "settings.json");
        if !expected || !entry.file_type().map_err(read(&entry.path()))?.is_file() {
            return Err(FreshError::NotFresh {
                directory: path.to_owned(),
                entry: entry.path(),
            });
        }
    }
    let settings_path = path.join("settings.json");
    if settings_path.exists() {
        Ok(Some(hash(
            &std::fs::read(&settings_path).map_err(read(&settings_path))?,
        )))
    } else {
        Ok(None)
    }
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), WriteNewError> {
    let fail = |source| WriteNewError {
        path: path.to_owned(),
        source,
    };
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(fail)?;
    file.write_all(bytes).map_err(fail)?;
    file.sync_all().map_err(fail)
}

fn encode(what: &'static str, value: &impl Serialize) -> Result<Vec<u8>, PrepareError> {
    serde_json::to_vec(value).map_err(|source| PrepareError::Encode { what, source })
}

#[expect(
    clippy::print_stdout,
    reason = "the qualification CLI reports the prepared seed to its caller on stdout"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one preparation run from seeding the store to writing the digest"
)]
pub fn prepare(path: &Path) -> Result<(), PrepareError> {
    let bytes = std::fs::read(path).map_err(|source| PrepareError::Read {
        path: path.to_owned(),
        source,
    })?;
    let request: Preparation =
        serde_json::from_slice(&bytes).map_err(|source| PrepareError::Decode {
            path: path.to_owned(),
            source,
        })?;
    let resolve = |source| PrepareError::ResolveFixture {
        path: request.fixture.clone(),
        source,
    };
    let fixture_path = super::fixture::canonical_path(&request.fixture).map_err(resolve)?;
    if request.fixture != fixture_path
        || !std::fs::symlink_metadata(&fixture_path)
            .map_err(resolve)?
            .is_file()
    {
        return Err(PrepareError::FixturePath {
            path: request.fixture,
        });
    }
    let fixture = ImmutableFixture::read(&fixture_path).map_err(PrepareError::Fixture)?;
    let dirs = [
        &request.forge_dir,
        &request.snapshot_dir,
        &request.output_dir,
        &request.source_editor_dir,
        &request.candidate_editor_dir,
    ]
    .into_iter()
    .map(|path| marked(path))
    .collect::<Result<Vec<_>, _>>()
    .map_err(PrepareError::Marked)?;
    for (index, left) in dirs.iter().enumerate() {
        if fixture_path.starts_with(left) {
            return Err(PrepareError::FixtureInside {
                fixture: fixture_path,
                directory: left.clone(),
            });
        }
        if let Some(right) = dirs[index + 1..]
            .iter()
            .find(|right| left.starts_with(right) || right.starts_with(left))
        {
            return Err(PrepareError::Overlap {
                left: left.clone(),
                right: right.clone(),
            });
        }
    }
    for dir in &dirs[..3] {
        fresh(dir, false).map_err(PrepareError::Fresh)?;
    }
    let source_settings = fresh(&dirs[3], true).map_err(PrepareError::Fresh)?;
    let candidate_settings = fresh(&dirs[4], true).map_err(PrepareError::Fresh)?;
    if source_settings != candidate_settings {
        return Err(PrepareError::SettingsDiffer);
    }
    if !request.listen.ip().is_loopback() || request.listen.port() == 0 {
        return Err(PrepareError::Listen {
            address: request.listen,
        });
    }
    if request.host_id.trim().is_empty()
        || !fixture
            .fixture()
            .threads
            .iter()
            .any(|thread| thread.id.as_str() == request.thread_id)
    {
        return Err(PrepareError::Ids {
            host: request.host_id,
            thread: request.thread_id,
        });
    }
    if !(request.width.is_finite()
        && request.height.is_finite()
        && request.width >= 480.0
        && request.height >= 420.0)
    {
        return Err(PrepareError::Dimensions {
            width: request.width,
            height: request.height,
        });
    }
    let runtime = crate::core::runtime::runtime();
    let server = runtime
        .block_on(ForgeServer::start(dirs[0].clone(), request.listen))
        .map_err(PrepareError::Start)?;
    let seed = runtime
        .block_on(SeedSnapshot::produce(
            server,
            fixture.fixture(),
            &dirs[0],
            &dirs[1],
        ))
        .map_err(PrepareError::Produce)?;
    fixture
        .verify_unchanged(&fixture_path)
        .map_err(PrepareError::FixtureChanged)?;
    let store = encode("the store snapshot", &seed.store)?;
    let store_path = dirs[2].join("store.json");
    let seed_path = dirs[2].join("seed.json");
    let database = dirs[0].join("forge.sqlite3");
    let database_hash =
        hash(
            &std::fs::read(&database).map_err(|source| PrepareError::ReadDatabase {
                path: database,
                source,
            })?,
        );
    let seed_bytes = encode(
        "the seed record",
        &SeedRecord {
            seed,
            snapshot_dir: dirs[1].clone(),
        },
    )?;
    write_new(&store_path, &store).map_err(PrepareError::Write)?;
    write_new(&seed_path, &seed_bytes).map_err(PrepareError::Write)?;
    for (name, editor_dir, settings) in [
        ("source", &dirs[3], source_settings),
        ("candidate", &dirs[4], candidate_settings),
    ] {
        let value = serde_json::json!({
            "fixture": fixture_path, "fixture_sha256": fixture.sha256(),
            "snapshot": store_path, "snapshot_sha256": hash(&store),
            "forge_dir": dirs[0], "forge_logical_path": dirs[0], "seed_database_sha256": database_hash,
            "editor_dir": editor_dir, "editor_settings_sha256": settings, "listen": request.listen,
            "host_id": request.host_id, "thread_id": request.thread_id, "width": request.width, "height": request.height
        });
        write_new(
            &dirs[2].join(format!("{name}-request.json")),
            &encode("a bootstrap request", &value)?,
        )
        .map_err(PrepareError::Write)?;
    }
    println!(
        "{}",
        serde_json::json!({"type":"qualification_seed", "seed":seed_path, "seed_sha256":hash(&seed_bytes), "source_request":dirs[2].join("source-request.json"), "candidate_request":dirs[2].join("candidate-request.json")})
    );
    Ok(())
}

#[expect(
    clippy::print_stdout,
    reason = "the qualification CLI reports the restored seed to its caller on stdout"
)]
pub(crate) fn restore(path: &Path, expected_sha256: &str) -> Result<(), RestoreError> {
    let record_path = || RestoreError::RecordPath {
        path: path.to_owned(),
    };
    let parent = path.parent().ok_or_else(record_path)?;
    marked(parent).map_err(RestoreError::Marked)?;
    let read = |source| RestoreError::Read {
        path: path.to_owned(),
        source,
    };
    if path.file_name().is_none_or(|name| name != "seed.json")
        || !std::fs::symlink_metadata(path).map_err(read)?.is_file()
    {
        return Err(record_path());
    }
    let bytes = std::fs::read(path).map_err(read)?;
    if hash(&bytes) != expected_sha256 {
        return Err(RestoreError::Mismatch {
            path: path.to_owned(),
        });
    }
    let record: SeedRecord =
        serde_json::from_slice(&bytes).map_err(|source| RestoreError::Decode {
            path: path.to_owned(),
            source,
        })?;
    record
        .seed
        .restore_after_all_forge_handles_closed(&record.snapshot_dir, &record.seed.logical_path)
        .map_err(RestoreError::Restore)?;
    println!(
        "{}",
        serde_json::json!({"type":"qualification_restored", "seed_sha256":expected_sha256, "forge_dir":record.seed.logical_path})
    );
    Ok(())
}
