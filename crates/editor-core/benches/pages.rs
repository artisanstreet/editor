use artisan_api::{
    Event, Thread, ThreadsPage,
    tls::{Identity, fingerprint},
};
use divan::Bencher;
use editor_core::runtime::runtime;
use editor_core::{
    Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, ThreadPageDirection,
};
use std::net::TcpListener;

const THREADS: u64 = 1_000;
const UPDATES: u64 = 100;
const CHUNK: u64 = 25;

struct Silent {
    _directory: tempfile::TempDir,
    _listener: TcpListener,
    core: Core,
    host: HostId,
}

fn thread(index: u64, seq: u64, activity: u64) -> Thread {
    serde_json::from_value(serde_json::json!({
        "id": format!("thread-{index}"),
        "project_id": "project",
        "title": format!("Thread {index}"),
        "config": {"engine": "claude"},
        "activity_seq": activity,
        "read_seq": activity,
        "seq": seq,
        "created_at": 0,
        "updated_at": 0,
        "archived": false,
    }))
    .expect("bench: thread")
}

fn silent_host() -> Silent {
    let directory = tempfile::tempdir().expect("bench: temporary directory");
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .expect("bench: core");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bench: listener");
    let identity = Identity::generate("pages-bench").expect("bench: identity");
    let pin = fingerprint(
        identity
            .certificate_der()
            .expect("bench: certificate")
            .as_ref(),
    );
    let config = HostConfig {
        id: HostId("silent-forge".into()),
        name: "Silent".into(),
        address: listener.local_addr().expect("bench: address").to_string(),
        fingerprint: pin.clone(),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(&config.address, &pin, &identity).expect("bench: client");
    core.add_test_host(&config, client);
    let host = core.host_mut(&config.id).expect("bench: host");
    host.test_stop_connection();
    host.load_threads(false, ThreadPageDirection::Latest);
    let generation = host.test_thread_page_generation(false);
    let sender = host.test_transition_sender();
    runtime().block_on(
        sender.thread_page(
            false,
            ThreadPageDirection::Latest,
            0,
            generation,
            ThreadsPage {
                threads: (0..THREADS)
                    .map(|index| thread(index, index, index))
                    .collect(),
                runs: Vec::new(),
                next: None,
                newer: false,
            },
        ),
    );
    let mut silent = Silent {
        _directory: directory,
        _listener: listener,
        core,
        host: config.id,
    };
    while silent
        .core
        .host(&silent.host)
        .expect("bench: host")
        .thread_page(false)
        .is_loading()
    {
        silent.core.drain_with(|_, _| {});
    }
    silent
}

fn main() {
    divan::main();
}

#[divan::bench(sample_count = 20)]
fn apply_a_burst_of_thread_updates(bencher: Bencher<'_, '_>) {
    let mut silent = silent_host();
    let sender = silent
        .core
        .host(&silent.host)
        .expect("bench: host")
        .test_transition_sender();
    let mut seq = THREADS;
    bencher.bench_local(|| {
        for chunk in 0..UPDATES / CHUNK {
            runtime().block_on(async {
                for update in chunk * CHUNK..(chunk + 1) * CHUNK {
                    seq += 1;
                    sender
                        .event(Event::Thread {
                            thread: thread((update * 37) % THREADS, seq, seq),
                        })
                        .await;
                }
            });
            while silent.core.drain_step(|_, _| {}) {}
        }
    });
}
