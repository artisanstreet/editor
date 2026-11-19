use crate::error::AtStep as _;
use crate::error::{StorageError, operation_error};
use crate::writer::Tx;
use crate::{Store, read};
use artisan_api::{Event, Seq, Thread, ThreadId, Timestamp};
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const THREADS: &str = include_str!("retention/threads.sql");
const REMOVE: &str = include_str!("retention/remove.sql");
const TOMBSTONE: &str = include_str!("retention/tombstone.sql");
const ELIGIBLE: &str = include_str!("retention/eligible.sql");
const JOINED: &str = include_str!("retention/joined.sql");

type Result<T, E = StorageError> = std::result::Result<T, E>;

operation_error!(RemoveInactiveError(StorageError), "remove the threads inactive since {cutoff}", cutoff: Timestamp);

struct Candidate {
    parent: Option<ThreadId>,
    inactive: bool,
}

struct Plan {
    seq: Seq,
    families: Vec<Vec<ThreadId>>,
}

impl Store {
    pub async fn remove_inactive_threads(
        &self,
        cutoff: Timestamp,
        live: HashSet<ThreadId>,
        limit: usize,
    ) -> Result<Vec<Thread>, RemoveInactiveError> {
        let failed = |cause| RemoveInactiveError {
            cutoff,
            cause: Box::new(cause),
        };
        let plan = self
            .scan(move |conn| plan(conn, cutoff))
            .await
            .map_err(failed)?;
        let live = Arc::new(live);
        let mut removed = Vec::new();
        let mut families = plan.families.into_iter().peekable();
        while families.peek().is_some() {
            let mut batch = Vec::new();
            let mut size = 0;
            while let Some(family) =
                families.next_if(|family| batch.is_empty() || size + family.len() <= limit)
            {
                size += family.len();
                batch.push(family);
            }
            let live = Arc::clone(&live);
            let seq = plan.seq;
            removed.extend(
                self.write(move |tx| remove_batch(tx, cutoff, seq, &live, batch))
                    .await
                    .map_err(failed)?,
            );
        }
        Ok(removed)
    }
}

fn remove_batch(
    tx: &mut Tx<'_>,
    cutoff: Timestamp,
    seq: Seq,
    live: &HashSet<ThreadId>,
    batch: Vec<Vec<ThreadId>>,
) -> Result<Vec<Thread>> {
    let mut removed = Vec::new();
    for family in batch {
        if !still_removable(tx, cutoff, seq, live, &family)? {
            continue;
        }
        for id in family {
            if let Some(thread) = tx.remove_thread(&id)? {
                removed.push(thread);
            }
        }
    }
    Ok(removed)
}

fn still_removable(
    conn: &Connection,
    cutoff: Timestamp,
    seq: Seq,
    live: &HashSet<ThreadId>,
    family: &[ThreadId],
) -> Result<bool> {
    if family.iter().any(|id| live.contains(id)) {
        return Ok(false);
    }
    let ids = serde_json::to_string(family).map_err(StorageError::encode::<[ThreadId]>)?;
    let eligible = conn
        .prepare_cached(ELIGIBLE)
        .at("retention/eligible.sql")?
        .query_map(params![cutoff, ids], |row| row.get::<_, String>(0))
        .at("retention/eligible.sql")?
        .count();
    if eligible != family.len() {
        return Ok(false);
    }
    let joined: bool = conn
        .prepare_cached(JOINED)
        .at("retention/joined.sql")?
        .query_row(params![seq, ids], |row| row.get(0))
        .at("retention/joined.sql")?;
    Ok(!joined)
}

impl Tx<'_> {
    fn remove_thread(&mut self, id: &ThreadId) -> Result<Option<Thread>> {
        let Some(thread) = read::thread(self, id)? else {
            return Ok(None);
        };
        for statement in REMOVE
            .split(';')
            .map(str::trim)
            .filter(|sql| !sql.is_empty())
        {
            self.sql
                .prepare_cached(statement)
                .at("retention/remove.sql")?
                .execute([id.as_str()])
                .at("retention/remove.sql")?;
        }
        let seq = self.next_seq();
        self.sql
            .prepare_cached(TOMBSTONE)
            .at("retention/tombstone.sql")?
            .execute(params![id.as_str(), seq])
            .at("retention/tombstone.sql")?;
        self.emit(
            seq,
            Event::ThreadRemoved {
                thread_id: id.clone(),
            },
        );
        Ok(Some(thread))
    }
}

fn plan(conn: &Connection, cutoff: Timestamp) -> Result<Plan> {
    let seq = read::max_seq(conn)?;
    let mut statement = conn.prepare_cached(THREADS).at("retention/threads.sql")?;
    let candidates = statement
        .query_map([cutoff], |row| {
            Ok((
                ThreadId(row.get(0)?),
                Candidate {
                    parent: row.get::<_, Option<String>>(1)?.map(ThreadId),
                    inactive: row.get(2)?,
                },
            ))
        })
        .at("retention/threads.sql")?
        .collect::<rusqlite::Result<HashMap<_, _>>>()
        .at("retention/threads.sql")?;
    Ok(Plan {
        seq,
        families: families(&candidates),
    })
}

fn families(candidates: &HashMap<ThreadId, Candidate>) -> Vec<Vec<ThreadId>> {
    let mut roots: HashMap<&ThreadId, &ThreadId> = HashMap::with_capacity(candidates.len());
    let mut members: HashMap<&ThreadId, Vec<ThreadId>> = HashMap::new();
    let mut ids: Vec<&ThreadId> = candidates.keys().collect();
    ids.sort_unstable();
    for id in ids {
        let root = family_root(candidates, &mut roots, id);
        members.entry(root).or_default().push(id.clone());
    }
    let mut removable: Vec<Vec<ThreadId>> = members
        .into_values()
        .filter(|family| family.iter().all(|id| candidates[id].inactive))
        .collect();
    for family in &mut removable {
        family.sort();
    }
    removable.sort();
    removable
}

fn family_root<'a>(
    candidates: &'a HashMap<ThreadId, Candidate>,
    roots: &mut HashMap<&'a ThreadId, &'a ThreadId>,
    id: &'a ThreadId,
) -> &'a ThreadId {
    let mut path = Vec::new();
    let mut seen = HashSet::from([id]);
    let mut at = id;
    let root = loop {
        if let Some(root) = roots.get(at) {
            break *root;
        }
        path.push(at);
        match candidates[at]
            .parent
            .as_ref()
            .and_then(|parent| candidates.get_key_value(parent).map(|(key, _)| key))
        {
            Some(parent) if seen.insert(parent) => at = parent,
            Some(_) | None => break at,
        }
    };
    for member in path {
        roots.insert(member, root);
    }
    root
}
