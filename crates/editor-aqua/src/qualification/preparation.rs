use super::fixture::{ForgeServer, ImmutableFixture, SeedSnapshot};
use anyhow::{Context, Result, ensure};
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

fn marked(path: &Path) -> Result<PathBuf> {
    ensure!(
        path.is_absolute(),
        "qualification directory must be absolute"
    );
    let canonical = super::fixture::canonical_path(path)?;
    ensure!(
        canonical == path,
        "qualification directory must be canonical and contain no symlinks"
    );
    ensure!(
        std::fs::read(path.join(".artisan-qualification"))? == b"artisan-qualification-v1\n",
        "invalid qualification marker"
    );
    Ok(canonical)
}

fn fresh(path: &Path, settings: bool) -> Result<Option<String>> {
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        ensure!(
            name == ".artisan-qualification" || (settings && name == "settings.json"),
            "qualification directory is not fresh"
        );
        ensure!(
            entry.file_type()?.is_file(),
            "qualification entry is not a regular file"
        );
    }
    let settings_path = path.join("settings.json");
    if settings_path.exists() {
        Ok(Some(hash(&std::fs::read(settings_path)?)))
    } else {
        Ok(None)
    }
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

pub fn prepare(path: &Path) -> Result<()> {
    let request: Preparation = serde_json::from_slice(&std::fs::read(path)?)?;
    let fixture_path = super::fixture::canonical_path(&request.fixture)?;
    ensure!(
        request.fixture == fixture_path && std::fs::symlink_metadata(&fixture_path)?.is_file(),
        "fixture must be a canonical regular file"
    );
    let fixture = ImmutableFixture::read(&fixture_path)?;
    let dirs = [
        &request.forge_dir,
        &request.snapshot_dir,
        &request.output_dir,
        &request.source_editor_dir,
        &request.candidate_editor_dir,
    ]
    .into_iter()
    .map(|path| marked(path))
    .collect::<Result<Vec<_>>>()?;
    for (index, left) in dirs.iter().enumerate() {
        ensure!(
            !fixture_path.starts_with(left),
            "fixture overlaps a mutable qualification directory"
        );
        for right in &dirs[index + 1..] {
            ensure!(
                !left.starts_with(right) && !right.starts_with(left),
                "qualification directories overlap"
            );
        }
    }
    for dir in &dirs[..3] {
        fresh(dir, false)?;
    }
    let source_settings = fresh(&dirs[3], true)?;
    let candidate_settings = fresh(&dirs[4], true)?;
    ensure!(
        source_settings == candidate_settings,
        "Editor settings differ between qualification runs"
    );
    ensure!(
        request.listen.ip().is_loopback() && request.listen.port() != 0,
        "qualification requires a fixed loopback port"
    );
    ensure!(
        !request.host_id.trim().is_empty()
            && fixture
                .fixture()
                .threads
                .iter()
                .any(|thread| thread.id.as_str() == request.thread_id),
        "invalid qualification host or thread"
    );
    ensure!(
        request.width.is_finite()
            && request.height.is_finite()
            && request.width >= 480.0
            && request.height >= 420.0,
        "invalid qualification dimensions"
    );
    let runtime = crate::core::runtime::runtime();
    let server = runtime.block_on(ForgeServer::start(dirs[0].clone(), request.listen))?;
    let seed = runtime.block_on(SeedSnapshot::produce(
        server,
        fixture.fixture(),
        &dirs[0],
        &dirs[1],
    ))?;
    fixture.verify_unchanged(&fixture_path)?;
    let store = serde_json::to_vec(&seed.store)?;
    let store_path = dirs[2].join("store.json");
    let seed_path = dirs[2].join("seed.json");
    let database_hash = hash(&std::fs::read(dirs[0].join("forge.sqlite3"))?);
    let seed_bytes = serde_json::to_vec(&SeedRecord {
        seed,
        snapshot_dir: dirs[1].clone(),
    })?;
    write_new(&store_path, &store)?;
    write_new(&seed_path, &seed_bytes)?;
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
            &serde_json::to_vec(&value)?,
        )?;
    }
    println!(
        "{}",
        serde_json::json!({"type":"qualification_seed", "seed":seed_path, "seed_sha256":hash(&seed_bytes), "source_request":dirs[2].join("source-request.json"), "candidate_request":dirs[2].join("candidate-request.json")})
    );
    Ok(())
}

pub fn restore(path: &Path, expected_sha256: &str) -> Result<()> {
    let parent = path.parent().context("seed record has no parent")?;
    marked(parent)?;
    ensure!(
        path.file_name().is_some_and(|name| name == "seed.json")
            && std::fs::symlink_metadata(path)?.is_file(),
        "invalid seed record path"
    );
    let bytes = std::fs::read(path)?;
    ensure!(hash(&bytes) == expected_sha256, "seed record hash mismatch");
    let record: SeedRecord = serde_json::from_slice(&bytes)?;
    record
        .seed
        .restore_after_all_forge_handles_closed(&record.snapshot_dir, &record.seed.logical_path)?;
    println!(
        "{}",
        serde_json::json!({"type":"qualification_restored", "seed_sha256":expected_sha256, "forge_dir":record.seed.logical_path})
    );
    Ok(())
}
