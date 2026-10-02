use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

static NEXT_THREAD: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static THREAD: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn thread_id() -> u64 {
    THREAD.with(|slot| {
        if slot.get() == 0 {
            slot.set(NEXT_THREAD.fetch_add(1, Ordering::Relaxed));
        }
        slot.get()
    })
}

#[derive(Clone, Debug)]
pub(crate) struct Event {
    pub ts: u64,
    pub tid: u64,
    pub category: &'static str,
    pub name: &'static str,
    pub phase: &'static str,
    pub id: Option<u64>,
    pub args: Value,
}

impl Event {
    pub fn json(&self, pid: u32) -> Value {
        let mut event = json!({
            "ts": self.ts, "pid": pid, "tid": self.tid,
            "cat": self.category, "name": self.name, "ph": self.phase,
            "args": self.args,
        });
        if let Some(id) = self.id {
            // Hex strings preserve 64-bit identities in JavaScript viewers.
            event["id"] = json!(format!("{id:x}"));
        }
        if self.phase == "i" {
            event["s"] = json!("t");
        }
        if self.phase == "X" {
            event["dur"] = self.args["duration_us"].clone();
        }
        if matches!(self.phase, "s" | "f") {
            // Flow endpoints need their own thread slices: an async span
            // lives on a separate track and cannot enclose a legacy flow.
            event["ph"] = json!("X");
            // One microsecond keeps the endpoint on the parser's enclosing
            // slice stack while it binds the flow. This is a marker, not work.
            event["dur"] = json!(1);
            event["bind_id"] = event["id"].take();
            event.as_object_mut().unwrap().remove("id");
            event[if self.phase == "s" {
                "flow_out"
            } else {
                "flow_in"
            }] = json!(true);
        }
        event
    }
}

/// Bound each record as well as the record count. Arbitrary payload trees
/// are rejected; strings are bounded, and only sixteen scalar fields survive.
pub(crate) fn bounded_args(args: Value) -> Value {
    let Value::Object(fields) = args else {
        return json!({});
    };
    let fields = fields.into_iter().take(16).filter_map(|(key, mut value)| {
        if key.len() > 64 || matches!(value, Value::Array(_) | Value::Object(_)) {
            return None;
        }
        if let Value::String(text) = &mut value {
            let end = text.char_indices().nth(128).map_or(text.len(), |(i, _)| i);
            text.truncate(end);
        }
        Some((key, value))
    });
    Value::Object(fields.collect())
}
