use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::Receiver;

use serde_json::{Value, json};

use crate::event::Event;
use crate::recorder::{HISTORY_US, History};

pub(crate) struct Snapshot {
    events: Vec<Event>,
    now: u64,
    dropped: u64,
    evicted: u64,
    reason: &'static str,
}

impl Snapshot {
    pub fn new(history: &History, now: u64, dropped: u64, reason: &'static str) -> Self {
        let mut events: Vec<_> = history.events.iter().cloned().collect();
        let begins: HashSet<_> = events
            .iter()
            .filter(|e| e.phase == "b")
            .filter_map(|e| e.id)
            .collect();
        events.extend(history.threads.values().cloned());
        // Preserve an unfinished operation even when its beginning aged out.
        for (begin, _) in history.open.values() {
            if !begins.contains(&begin.id.unwrap_or(0)) {
                let mut begin = begin.clone();
                begin.ts = begin.ts.max(now.saturating_sub(HISTORY_US));
                begin.args["begin_truncated"] = json!(true);
                events.push(begin);
            }
        }
        events.sort_by_key(|e| (e.ts, e.phase == "e"));
        let mut seen = HashSet::new();
        events.retain(|event| match (event.phase, event.id) {
            ("b", Some(id)) => seen.insert(id),
            ("e", Some(id)) => seen.remove(&id),
            _ => true,
        });
        // Close unfinished async slices at the snapshot boundary, explicitly
        // marking them unfinished, so every viewer displays their duration.
        let unfinished: Vec<_> = events
            .iter()
            .filter(|e| e.phase == "b" && e.id.is_some_and(|id| seen.contains(&id)))
            .cloned()
            .collect();
        for mut event in unfinished {
            event.ts = now;
            event.phase = "e";
            event.args = json!({"unfinished": true});
            events.push(event);
        }
        Self {
            events,
            now,
            dropped,
            evicted: history.evicted,
            reason,
        }
    }

    pub fn json(&self, role: &str, pid: u32) -> Value {
        let tids: HashSet<_> = self.events.iter().map(|e| e.tid).collect();
        let mut events = vec![
            json!({"ph":"M", "name":"process_name", "pid":pid, "tid":0, "args":{"name":role}}),
        ];
        for tid in tids {
            events.push(json!({"ph":"M", "name":"thread_name", "pid":pid, "tid":tid, "args":{"name":format!("{role} thread {tid}")}}));
        }
        events.extend(self.events.iter().map(|event| event.json(pid)));
        events.push(json!({"ph":"i", "s":"g", "cat":"recorder", "name":"capture", "ts":self.now, "pid":pid, "tid":0,
            "args":{"reason":self.reason, "dropped":self.dropped, "evicted":self.evicted}}));
        json!({"traceEvents":events, "displayTimeUnit":"ms", "metadata":{
            "format_version":1, "role":role, "reason":self.reason,
            "session_start_us": crate::recorder::session_start_us(),
            "build": artisan_build_info::version_line(),
            "clock":"Unix microseconds anchored to process monotonic clock",
            "dropped_records":self.dropped, "evicted_records":self.evicted,
        }})
    }
}

pub(crate) fn writer(snapshots: Receiver<Snapshot>, directory: &Path, role: &str) {
    let pid = std::process::id();
    for (sequence, snapshot) in snapshots.into_iter().enumerate() {
        if let Err(error) = write_snapshot(directory, role, pid, sequence, &snapshot) {
            eprintln!("Artisan trace export failed: {error}");
        }
    }
}

fn write_snapshot(
    directory: &Path,
    role: &str,
    pid: u32,
    sequence: usize,
    snapshot: &Snapshot,
) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let stem = format!("{role}-{pid}-{}-{sequence}", snapshot.now);
    let partial = directory.join(format!("{stem}.partial"));
    let destination = directory.join(format!("{stem}.json"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut output = BufWriter::new(options.open(&partial)?);
        serde_json::to_writer(&mut output, &snapshot.json(role, pid))?;
        output.flush()?;
        drop(output);
        fs::rename(&partial, &destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result?;
    retain_recent(directory, role)?;
    eprintln!(
        "Artisan trace saved ({}): {}",
        snapshot.reason,
        destination.display()
    );
    Ok(())
}

fn retain_recent(directory: &Path, role: &str) -> io::Result<()> {
    let prefix = format!("{role}-");
    let mut files: Vec<_> = fs::read_dir(directory)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_name().to_string_lossy().starts_with(&prefix)
                && entry.path().extension().is_some_and(|ext| ext == "json")
        })
        .filter_map(|entry| {
            entry
                .metadata()
                .ok()
                .filter(std::fs::Metadata::is_file)
                .and_then(|m| m.modified().ok())
                .map(|modified| (modified, entry.path()))
        })
        .collect();
    files.sort_by_key(|(modified, _)| *modified);
    let remove = files.len().saturating_sub(16);
    for (_, path) in files.into_iter().take(remove) {
        fs::remove_file(path)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../tests/tracing/export.rs"]
mod tests;
