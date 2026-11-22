use crate::content::{ContentError, Damage, to_usize};
use crate::error::AtStep as _;
use artisan_api::{CollectionQuery, CollectionRange, CollectionRef, ContentId, ContentRef};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::ops::ControlFlow;

pub const MAX_COLLECTION_BYTES: u64 = 64 * 1024 * 1024;

type Result<T, E = ContentError> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum AnswerRefused {
    #[error("{0}")]
    Invalid(&'static str),
    #[error("submitted choices must be inline")]
    StoredChoices,
    #[error("unknown question choice")]
    UnknownChoice,
    #[error("answer count does not match questions")]
    CountMismatch,
    #[error("This question needs its engine, which has stopped.")]
    EngineStopped,
    #[error("This question was already answered differently.")]
    AnsweredDifferently,
    #[error("This question is not waiting for an answer.")]
    NotWaiting,
    #[error("Item {item} is not a question.")]
    NotAQuestion { item: artisan_api::ItemId },
}

fn damaged(content: &ContentId, damage: Damage) -> ContentError {
    ContentError::Damaged {
        content: content.clone(),
        damage,
    }
}

fn missing(content: &ContentId) -> ContentError {
    ContentError::Missing {
        content: content.clone(),
    }
}

fn written(result: std::fmt::Result) {
    result.expect("invariant: writing to a String cannot fail");
}

fn encode<T: Serialize>(record: &T) -> Result<String> {
    let json = crate::read::to_body(record)?;
    if json.len() > forge_content::INLINE_COLLECTION_BYTES {
        return Err(ContentError::RecordTooLarge { bytes: json.len() });
    }
    Ok(json)
}

fn record_key(encoded: &str) -> Result<(Option<String>, Option<String>)> {
    let value: serde_json::Value = crate::read::from_body(encoded)?;
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
    if !key
        .as_ref()
        .is_none_or(|key| !key.is_empty() && key.len() <= 4096)
    {
        return Err(ContentError::RecordIdentity);
    }
    let label = value
        .get("label")
        .and_then(|label| label.as_str())
        .map(str::to_owned);
    Ok((key, label))
}

const CONTENT_EXISTS: &str = include_str!("sql/content/exists.sql");
const INSERT_CONTENT: &str = include_str!("sql/collection/insert_content.sql");
const INSERT_COLLECTION: &str = include_str!("sql/collection/insert.sql");
const INSERT_RECORD: &str = include_str!("sql/collection/insert_record.sql");
const INSERT_CHUNK: &str = include_str!("sql/content/insert_chunk.sql");
const RECORD_COUNT: &str = include_str!("sql/collection/count.sql");
const INSERT_DEPENDENCY: &str = include_str!("sql/content/insert_dependency.sql");

fn check_dependencies(
    conn: &Connection,
    source: &ContentId,
    dependencies: &BTreeSet<ContentId>,
) -> Result<()> {
    for child in dependencies {
        if child == source {
            return Err(ContentError::SelfDependency {
                content: child.clone(),
            });
        }
        let exists: bool = conn
            .query_row(CONTENT_EXISTS, [child.as_str()], |row| row.get(0))
            .at("content/exists.sql")?;
        if !exists {
            return Err(ContentError::MissingDependency {
                content: child.clone(),
            });
        }
    }
    Ok(())
}

fn record_dependencies(
    conn: &Connection,
    parent: &ContentId,
    dependencies: &BTreeSet<ContentId>,
) -> Result<()> {
    for child in dependencies {
        conn.execute(INSERT_DEPENDENCY, params![parent.as_str(), child.as_str()])
            .at("content/insert_dependency.sql")?;
    }
    Ok(())
}

fn store<T: Serialize>(
    conn: &crate::content::Stamped<'_>,
    records: &[T],
    dependencies: &BTreeSet<ContentId>,
    authored_bytes: u64,
    nested_structural_bytes: u64,
) -> Result<CollectionRef> {
    let reference = forge_content::collection_ref(records)
        .map_err(crate::error::StorageError::encode::<[T]>)?;
    let source = reference.source;
    let bytes = source
        .bytes
        .saturating_add(authored_bytes)
        .saturating_add(nested_structural_bytes);
    if bytes > MAX_COLLECTION_BYTES {
        return Err(ContentError::CollectionTooLarge { bytes });
    }
    check_dependencies(conn, &source.id, dependencies)?;
    let inserted = conn
        .execute(
            INSERT_CONTENT,
            params![source.id.as_str(), source.bytes, source.lines, conn.now],
        )
        .at("collection/insert_content.sql")?
        != 0;
    let indexed = conn
        .execute(
            INSERT_COLLECTION,
            params![
                source.id.as_str(),
                records.len() as u64,
                authored_bytes,
                source.bytes.saturating_add(nested_structural_bytes)
            ],
        )
        .at("collection/insert.sql")?
        != 0;
    if inserted || indexed {
        let mut insert_record = conn
            .prepare_cached(INSERT_RECORD)
            .at("collection/insert_record.sql")?;
        let mut insert_chunk = conn
            .prepare_cached(INSERT_CHUNK)
            .at("content/insert_chunk.sql")?;
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
                insert_record
                    .execute(params![
                        source.id.as_str(),
                        ordinal as u64,
                        position,
                        end,
                        key,
                        label
                    ])
                    .at("collection/insert_record.sql")?;
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
                insert_chunk
                    .execute(params![
                        source.id.as_str(),
                        chunk_start,
                        &buffer.as_bytes()[..boundary]
                    ])
                    .at("content/insert_chunk.sql")?;
                buffer.drain(..boundary);
                chunk_start += boundary as u64;
            }
        }
        if !buffer.is_empty() {
            insert_chunk
                .execute(params![source.id.as_str(), chunk_start, buffer.as_bytes()])
                .at("content/insert_chunk.sql")?;
        }
    }
    let known: u64 = conn
        .query_row(RECORD_COUNT, [source.id.as_str()], |row| row.get(0))
        .at("collection/count.sql")?;
    if known != records.len() as u64 {
        return Err(damaged(&source.id, Damage::DescriptorMismatch));
    }
    record_dependencies(conn, &source.id, dependencies)?;
    Ok(CollectionRef {
        source,
        count: records.len() as u64,
    })
}

pub(crate) fn range<T: DeserializeOwned>(
    conn: &Connection,
    id: &ContentId,
    query: CollectionQuery,
) -> Result<Option<CollectionRange<T>>> {
    if !id.is_valid() {
        return Err(ContentError::InvalidIdentity {
            content: id.clone(),
        });
    }
    let limit = query.limit.unwrap_or(artisan_api::COLLECTION_RANGE_ENTRIES);
    if !(1..=artisan_api::COLLECTION_RANGE_ENTRIES).contains(&limit) {
        return Err(ContentError::CollectionLimit { limit });
    }
    let Some((bytes, lines, total)) = conn
        .query_row(
            include_str!("sql/collection/header.sql"),
            [id.as_str()],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            },
        )
        .optional()
        .at("collection/header.sql")?
    else {
        return Ok(None);
    };
    if query.offset > total {
        return Err(ContentError::RecordOffsetPastEnd {
            content: id.clone(),
            offset: query.offset,
            total,
        });
    }
    let mut select = conn
        .prepare_cached(include_str!("sql/collection/records.sql"))
        .at("collection/records.sql")?;
    let mut rows = select
        .query(params![id.as_str(), query.offset, limit])
        .at("collection/records.sql")?;
    let mut records = Vec::new();
    let mut loaded_bytes = 0usize;
    let mut byte_limited = false;
    while let Some(row) = rows.next().at("collection/records.sql")? {
        let ordinal: u64 = row.get(0).at("collection/records.sql")?;
        let start: u64 = row.get(1).at("collection/records.sql")?;
        let end: u64 = row.get(2).at("collection/records.sql")?;
        if !(ordinal == query.offset + records.len() as u64 && end >= start && end <= bytes) {
            return Err(damaged(id, Damage::RecordIndex));
        }
        let size = to_usize(end - start);
        if size > forge_content::INLINE_COLLECTION_BYTES {
            return Err(damaged(id, Damage::OversizedRecord));
        }
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
        .ok_or_else(|| missing(id))?;
        if !(source.start == start && source.end >= end) {
            return Err(damaged(id, Damage::IncompleteRecord));
        }
        let record = source
            .text
            .get(..size)
            .ok_or_else(|| damaged(id, Damage::IncompleteRecord))?;
        records.push(crate::read::from_body::<T>(record)?);
        loaded_bytes += size;
    }
    if query.offset != total && records.is_empty() {
        return Err(damaged(id, Damage::RecordIndex));
    }
    if !(byte_limited
        || query.offset + records.len() as u64
            == query.offset.saturating_add(u64::from(limit)).min(total))
    {
        return Err(damaged(id, Damage::MissingTrailingRecord));
    }
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

fn validate_answer(
    conn: &Connection,
    question: &artisan_api::Question,
    answer: Option<&artisan_api::QuestionAnswer>,
) -> Result<()> {
    question
        .validate_answer_structure(answer)
        .map_err(AnswerRefused::Invalid)?;
    if let Some(artisan_api::QuestionAnswer::Choices { ids }) = answer {
        let ids = ids.inline().ok_or(AnswerRefused::StoredChoices)?;
        match &question.options {
            artisan_api::Collection::Inline(_) => {
                question
                    .validate_answer(answer)
                    .map_err(AnswerRefused::Invalid)?;
            }
            artisan_api::Collection::Stored(reference) => {
                metrics(conn, reference)?;
                let mut exists = conn
                    .prepare_cached(include_str!("sql/collection/key_exists.sql"))
                    .at("collection/key_exists.sql")?;
                for id in ids {
                    let known: bool = exists
                        .query_row(params![reference.source.id.as_str(), id], |row| row.get(0))
                        .at("collection/key_exists.sql")?;
                    if !known {
                        return Err(AnswerRefused::UnknownChoice.into());
                    }
                }
            }
        }
    }
    Ok(())
}
pub(crate) fn answer_text(
    conn: &Connection,
    question: &artisan_api::Question,
    answer: &artisan_api::QuestionAnswer,
) -> Result<String> {
    let artisan_api::Collection::Stored(reference) = &question.options else {
        return Ok(question
            .answer_text(answer)
            .map_err(AnswerRefused::Invalid)?);
    };
    let ids = match answer {
        artisan_api::QuestionAnswer::Choices { ids } => ids,
        artisan_api::QuestionAnswer::Text { text } => return Ok(text.clone()),
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
    let mut option = conn
        .prepare_cached(include_str!("sql/collection/record_by_key.sql"))
        .at("collection/record_by_key.sql")?;
    let mut count = conn
        .prepare_cached(include_str!("sql/collection/label_count.sql"))
        .at("collection/label_count.sql")?;
    for id in ids {
        let (index, label): (u64, String) = option
            .query_row(params![reference.source.id.as_str(), id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .optional()
            .at("collection/label_count.sql")?
            .ok_or(AnswerRefused::UnknownChoice)?;
        let duplicates = if let Some(value) = duplicate_counts.get(&label) {
            *value
        } else {
            let value: u64 = count
                .query_row(params![reference.source.id.as_str(), label], |row| {
                    row.get(0)
                })
                .at("collection/label_count.sql")?;
            duplicate_counts.insert(label.clone(), value);
            value
        };
        if !labels.is_empty() {
            labels.push_str(", ");
        }
        labels.push_str(&label);
        if duplicates > 1 {
            written(write!(
                labels,
                " (option {} of {})",
                index + 1,
                reference.count
            ));
        }
        if labels.len() as u64 > MAX_COLLECTION_BYTES {
            return Err(ContentError::AnswerDisplayTooLarge);
        }
    }
    Ok(labels)
}

pub(crate) fn derive_answer_text(
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
    if answers.len() != questions.len() {
        return Err(ContentError::SavedAnswerCount {
            questions: questions.len(),
            answers: answers.len(),
        });
    }
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
                        .ok_or_else(|| missing(&reference.source.id))?,
                    );
                }
                let page = cached
                    .as_ref()
                    .expect("invariant: the saved answer page was just loaded");
                &page.records[to_usize(index64 - page.start)]
            }
        };
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        if questions.len() > 1 {
            written(write!(text, "Question {}: ", index + 1));
        }
        match answer {
            Some(answer) => text.push_str(&answer_text(conn, question, answer)?),
            None => text.push_str("You skipped this question."),
        }
        if text.len() as u64 > MAX_COLLECTION_BYTES {
            return Err(ContentError::AnswerDisplayTooLarge);
        }
        Ok(())
    })?;
    *display = Some(text.into());
    Ok(())
}

#[derive(Default, Clone, Copy)]
pub(crate) struct Metrics {
    pub authored: u64,
    pub structural: u64,
}
fn metrics(conn: &Connection, reference: &CollectionRef) -> Result<Metrics> {
    if !reference.source.id.is_valid() {
        return Err(ContentError::InvalidIdentity {
            content: reference.source.id.clone(),
        });
    }
    let value = conn
        .query_row(
            include_str!("sql/collection/metrics.sql"),
            [reference.source.id.as_str()],
            |row| {
                Ok((
                    row.get::<_, u64>(0)?,
                    row.get::<_, u64>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, u64>(4)?,
                ))
            },
        )
        .optional()
        .at("collection/metrics.sql")?
        .ok_or_else(|| missing(&reference.source.id))?;
    if !(reference.count == value.0
        && reference.source.bytes == value.1
        && reference.source.lines == value.2)
    {
        return Err(ContentError::UnknownReference {
            content: reference.source.id.clone(),
        });
    }
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
    conn: &crate::content::Stamped<'_>,
    collection: &mut artisan_api::Collection<T>,
    mut record_metrics: impl FnMut(
        &crate::content::Stamped<'_>,
        &mut T,
        &mut BTreeSet<ContentId>,
    ) -> Result<Metrics>,
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
        crate::content::validate_metadata(&crate::read::from_body(&encoded)?, "")?;
        encoded_bytes = encoded_bytes.saturating_add(encoded.len() as u64 + 1);
        let bytes = total
            .authored
            .saturating_add(total.structural)
            .saturating_add(encoded_bytes);
        if bytes > MAX_COLLECTION_BYTES {
            return Err(ContentError::CollectionTooLarge { bytes });
        }
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
    conn: &crate::content::Stamped<'_>,
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
pub(crate) fn normalize_item(
    conn: &crate::content::Stamped<'_>,
    kind: &mut artisan_api::ItemKind,
) -> Result<()> {
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
pub(crate) fn source_metrics(conn: &Connection, source: &ContentRef) -> Result<Metrics> {
    let Some(count) = conn
        .query_row(
            include_str!("sql/collection/count.sql"),
            [source.id.as_str()],
            |row| row.get::<_, u64>(0),
        )
        .optional()
        .at("collection/count.sql")?
    else {
        return Err(missing(&source.id));
    };
    metrics(
        conn,
        &CollectionRef {
            source: source.clone(),
            count,
        },
    )
}

pub(crate) fn resolve<T: DeserializeOwned>(
    conn: &Connection,
    collection: &artisan_api::Collection<T>,
) -> Result<Vec<T>> {
    let reference = collection
        .stored()
        .expect("invariant: only stored collections are resolved");
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
        .ok_or_else(|| missing(&reference.source.id))?;
        if !(page.total == reference.count && page.end > offset) {
            return Err(damaged(&reference.source.id, Damage::DescriptorMismatch));
        }
        offset = page.end;
        records.extend(page.records);
    }
    Ok(records)
}

pub(crate) fn visit_until<T: DeserializeOwned>(
    conn: &Connection,
    collection: &artisan_api::Collection<T>,
    mut visit: impl FnMut(&T) -> Result<ControlFlow<()>>,
) -> Result<()> {
    match collection {
        artisan_api::Collection::Inline(records) => {
            for record in records {
                if visit(record)?.is_break() {
                    return Ok(());
                }
            }
        }
        artisan_api::Collection::Stored(reference) => {
            metrics(conn, reference)?;
            let mut offset = 0;
            while offset < reference.count {
                let page = range::<T>(
                    conn,
                    &reference.source.id,
                    CollectionQuery {
                        offset,
                        limit: None,
                    },
                )?
                .ok_or_else(|| missing(&reference.source.id))?;
                if !(page.end > offset && page.total == reference.count) {
                    return Err(damaged(&reference.source.id, Damage::DescriptorMismatch));
                }
                for record in &page.records {
                    if visit(record)?.is_break() {
                        return Ok(());
                    }
                }
                offset = page.end;
            }
        }
    }
    Ok(())
}

pub(crate) fn visit_questions(
    conn: &Connection,
    questions: &artisan_api::Collection<artisan_api::Question>,
    mut visit: impl FnMut(usize, &artisan_api::Question) -> Result<()>,
) -> Result<()> {
    let mut index = 0;
    visit_until(conn, questions, |question| {
        visit(index, question)?;
        index += 1;
        Ok(ControlFlow::Continue(()))
    })
}
pub(crate) fn validate_answers(
    conn: &Connection,
    questions: &artisan_api::Collection<artisan_api::Question>,
    answers: &[Option<artisan_api::QuestionAnswer>],
) -> Result<()> {
    if questions.len() != answers.len() as u64 {
        return Err(AnswerRefused::CountMismatch.into());
    }
    visit_questions(conn, questions, |index, question| {
        validate_answer(conn, question, answers[index].as_ref())
    })
}
pub(crate) fn answers_equal(
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

pub(crate) fn hydrate_item(conn: &Connection, kind: &mut artisan_api::ItemKind) -> Result<()> {
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
#[path = "../tests/collection/unit.rs"]
mod tests;
