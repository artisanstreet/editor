use crate::error::{StorageError, operation_error};
use crate::writer::Tx;
use crate::{Store, read};
use artisan_api::{Event, Thread, ThreadId, Timestamp};
use rusqlite::{Connection, params};
use std::collections::{HashMap, HashSet};

const THREADS: &str = include_str!("retention/threads.sql");
const REMOVE: &str = include_str!("retention/remove.sql");
const TOMBSTONE: &str = include_str!("retention/tombstone.sql");

type Result<T, E = StorageError> = std::result::Result<T, E>;

operation_error!(RemoveInactiveError(StorageError), "remove the threads inactive since {cutoff}", cutoff: Timestamp);

struct Candidate {
    parent: Option<ThreadId>,
    inactive: bool,
}

impl Store {
    pub async fn remove_inactive_threads(
        &self,
        cutoff: Timestamp,
        live: HashSet<ThreadId>,
        limit: usize,
    ) -> Result<Vec<Thread>, RemoveInactiveError> {
        self.write(move |tx| {
            let removable = removable_families(tx, cutoff, &live)?;
            let mut removed = Vec::new();
            for family in removable {
                if !removed.is_empty() && removed.len() + family.len() > limit {
                    break;
                }
                for id in family {
                    if let Some(thread) = tx.remove_thread(&id)? {
                        removed.push(thread);
                    }
                }
            }
            Ok(removed)
        })
        .await
        .map_err(|cause| RemoveInactiveError {
            cutoff,
            cause: Box::new(cause),
        })
    }
}

impl Tx<'_> {
    pub(crate) fn remove_thread(&mut self, id: &ThreadId) -> Result<Option<Thread>> {
        let Some(thread) = read::thread(self, id)? else {
            return Ok(None);
        };
        for statement in REMOVE
            .split(';')
            .map(str::trim)
            .filter(|sql| !sql.is_empty())
        {
            self.sql.prepare_cached(statement)?.execute([id.as_str()])?;
        }
        let seq = self.next_seq();
        self.sql
            .prepare_cached(TOMBSTONE)?
            .execute(params![id.as_str(), seq])?;
        self.emit(
            seq,
            Event::ThreadRemoved {
                thread_id: id.clone(),
            },
        );
        Ok(Some(thread))
    }
}

fn removable_families(
    conn: &Connection,
    cutoff: Timestamp,
    live: &HashSet<ThreadId>,
) -> Result<Vec<Vec<ThreadId>>> {
    let mut statement = conn.prepare_cached(THREADS)?;
    let candidates = statement
        .query_map([cutoff], |row| {
            Ok((
                ThreadId(row.get(0)?),
                Candidate {
                    parent: row.get::<_, Option<String>>(1)?.map(ThreadId),
                    inactive: row.get(2)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(families(&candidates, live))
}

fn families(
    candidates: &HashMap<ThreadId, Candidate>,
    live: &HashSet<ThreadId>,
) -> Vec<Vec<ThreadId>> {
    let mut members: HashMap<&ThreadId, Vec<ThreadId>> = HashMap::new();
    for id in candidates.keys() {
        members
            .entry(family_root(candidates, id))
            .or_default()
            .push(id.clone());
    }
    let mut removable: Vec<Vec<ThreadId>> = members
        .into_values()
        .filter(|family| {
            family
                .iter()
                .all(|id| candidates[id].inactive && !live.contains(id))
        })
        .collect();
    for family in &mut removable {
        family.sort();
    }
    removable.sort();
    removable
}

fn family_root<'a>(candidates: &'a HashMap<ThreadId, Candidate>, id: &'a ThreadId) -> &'a ThreadId {
    let mut root = id;
    let mut seen = HashSet::from([id]);
    while let Some(parent) = candidates[root]
        .parent
        .as_ref()
        .and_then(|parent| candidates.get_key_value(parent).map(|(key, _)| key))
    {
        if !seen.insert(parent) {
            break;
        }
        root = parent;
    }
    root
}
