//! Indexed canonical collections in the existing immutable content owner.
use anyhow::{Context, Result, ensure};
use artisan_api::{CollectionQuery, CollectionRange, CollectionRef, ContentId, ContentRef};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::fmt::Write as _;

const MAX_COLLECTION_BYTES: u64 = 64 * 1024 * 1024;

fn encode<T: Serialize>(record: &T) -> Result<String> {
    let json = serde_json::to_string(record)?;
    ensure!(
        json.len() <= forge_content::INLINE_COLLECTION_BYTES,
        "canonical record metadata exceeds 64 KiB; authored text must use a source reference"
    );
    Ok(json)
}

fn record_key(encoded: &str) -> Result<(Option<String>, Option<String>)> {
    let value: serde_json::Value = serde_json::from_str(encoded)?;
    let key = match &value {
        serde_json::Value::String(key) => Some(key.clone()),
        serde_json::Value::Object(record) => record
            .get("id")
            .and_then(|id| id.as_str())
            .map(str::to_owned),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Array(_) => None,
    };
    ensure!(
        key.as_ref()
            .is_none_or(|key| !key.is_empty() && key.len() <= 4096),
        "invalid canonical record identity"
    );
    let label = value
        .get("label")
        .and_then(|label| label.as_str())
        .map(str::to_owned);
    Ok((key, label))
}

pub fn store<T: Serialize>(
    conn: &Connection,
    records: &[T],
    dependencies: &BTreeSet<ContentId>,
    authored_bytes: u64,
    nested_structural_bytes: u64,
) -> Result<CollectionRef> {
    // First pass computes identity with at most one encoded record in memory.
    // Existing immutable collections do not incur chunk or record writes.
    let reference = forge_content::collection_ref(records)?;
    let source = reference.source;
    ensure!(
        source
            .bytes
            .saturating_add(authored_bytes)
            .saturating_add(nested_structural_bytes)
            <= MAX_COLLECTION_BYTES,
        "canonical collection authored and structural data exceeds 64 MiB"
    );
    for child in dependencies {
        ensure!(child != &source.id, "source cannot depend on itself");
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM content WHERE id=?1)",
            [child.as_str()],
            |row| row.get(0),
        )?;
        ensure!(exists, "collection source dependency is missing");
    }
    let inserted = conn.execute("INSERT INTO content(id,bytes,lines,diff_added,diff_removed,unreferenced_at) VALUES(?1,?2,?3,0,0,?4) ON CONFLICT(id) DO NOTHING",
        params![source.id.as_str(), source.bytes, source.lines, artisan_api::now()])? != 0;
    // A pre-existing ordinary text source with identical NDJSON bytes can become
    // an indexed collection. It still has one content identity and one lifecycle.
    let indexed = conn.execute("INSERT INTO content_collections(content_id,count,authored_bytes,structural_bytes) VALUES(?1,?2,?3,?4) ON CONFLICT(content_id) DO NOTHING",
        params![source.id.as_str(), records.len() as u64,authored_bytes,source.bytes.saturating_add(nested_structural_bytes)])? != 0;
    if inserted || indexed {
        let mut insert_record = conn.prepare_cached(
            "INSERT INTO content_records(content_id,ordinal,start,end,record_key,record_label) VALUES(?1,?2,?3,?4,?5,?6)",
        )?;
        let mut insert_chunk = conn
            .prepare_cached("INSERT INTO content_chunks(content_id,start,data) VALUES(?1,?2,?3)")?;
        let mut buffer = String::with_capacity(if inserted {
            crate::content::CHUNK_BYTES + artisan_api::COLLECTION_RANGE_BYTES
        } else {
            0
        });
        let mut chunk_start = 0u64;
        let mut position = 0u64;
        for (ordinal, record) in records.iter().enumerate() {
            let encoded = encode(record)?;
            let end = position + encoded.len() as u64;
            if indexed {
                let (key, label) = record_key(&encoded)?;
                insert_record.execute(params![
                    source.id.as_str(),
                    ordinal as u64,
                    position,
                    end,
                    key,
                    label
                ])?;
            }
            position = end + 1;
            if !inserted {
                continue;
            }
            buffer.push_str(&encoded);
            buffer.push('\n');
            while buffer.len() >= crate::content::CHUNK_BYTES {
                let mut boundary = crate::content::CHUNK_BYTES;
                while !buffer.is_char_boundary(boundary) {
                    boundary -= 1;
                }
                insert_chunk.execute(params![
                    source.id.as_str(),
                    chunk_start,
                    &buffer.as_bytes()[..boundary]
                ])?;
                buffer.drain(..boundary);
                chunk_start += boundary as u64;
            }
        }
        if !buffer.is_empty() {
            insert_chunk.execute(params![source.id.as_str(), chunk_start, buffer.as_bytes()])?;
        }
    }
    let known: u64 = conn.query_row(
        "SELECT count FROM content_collections WHERE content_id=?1",
        [source.id.as_str()],
        |row| row.get(0),
    )?;
    ensure!(
        known == records.len() as u64,
        "collection descriptor mismatch"
    );
    for child in dependencies {
        conn.execute("INSERT INTO content_dependencies(parent_id,child_id) VALUES(?1,?2) ON CONFLICT DO NOTHING", params![source.id.as_str(),child.as_str()])?;
    }
    Ok(CollectionRef {
        source,
        count: records.len() as u64,
    })
}

pub fn range<T: DeserializeOwned>(
    conn: &Connection,
    id: &ContentId,
    query: CollectionQuery,
) -> Result<Option<CollectionRange<T>>> {
    ensure!(id.is_valid(), "invalid collection identity");
    let limit = query.limit.unwrap_or(artisan_api::COLLECTION_RANGE_ENTRIES);
    ensure!(
        (1..=artisan_api::COLLECTION_RANGE_ENTRIES).contains(&limit),
        "collection limit must be 1..64"
    );
    let Some((bytes, lines, total)) = conn.query_row("SELECT c.bytes,c.lines,r.count FROM content c JOIN content_collections r ON r.content_id=c.id WHERE c.id=?1", [id.as_str()], |row| Ok((row.get::<_,u64>(0)?,row.get::<_,u64>(1)?,row.get::<_,u64>(2)?))).optional()? else { return Ok(None) };
    ensure!(
        query.offset <= total,
        "collection offset exceeds record count"
    );
    let mut select = conn.prepare_cached("SELECT ordinal,start,end FROM content_records WHERE content_id=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3")?;
    let mut rows = select.query(params![id.as_str(), query.offset, limit])?;
    let mut records = Vec::new();
    let mut loaded_bytes = 0usize;
    let mut byte_limited = false;
    while let Some(row) = rows.next()? {
        let ordinal: u64 = row.get(0)?;
        let start: u64 = row.get(1)?;
        let end: u64 = row.get(2)?;
        ensure!(
            ordinal == query.offset + records.len() as u64 && end >= start && end <= bytes,
            "collection index is missing or inconsistent"
        );
        let size = usize::try_from(end - start)?;
        ensure!(
            size <= forge_content::INLINE_COLLECTION_BYTES,
            "stored record exceeds range budget"
        );
        if loaded_bytes + size > artisan_api::COLLECTION_RANGE_BYTES {
            byte_limited = true;
            break;
        }
        let source = crate::content::range(
            conn,
            id,
            &artisan_api::ContentQuery {
                offset: start,
                limit: Some(
                    u32::try_from(size.max(4)).map_or(artisan_api::CONTENT_RANGE_BYTES, |size| {
                        size.min(artisan_api::CONTENT_RANGE_BYTES)
                    }),
                ),
            },
        )?
        .context("collection source is missing")?;
        ensure!(
            source.start == start && source.end >= end,
            "collection record source is incomplete"
        );
        records.push(
            serde_json::from_str::<T>(&source.text[..size])
                .context("canonical collection record has the wrong shape")?,
        );
        loaded_bytes += size;
    }
    ensure!(
        query.offset == total || !records.is_empty(),
        "collection index is missing"
    );
    ensure!(
        byte_limited
            || query.offset + records.len() as u64
                == query.offset.saturating_add(u64::from(limit)).min(total),
        "collection index has a missing trailing record"
    );
    Ok(Some(CollectionRange {
        source: ContentRef {
            id: id.clone(),
            bytes,
            lines,
        },
        start: query.offset,
        end: query.offset + records.len() as u64,
        total,
        records,
    }))
}

#[expect(
    clippy::ref_option,
    reason = "forwards to artisan_api::Question's validators, which take &Option"
)]
pub fn validate_answer(
    conn: &Connection,
    question: &artisan_api::Question,
    answer: &Option<artisan_api::QuestionAnswer>,
) -> Result<()> {
    let refused = |message: &str| crate::Refused(message.to_owned());
    question
        .validate_answer_structure(answer)
        .map_err(refused)?;
    if let Some(artisan_api::QuestionAnswer::Choices { ids }) = answer {
        let ids = ids
            .inline()
            .ok_or_else(|| refused("submitted choices must be inline"))?;
        match &question.options {
            artisan_api::Collection::Inline(_) => {
                question.validate_answer(answer).map_err(refused)?;
            }
            artisan_api::Collection::Stored(reference) => {
                metrics(conn, reference)?;
                let mut exists=conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM content_records WHERE content_id=?1 AND record_key=?2)")?;
                for id in ids {
                    let known: bool = exists
                        .query_row(params![reference.source.id.as_str(), id], |row| row.get(0))?;
                    ensure!(known, refused("unknown question choice"));
                }
            }
        }
    }
    Ok(())
}
pub fn answer_text(
    conn: &Connection,
    question: &artisan_api::Question,
    answer: &artisan_api::QuestionAnswer,
) -> Result<String> {
    let artisan_api::Collection::Stored(reference) = &question.options else {
        return question.answer_text(answer).map_err(anyhow::Error::msg);
    };
    let artisan_api::QuestionAnswer::Choices { ids } = answer else {
        let artisan_api::QuestionAnswer::Text { text } = answer else {
            unreachable!()
        };
        return Ok(text.clone());
    };
    metrics(conn, reference)?;
    let loaded;
    let ids = match ids {
        artisan_api::Collection::Inline(ids) => ids.as_slice(),
        artisan_api::Collection::Stored(_) => {
            loaded = resolve::<String>(conn, ids)?;
            loaded.as_slice()
        }
    };
    let mut labels = String::new();
    let mut duplicate_counts = std::collections::HashMap::<String, u64>::new();
    let mut option = conn.prepare_cached(
        "SELECT ordinal,record_label FROM content_records WHERE content_id=?1 AND record_key=?2",
    )?;
    let mut count = conn.prepare_cached(
        "SELECT COUNT(*) FROM content_records WHERE content_id=?1 AND record_label=?2",
    )?;
    for id in ids {
        let (index, label): (u64, String) = option
            .query_row(params![reference.source.id.as_str(), id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()?
            .context("unknown question choice")?;
        let duplicates = if let Some(value) = duplicate_counts.get(&label) {
            *value
        } else {
            let value: u64 = count
                .query_row(params![reference.source.id.as_str(), label], |row| {
                    row.get(0)
                })?;
            duplicate_counts.insert(label.clone(), value);
            value
        };
        if !labels.is_empty() {
            labels.push_str(", ");
        }
        labels.push_str(&label);
        if duplicates > 1 {
            write!(labels, " (option {} of {})", index + 1, reference.count)?;
        }
        ensure!(
            labels.len() as u64 <= MAX_COLLECTION_BYTES,
            "answer display exceeds 64 MiB"
        );
    }
    Ok(labels)
}

/// Runs once when answers are first persisted or during the historical data
/// migration. Status-only native events inherit the same immutable display.
pub fn derive_answer_text(
    conn: &Connection,
    kind: &mut artisan_api::ItemKind,
    old: Option<&artisan_api::ItemKind>,
) -> Result<()> {
    let artisan_api::ItemKind::Question {
        questions,
        answers: Some(answers),
        answer_text: display,
        ..
    } = kind
    else {
        return Ok(());
    };
    if display.is_some() {
        return Ok(());
    }
    if let Some(artisan_api::ItemKind::Question {
        questions: old_questions,
        answers: Some(old_answers),
        answer_text: Some(text),
        ..
    }) = old
        && *questions == *old_questions
        && *answers == *old_answers
    {
        *display = Some(text.clone());
        return Ok(());
    }
    ensure!(
        answers.len() == questions.len(),
        "saved answer count does not match questions"
    );
    let mut cached: Option<CollectionRange<Option<artisan_api::QuestionAnswer>>> = None;
    let mut text = String::new();
    visit_questions(conn, questions, |index, question| {
        let index64 = index as u64;
        let answer = match answers {
            artisan_api::Collection::Inline(answers) => &answers[index],
            artisan_api::Collection::Stored(reference) => {
                if cached
                    .as_ref()
                    .is_none_or(|page| index64 < page.start || index64 >= page.end)
                {
                    cached = Some(
                        range(
                            conn,
                            &reference.source.id,
                            CollectionQuery {
                                offset: index64,
                                limit: None,
                            },
                        )?
                        .context("saved answer source is missing")?,
                    );
                }
                let page = cached.as_ref().context("saved answer page is missing")?;
                &page.records[usize::try_from(index64 - page.start)?]
            }
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        if questions.len() > 1 {
            write!(text, "Question {}: ", index + 1)?;
        }
        match answer {
            Some(answer) => text.push_str(&answer_text(conn, question, answer)?),
            None => text.push_str("You skipped this question."),
        }
        ensure!(
            text.len() as u64 <= MAX_COLLECTION_BYTES,
            "answer display exceeds 64 MiB"
        );
        Ok(())
    })?;
    *display = Some(text.into());
    Ok(())
}

#[derive(Default, Clone, Copy)]
pub struct Metrics {
    pub authored: u64,
    pub structural: u64,
}
pub fn metrics(conn: &Connection, reference: &CollectionRef) -> Result<Metrics> {
    ensure!(
        reference.source.id.is_valid(),
        "invalid collection source identity"
    );
    let value = conn.query_row("SELECT r.count,c.bytes,c.lines,r.authored_bytes,r.structural_bytes FROM content_collections r JOIN content c ON c.id=r.content_id WHERE r.content_id=?1",
        [reference.source.id.as_str()], |row| Ok((row.get::<_,u64>(0)?,row.get::<_,u64>(1)?,row.get::<_,u64>(2)?,row.get::<_,u64>(3)?,row.get::<_,u64>(4)?))).optional()?.context("collection source is missing")?;
    ensure!(
        reference.count == value.0
            && reference.source.bytes == value.1
            && reference.source.lines == value.2,
        "collection descriptor mismatch"
    );
    Ok(Metrics {
        authored: value.3,
        structural: value.4,
    })
}
fn dependency(text: &artisan_api::TextContent, ids: &mut BTreeSet<ContentId>) -> u64 {
    if let artisan_api::TextContent::Stored(source) = text {
        ids.insert(source.id.clone());
    }
    text.byte_len()
}
fn normalize<T: Serialize>(
    conn: &Connection,
    collection: &mut artisan_api::Collection<T>,
    mut record_metrics: impl FnMut(&Connection, &mut T, &mut BTreeSet<ContentId>) -> Result<Metrics>,
) -> Result<Metrics> {
    let records = match collection {
        artisan_api::Collection::Inline(records) => records,
        artisan_api::Collection::Stored(reference) => return metrics(conn, reference),
    };
    let mut total = Metrics::default();
    let mut dependencies = BTreeSet::new();
    let mut encoded_bytes = 0u64;
    for record in records.iter_mut() {
        let record_total = record_metrics(conn, record, &mut dependencies)?;
        total.authored = total.authored.saturating_add(record_total.authored);
        total.structural = total.structural.saturating_add(record_total.structural);
        let encoded = encode(record)?;
        crate::content::validate_metadata(&serde_json::from_str(&encoded)?, "")?;
        encoded_bytes = encoded_bytes.saturating_add(encoded.len() as u64 + 1);
        ensure!(
            total
                .authored
                .saturating_add(total.structural)
                .saturating_add(encoded_bytes)
                <= MAX_COLLECTION_BYTES,
            "collection data exceeds 64 MiB"
        );
    }
    if records.len() > forge_content::INLINE_COLLECTION_ENTRIES
        || encoded_bytes.saturating_add(1) > forge_content::INLINE_COLLECTION_BYTES as u64
    {
        let reference = store(
            conn,
            records,
            &dependencies,
            total.authored,
            total.structural,
        )?;
        total.structural = total.structural.saturating_add(reference.source.bytes);
        *collection = artisan_api::Collection::Stored(reference);
    }
    Ok(total)
}
fn normalize_options(
    conn: &Connection,
    options: &mut artisan_api::Collection<artisan_api::QuestionOption>,
) -> Result<Metrics> {
    normalize(conn, options, |_, option, ids| {
        Ok(Metrics {
            authored: option
                .description
                .as_ref()
                .map_or(0, |text| dependency(text, ids)),
            structural: 0,
        })
    })
}
pub fn normalize_item(conn: &Connection, kind: &mut artisan_api::ItemKind) -> Result<()> {
    use artisan_api::{ItemKind, ToolKind};
    match kind {
        ItemKind::Plan { steps, .. } => {
            normalize(conn, steps, |_, step, ids| {
                Ok(Metrics {
                    authored: dependency(&step.text, ids),
                    structural: 0,
                })
            })?;
        }
        ItemKind::Tool {
            tool: ToolKind::Edit { changes },
            ..
        } => {
            normalize(conn, changes, |_, change, ids| {
                Ok(Metrics {
                    authored: change.diff.as_ref().map_or(0, |text| dependency(text, ids)),
                    structural: 0,
                })
            })?;
        }
        ItemKind::Question {
            questions, answers, ..
        } => {
            normalize(conn, questions, |conn, question, ids| {
                let options = normalize_options(conn, &mut question.options)?;
                if let Some(reference) = question.options.stored() {
                    ids.insert(reference.source.id.clone());
                } else if let Some(options) = question.options.inline() {
                    for option in options {
                        if let Some(text) = &option.description {
                            dependency(text, ids);
                        }
                    }
                }
                Ok(Metrics {
                    authored: dependency(&question.text, ids).saturating_add(options.authored),
                    structural: options.structural,
                })
            })?;
            if let Some(answers) = answers {
                normalize(conn, answers, |conn, answer, ids| match answer {
                    Some(artisan_api::QuestionAnswer::Choices { ids: choices }) => {
                        let totals = normalize(conn, choices, |_, _, _| Ok(Metrics::default()))?;
                        if let Some(reference) = choices.stored() {
                            ids.insert(reference.source.id.clone());
                        }
                        Ok(totals)
                    }
                    Some(artisan_api::QuestionAnswer::Text { text }) => Ok(Metrics {
                        authored: text.len() as u64,
                        structural: 0,
                    }),
                    None => Ok(Metrics::default()),
                })?;
            }
        }
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => {}
    }
    Ok(())
}
pub fn source_metrics(conn: &Connection, source: &ContentRef) -> Result<Metrics> {
    let Some(count) = conn
        .query_row(
            "SELECT count FROM content_collections WHERE content_id=?1",
            [source.id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .optional()?
    else {
        anyhow::bail!("collection source is missing")
    };
    metrics(
        conn,
        &CollectionRef {
            source: source.clone(),
            count,
        },
    )
}

/// Resolving all records is reserved for admitted Forge operations such as a
/// native question callback. HTTP and the Client use indexed range reads.
pub fn resolve<T: DeserializeOwned>(
    conn: &Connection,
    collection: &artisan_api::Collection<T>,
) -> Result<Vec<T>> {
    let reference = collection
        .stored()
        .context("only stored collections need resolving")?;
    metrics(conn, reference)?;
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < reference.count {
        let page = range(
            conn,
            &reference.source.id,
            CollectionQuery {
                offset,
                limit: None,
            },
        )?
        .context("collection source is missing")?;
        ensure!(
            page.total == reference.count && page.end > offset,
            "collection descriptor changed"
        );
        offset = page.end;
        records.extend(page.records);
    }
    Ok(records)
}

pub fn visit_questions(
    conn: &Connection,
    questions: &artisan_api::Collection<artisan_api::Question>,
    mut visit: impl FnMut(usize, &artisan_api::Question) -> Result<()>,
) -> Result<()> {
    match questions {
        artisan_api::Collection::Inline(questions) => {
            for (index, question) in questions.iter().enumerate() {
                visit(index, question)?;
            }
        }
        artisan_api::Collection::Stored(reference) => {
            metrics(conn, reference)?;
            let mut offset = 0;
            while offset < reference.count {
                let page = range::<artisan_api::Question>(
                    conn,
                    &reference.source.id,
                    CollectionQuery {
                        offset,
                        limit: None,
                    },
                )?
                .context("question collection is missing")?;
                ensure!(
                    page.end > offset && page.total == reference.count,
                    "question collection page is inconsistent"
                );
                for (index, question) in page.records.iter().enumerate() {
                    visit(usize::try_from(offset)? + index, question)?;
                }
                offset = page.end;
            }
        }
    }
    Ok(())
}
pub fn validate_answers(
    conn: &Connection,
    questions: &artisan_api::Collection<artisan_api::Question>,
    answers: &[Option<artisan_api::QuestionAnswer>],
) -> Result<()> {
    ensure!(
        questions.len() == answers.len() as u64,
        crate::Refused("answer count does not match questions".into())
    );
    visit_questions(conn, questions, |index, question| {
        validate_answer(conn, question, &answers[index])
    })
}
pub fn answers_equal(
    conn: &Connection,
    saved: Option<&artisan_api::Collection<Option<artisan_api::QuestionAnswer>>>,
    answers: &[Option<artisan_api::QuestionAnswer>],
) -> Result<bool> {
    let Some(saved) = saved else { return Ok(false) };
    if saved.len() != answers.len() as u64 {
        return Ok(false);
    }
    let mut decoded = match saved {
        artisan_api::Collection::Inline(records) => records.clone(),
        artisan_api::Collection::Stored(_) => resolve(conn, saved)?,
    };
    for answer in decoded.iter_mut().flatten() {
        if let artisan_api::QuestionAnswer::Choices { ids } = answer
            && ids.stored().is_some()
        {
            *ids = resolve(conn, ids)?.into();
        }
    }
    Ok(decoded == answers)
}

pub fn hydrate_item(conn: &Connection, kind: &mut artisan_api::ItemKind) -> Result<()> {
    use artisan_api::{Collection, ItemKind, ToolKind};
    fn hydrate<T: DeserializeOwned>(conn: &Connection, value: &mut Collection<T>) -> Result<()> {
        if let Collection::Stored(_) = value {
            *value = Collection::Inline(resolve(conn, value)?);
        }
        Ok(())
    }
    match kind {
        ItemKind::Plan { steps, .. } => hydrate(conn, steps)?,
        ItemKind::Tool {
            tool: ToolKind::Edit { changes },
            ..
        } => hydrate(conn, changes)?,
        ItemKind::Question {
            questions, answers, ..
        } => {
            hydrate(conn, questions)?;
            let Collection::Inline(questions) = questions else {
                unreachable!("successful hydration produces inline records")
            };
            for question in questions {
                hydrate(conn, &mut question.options)?;
            }
            if let Some(answers) = answers {
                hydrate(conn, answers)?;
                let Collection::Inline(answers) = answers else {
                    unreachable!("successful hydration produces inline records")
                };
                for answer in answers.iter_mut().flatten() {
                    if let artisan_api::QuestionAnswer::Choices { ids } = answer {
                        hydrate(conn, ids)?;
                    }
                }
            }
        }
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::{PlanStep, StepStatus, TextContent};
    fn database() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::schema::migrate(&mut conn).unwrap();
        conn
    }
    fn steps(count: usize) -> Vec<PlanStep> {
        (0..count)
            .map(|index| PlanStep {
                text: format!("Step {index}: αβ🙂").into(),
                status: StepStatus::Pending,
            })
            .collect()
    }
    #[test]
    fn five_thousand_records_use_indexed_bounded_ranges_and_keep_order() {
        let conn = database();
        let original = steps(5000);
        let reference = store(&conn, &original, &BTreeSet::new(), 0, 0).unwrap();
        assert_eq!(reference.count, 5000);
        assert!(serde_json::to_string(&reference).unwrap().len() < 256);
        let duplicate = store(&conn, &original, &BTreeSet::new(), 0, 0).unwrap();
        assert_eq!(reference, duplicate);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content_records", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            5000
        );
        let mut actual = Vec::new();
        let mut offset = 0;
        while offset < reference.count {
            let page = range::<PlanStep>(
                &conn,
                &reference.source.id,
                CollectionQuery {
                    offset,
                    limit: None,
                },
            )
            .unwrap()
            .unwrap();
            assert_eq!(page.start, offset);
            assert_eq!(page.end - offset, page.records.len() as u64);
            assert!(page.records.len() <= 64);
            offset = page.end;
            actual.extend(page.records);
        }
        assert_eq!(actual, original);
        let last = range::<PlanStep>(
            &conn,
            &reference.source.id,
            CollectionQuery {
                offset: 4999,
                limit: Some(1),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(last.records, original[4999..]);
        assert!(
            range::<PlanStep>(
                &conn,
                &reference.source.id,
                CollectionQuery {
                    offset: 5001,
                    limit: None
                }
            )
            .is_err()
        );
        assert!(
            range::<PlanStep>(
                &conn,
                &reference.source.id,
                CollectionQuery {
                    offset: 0,
                    limit: Some(65)
                }
            )
            .is_err()
        );
    }
    #[test]
    fn byte_budget_bounds_a_page_even_when_count_limit_is_not_reached() {
        let conn = database();
        let original: Vec<_> = (0..4000)
            .map(|index| PlanStep {
                text: format!("{index}{}", "α".repeat(6000)).into(),
                status: StepStatus::Pending,
            })
            .collect();
        let reference = store(&conn, &original, &BTreeSet::new(), 0, 0).unwrap();
        let page = range::<PlanStep>(&conn, &reference.source.id, CollectionQuery::default())
            .unwrap()
            .unwrap();
        assert!(page.records.len() < 64);
        assert_eq!(page.records, original[..page.records.len()]);
        let encoded: usize = page
            .records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap().len())
            .sum();
        assert!(encoded <= artisan_api::COLLECTION_RANGE_BYTES);
    }
    #[test]
    fn nested_sources_survive_item_updates_then_collect_after_parent_grace() {
        let conn = database();
        let mut text = TextContent::Inline("a large step\n".repeat(3000));
        crate::content::normalize_text(&conn, &mut text).unwrap();
        let TextContent::Stored(child_source) = &text else {
            panic!("large text must be stored")
        };
        let child_bytes = child_source.bytes;
        let child = child_source.id.clone();
        let dependencies = BTreeSet::from([child]);
        let parent = store(
            &conn,
            &[PlanStep {
                text,
                status: StepStatus::Pending,
            }],
            &dependencies,
            child_bytes,
            0,
        )
        .unwrap();
        conn.execute(
            "INSERT INTO item_content(item_id,content_id) VALUES('owner',?1)",
            [parent.source.id.as_str()],
        )
        .unwrap();
        assert_eq!(crate::content::collect(&conn, i64::MAX).unwrap(), 0);
        conn.execute("DELETE FROM item_content WHERE item_id='owner'", [])
            .unwrap();
        assert_eq!(
            crate::content::collect(&conn, artisan_api::now()).unwrap(),
            0
        );
        assert_eq!(crate::content::collect(&conn, i64::MAX).unwrap(), 1);
        // Deleting the parent marks its last nested owner as unreferenced now,
        // so in-flight windows retain the child for the same full-day grace.
        assert_eq!(
            crate::content::collect(&conn, artisan_api::now()).unwrap(),
            0
        );
        assert_eq!(crate::content::collect(&conn, i64::MAX).unwrap(), 1);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content_records", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
    #[test]
    fn rollback_and_missing_index_cannot_publish_partial_collections() {
        let mut conn = database();
        {
            let tx = conn.transaction().unwrap();
            store(&tx, &steps(5000), &BTreeSet::new(), 0, 0).unwrap();
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content_records", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        let reference = store(&conn, &steps(5000), &BTreeSet::new(), 0, 0).unwrap();
        conn.execute(
            "DELETE FROM content_records WHERE content_id=?1 AND ordinal=4000",
            [reference.source.id.as_str()],
        )
        .unwrap();
        assert!(
            range::<PlanStep>(
                &conn,
                &reference.source.id,
                CollectionQuery {
                    offset: 3999,
                    limit: Some(4)
                }
            )
            .is_err()
        );
    }
    #[test]
    fn selected_option_lookup_uses_opaque_keys_and_preserves_duplicate_labels() {
        let conn = database();
        let options = (0..5000)
            .map(|index| artisan_api::QuestionOption {
                id: format!("opaque-{index}"),
                label: "Same".into(),
                description: None,
            })
            .collect::<Vec<_>>();
        let reference = store(&conn, &options, &BTreeSet::new(), 0, 0).unwrap();
        let question = artisan_api::Question {
            id: "question".into(),
            header: None,
            text: "Pick".into(),
            multi_select: false,
            options: artisan_api::Collection::Stored(reference),
        };
        let answer = artisan_api::QuestionAnswer::Choices {
            ids: vec!["opaque-4999".into()].into(),
        };
        validate_answer(&conn, &question, &Some(answer.clone())).unwrap();
        assert_eq!(
            answer_text(&conn, &question, &answer).unwrap(),
            "Same (option 5000 of 5000)"
        );
        assert!(
            validate_answer(
                &conn,
                &question,
                &Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["Same".into()].into()
                })
            )
            .is_err()
        );
        assert!(
            validate_answer(
                &conn,
                &question,
                &Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["opaque-1".into(), "opaque-1".into()].into()
                })
            )
            .is_err()
        );
    }
    #[test]
    fn five_thousand_saved_answer_records_and_choice_ids_keep_nested_ownership() {
        let conn = database();
        let mut kind = artisan_api::ItemKind::Question {
            blocking: false,
            answer_text: None,
            questions: vec![artisan_api::Question {
                id: "q".into(),
                header: None,
                text: "Choose".into(),
                multi_select: true,
                options: vec![].into(),
            }]
            .into(),
            answers: Some(
                vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: (0..5000)
                        .map(|index| format!("id-{index}"))
                        .collect::<Vec<_>>()
                        .into(),
                })]
                .into(),
            ),
        };
        normalize_item(&conn, &mut kind).unwrap();
        let artisan_api::ItemKind::Question {
            answers: Some(artisan_api::Collection::Inline(answers)),
            ..
        } = &kind
        else {
            panic!("small answer collection should stay inline")
        };
        let Some(artisan_api::QuestionAnswer::Choices {
            ids: artisan_api::Collection::Stored(ids),
        }) = &answers[0]
        else {
            panic!("many choices must use an indexed source")
        };
        assert_eq!(ids.count, 5000);
        let ordinal = conn
            .query_row(
                "SELECT ordinal FROM content_records WHERE content_id=?1 AND record_key=?2",
                params![ids.source.id.as_str(), "id-4999"],
                |row| row.get::<_, u64>(0),
            )
            .unwrap();
        assert_eq!(ordinal, 4999);
        let page = range::<String>(
            &conn,
            &ids.source.id,
            artisan_api::CollectionQuery {
                offset: ordinal,
                limit: Some(1),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(page.records, ["id-4999"]);
        let mut kind = artisan_api::ItemKind::Question {
            blocking: false,
            answer_text: None,
            questions: vec![].into(),
            answers: Some(vec![None; 5000].into()),
        };
        normalize_item(&conn, &mut kind).unwrap();
        let artisan_api::ItemKind::Question {
            answers: Some(artisan_api::Collection::Stored(answers)),
            ..
        } = kind
        else {
            panic!("large saved answer arrays must use a source")
        };
        assert_eq!(answers.count, 5000);
        let page = range::<Option<artisan_api::QuestionAnswer>>(
            &conn,
            &answers.source.id,
            CollectionQuery {
                offset: 4999,
                limit: Some(1),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(page.records, vec![None]);
    }
    #[test]
    fn missing_final_index_record_cannot_return_a_short_successful_page() {
        let conn = database();
        let reference = store(&conn, &steps(5000), &BTreeSet::new(), 0, 0).unwrap();
        conn.execute(
            "DELETE FROM content_records WHERE content_id=?1 AND ordinal=4999",
            [reference.source.id.as_str()],
        )
        .unwrap();
        assert!(
            range::<PlanStep>(
                &conn,
                &reference.source.id,
                CollectionQuery {
                    offset: 4990,
                    limit: Some(64)
                }
            )
            .is_err()
        );
    }
    #[test]
    fn canonical_answer_display_is_derived_once_and_free_text_stays_literal() {
        let conn = database();
        let options = (0..5000)
            .map(|index| artisan_api::QuestionOption {
                id: format!("opaque-{index}"),
                label: "Same".into(),
                description: None,
            })
            .collect::<Vec<_>>();
        let reference = store(&conn, &options, &BTreeSet::new(), 0, 0).unwrap();
        let mut kind = artisan_api::ItemKind::Question {
            blocking: false,
            questions: vec![artisan_api::Question {
                id: "q".into(),
                header: None,
                text: "Pick".into(),
                multi_select: false,
                options: artisan_api::Collection::Stored(reference.clone()),
            }]
            .into(),
            answers: Some(
                vec![Some(artisan_api::QuestionAnswer::Choices {
                    ids: vec!["opaque-4999".into()].into(),
                })]
                .into(),
            ),
            answer_text: None,
        };
        derive_answer_text(&conn, &mut kind, None).unwrap();
        let expected = artisan_api::TextContent::from("Same (option 5000 of 5000)");
        assert!(
            matches!(&kind,artisan_api::ItemKind::Question {answer_text:Some(text),..} if text==&expected)
        );
        let old = kind.clone();
        let artisan_api::ItemKind::Question { answer_text, .. } = &mut kind else {
            unreachable!()
        };
        *answer_text = None;
        conn.execute(
            "UPDATE content_records SET record_label='Changed' WHERE content_id=?1",
            [reference.source.id.as_str()],
        )
        .unwrap();
        derive_answer_text(&conn, &mut kind, Some(&old)).unwrap();
        assert!(
            matches!(&kind,artisan_api::ItemKind::Question {answer_text:Some(text),..} if text==&expected)
        );
        let literal = " \n**still literal** \n";
        let mut kind = artisan_api::ItemKind::Question {
            blocking: false,
            questions: vec![artisan_api::Question {
                id: "free".into(),
                header: None,
                text: "Explain".into(),
                multi_select: false,
                options: vec![].into(),
            }]
            .into(),
            answers: Some(
                vec![Some(artisan_api::QuestionAnswer::Text {
                    text: literal.into(),
                })]
                .into(),
            ),
            answer_text: None,
        };
        derive_answer_text(&conn, &mut kind, None).unwrap();
        assert!(
            matches!(&kind,artisan_api::ItemKind::Question {answer_text:Some(text),..} if text.inline()==Some(literal))
        );
    }
    #[test]
    fn duplicate_opaque_record_keys_roll_back_with_the_item_transaction() {
        let mut conn = database();
        {
            let tx = conn.transaction().unwrap();
            let duplicate = artisan_api::QuestionOption {
                id: "same".into(),
                label: "Choice".into(),
                description: None,
            };
            assert!(store(&tx, &[duplicate.clone(), duplicate], &BTreeSet::new(), 0, 0).is_err());
        }
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM content_records", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            0
        );
    }
}
