use artisan_api::{
    Collection, CollectionQuery, ContentId, ContentQuery, ContentRef, Item, ItemId, ItemKind,
    Phase, TextContent, ToolKind,
};
use forge_store::Store;

const CLIPPED: usize = 4_000;
const WHOLE: usize = 64 * 1024;
const LISTED_ENTRIES: u32 = 50;

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("read a range of stored text")]
    Range(#[from] forge_store::ContentRangeError),
    #[error("read a range of a stored collection")]
    Collection(#[from] forge_store::CollectionRangeError),
    #[error("stored content {id} is gone")]
    Gone { id: ContentId },
    #[error("stored collection {id} stopped at entry {offset}")]
    Stalled { id: ContentId, offset: u64 },
}

#[derive(Debug, thiserror::Error)]
#[error("render item {item}")]
pub struct RenderError {
    item: ItemId,
    #[source]
    source: ResolveError,
}

#[derive(Debug, thiserror::Error)]
pub enum SliceError {
    #[error("offset {offset} is not a UTF-8 boundary of the rendered item")]
    Offset { offset: u64 },
    #[error(transparent)]
    Render(#[from] RenderError),
}

type Result<T> = std::result::Result<T, ResolveError>;

pub struct Options {
    pub whole: bool,
    pub output: bool,
}

#[derive(Clone, Copy)]
enum Mode {
    Clipped(usize),
    Sourced,
}

enum Segment {
    Literal(String),
    Source(ContentRef),
}

struct Doc<'a> {
    store: &'a Store,
    mode: Mode,
    output: bool,
    segments: Vec<Segment>,
}

impl Doc<'_> {
    fn push_str(&mut self, text: &str) {
        if let Some(Segment::Literal(last)) = self.segments.last_mut() {
            last.push_str(text);
        } else {
            self.segments.push(Segment::Literal(text.to_owned()));
        }
    }

    async fn text(&mut self, text: &TextContent) -> Result<()> {
        match (self.mode, text) {
            (Mode::Sourced, TextContent::Stored(source)) => {
                self.segments.push(Segment::Source(source.clone()));
            }
            (Mode::Sourced, TextContent::Inline(text)) => self.push_str(text),
            (Mode::Clipped(limit), TextContent::Inline(text)) => self.push_str(&clip(text, limit)),
            (Mode::Clipped(limit), TextContent::Stored(source)) => {
                let prefix = prefix(self.store, source, limit).await?;
                let omitted = usize::try_from(source.bytes)
                    .unwrap_or(usize::MAX)
                    .saturating_sub(prefix.len());
                self.push_str(&prefix);
                if omitted > 0 {
                    self.push_str(&omission(omitted));
                }
            }
        }
        Ok(())
    }

    async fn line(&mut self, lead: &str, text: &TextContent) -> Result<()> {
        self.push_str(lead);
        self.text(text).await?;
        self.push_str("\n");
        Ok(())
    }
}

async fn prefix(store: &Store, source: &ContentRef, limit: usize) -> Result<String> {
    let wanted = u64::try_from(limit).unwrap_or(u64::MAX).min(source.bytes);
    let mut text = String::new();
    let mut offset = 0;
    while offset < wanted {
        let range = store
            .content_range(
                &source.id,
                ContentQuery {
                    offset,
                    limit: Some(
                        u32::try_from(wanted - offset)
                            .unwrap_or(u32::MAX)
                            .clamp(4, artisan_api::CONTENT_RANGE_BYTES),
                    ),
                },
            )
            .await?
            .ok_or_else(|| ResolveError::Gone {
                id: source.id.clone(),
            })?;
        if range.end <= offset {
            break;
        }
        offset = range.end;
        text.push_str(&range.text);
    }
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    Ok(text)
}

fn omission(omitted: usize) -> String {
    format!(
        "\n\n_{omitted} more bytes; use read_thread_item with its item ID for byte-range continuation._"
    )
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &text[..end], omission(text.len() - end))
}

pub async fn render(
    store: &Store,
    items: &[Item],
    options: &Options,
) -> std::result::Result<String, RenderError> {
    let mut doc = Doc {
        store,
        mode: Mode::Clipped(if options.whole { WHOLE } else { CLIPPED }),
        output: options.output,
        segments: Vec::new(),
    };
    for item in items {
        block(&mut doc, item).await.map_err(|source| RenderError {
            item: item.id.clone(),
            source,
        })?;
        doc.push_str("\n");
    }
    Ok(doc
        .segments
        .into_iter()
        .map(|segment| match segment {
            Segment::Literal(text) => text,
            Segment::Source(source) => {
                unreachable!("clipped renders read stored text {} inline", source.id)
            }
        })
        .collect())
}

pub struct Rendered<'a> {
    store: &'a Store,
    item: ItemId,
    segments: Vec<Segment>,
}

pub struct Slice {
    pub text: String,
    pub end: u64,
}

pub async fn render_item<'a>(
    store: &'a Store,
    item: &Item,
    output: bool,
) -> std::result::Result<Rendered<'a>, RenderError> {
    let mut doc = Doc {
        store,
        mode: Mode::Sourced,
        output,
        segments: Vec::new(),
    };
    block(&mut doc, item).await.map_err(|source| RenderError {
        item: item.id.clone(),
        source,
    })?;
    doc.push_str("\n");
    Ok(Rendered {
        store,
        item: item.id.clone(),
        segments: doc.segments,
    })
}

const fn segment_len(segment: &Segment) -> u64 {
    match segment {
        Segment::Literal(text) => text.len() as u64,
        Segment::Source(source) => source.bytes,
    }
}

impl Rendered<'_> {
    pub fn len(&self) -> u64 {
        self.segments.iter().map(segment_len).sum()
    }

    pub async fn slice(&self, start: u64, limit: u64) -> std::result::Result<Slice, SliceError> {
        let total = self.len();
        if start > total {
            return Err(SliceError::Offset { offset: start });
        }
        let wanted = start.saturating_add(limit.max(1)).min(total);
        let mut text = String::new();
        let mut at = 0u64;
        let mut end = start;
        for segment in &self.segments {
            let len = segment_len(segment);
            let (from, to) = (at, at + len);
            at = to;
            if to <= end || from >= wanted.max(end + 1) {
                continue;
            }
            let local = end - from;
            let room = wanted.saturating_sub(end).max(u64::from(text.is_empty()));
            let piece = self
                .piece(segment, local, room)
                .await?
                .filter(|piece| piece.start == local)
                .ok_or(SliceError::Offset { offset: end })?;
            end = from + piece.end;
            text.push_str(&piece.text);
            if end >= wanted || end < to {
                break;
            }
        }
        Ok(Slice { text, end })
    }

    async fn piece(
        &self,
        segment: &Segment,
        local: u64,
        room: u64,
    ) -> std::result::Result<Option<Piece>, RenderError> {
        match segment {
            Segment::Literal(literal) => {
                let from = usize::try_from(local).expect("invariant: a literal fits in memory");
                if !literal.is_char_boundary(from) {
                    return Ok(None);
                }
                let mut to = from
                    .saturating_add(usize::try_from(room).unwrap_or(usize::MAX))
                    .min(literal.len());
                while !literal.is_char_boundary(to) {
                    to -= 1;
                }
                if to == from && from < literal.len() {
                    to = from
                        + literal[from..]
                            .chars()
                            .next()
                            .expect("invariant: a boundary before the end starts a character")
                            .len_utf8();
                }
                Ok(Some(Piece {
                    start: local,
                    end: to as u64,
                    text: literal[from..to].to_owned(),
                }))
            }
            Segment::Source(source) => {
                let range = self
                    .store
                    .content_range(
                        &source.id,
                        ContentQuery {
                            offset: local,
                            limit: Some(
                                u32::try_from(room)
                                    .unwrap_or(u32::MAX)
                                    .clamp(4, artisan_api::CONTENT_RANGE_BYTES),
                            ),
                        },
                    )
                    .await
                    .map_err(|cause| RenderError {
                        item: self.item.clone(),
                        source: cause.into(),
                    })?
                    .ok_or_else(|| RenderError {
                        item: self.item.clone(),
                        source: ResolveError::Gone {
                            id: source.id.clone(),
                        },
                    })?;
                let text = within(range.text, usize::try_from(room).unwrap_or(usize::MAX));
                Ok(Some(Piece {
                    start: range.start,
                    end: range.start + text.len() as u64,
                    text,
                }))
            }
        }
    }
}

fn within(mut text: String, room: usize) -> String {
    if text.len() <= room {
        return text;
    }
    let mut end = room;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == 0 {
        end = text.chars().next().map_or(0, char::len_utf8);
    }
    text.truncate(end);
    text
}

struct Piece {
    start: u64,
    end: u64,
    text: String,
}

pub fn time(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rest / 3600,
        rest % 3600 / 60
    )
}

fn status_suffix(status: artisan_api::Status) -> String {
    match status {
        artisan_api::Status::Done => String::new(),
        artisan_api::Status::AwaitingUser => " · waiting for the user".to_owned(),
        other @ (artisan_api::Status::Running
        | artisan_api::Status::Failed
        | artisan_api::Status::Declined
        | artisan_api::Status::Cancelled) => format!(" · {}", format!("{other:?}").to_lowercase()),
    }
}

fn head(item: &Item, kind: &str) -> String {
    format!(
        "**{kind}** · {}{} `{}`\n\n",
        time(item.created_at),
        status_suffix(item.status),
        item.id
    )
}

async fn block(doc: &mut Doc<'_>, item: &Item) -> Result<()> {
    match &item.kind {
        ItemKind::UserMessage { text: body, .. } => {
            doc.push_str(&head(item, "You"));
            doc.line("", body).await?;
        }
        ItemKind::AssistantMessage { text: body, phase } => {
            let kind = match phase {
                Phase::Final => "Reply",
                Phase::Commentary => "Working note",
            };
            doc.push_str(&head(item, kind));
            doc.line("", body).await?;
        }
        ItemKind::Tool {
            tool,
            title,
            output,
            exit_code,
            ..
        } => {
            doc.push_str(&head(item, "Tool"));
            tool_block(doc, tool, title.as_deref(), output.as_ref(), *exit_code).await?;
        }
        ItemKind::Agent {
            label,
            description,
            result,
            engine,
            thread,
            model,
            ..
        } => {
            doc.push_str(&head(item, "Agent"));
            doc.push_str(&agent_line(
                label.as_deref(),
                description,
                *engine,
                model.as_deref(),
                thread.as_ref(),
            ));
            if let Some(result) = result {
                doc.line("result:\n", result).await?;
            }
        }
        ItemKind::Question {
            questions,
            answer_text,
            ..
        } => {
            doc.push_str(&head(item, "Question"));
            question_block(doc, questions, answer_text.as_ref()).await?;
        }
        ItemKind::Plan {
            steps, proposal, ..
        } => {
            doc.push_str(&head(item, "Plan"));
            plan_block(doc, steps, proposal.as_ref()).await?;
        }
        ItemKind::Image { caption, .. } => {
            doc.push_str(&head(item, "Image"));
            optional_line(doc, caption.as_ref()).await?;
        }
        ItemKind::Compaction { summary, .. } => {
            doc.push_str(&head(item, "Compacted"));
            optional_line(doc, summary.as_ref()).await?;
        }
        ItemKind::Notice { text: body, .. } => {
            doc.push_str(&head(item, "Note"));
            doc.line("", body).await?;
        }
        ItemKind::Error { message, .. } => {
            doc.push_str(&head(item, "Error"));
            doc.line("", message).await?;
        }
    }
    Ok(())
}

async fn optional_line(doc: &mut Doc<'_>, text: Option<&TextContent>) -> Result<()> {
    if let Some(text) = text {
        doc.line("", text).await
    } else {
        doc.push_str("\n");
        Ok(())
    }
}

async fn tool_block(
    doc: &mut Doc<'_>,
    tool: &ToolKind,
    title: Option<&str>,
    output: Option<&artisan_api::Output>,
    exit_code: Option<i32>,
) -> Result<()> {
    if let Some(title) = title {
        doc.push_str(title);
        doc.push_str("\n");
    }
    tool_summary(doc, tool).await?;
    if let Some(code) = exit_code {
        doc.push_str(&format!("exit code {code}\n"));
    }
    let failed = exit_code.is_some_and(|code| code != 0);
    if let Some(output) = output
        && (doc.output || failed)
    {
        doc.line("output:\n", &output.text).await?;
    } else if output.is_some() {
        doc.push_str("_Output left out; include_output=true shows it._\n");
    }
    Ok(())
}

fn agent_line(
    label: Option<&str>,
    description: &TextContent,
    engine: Option<artisan_api::EngineId>,
    model: Option<&str>,
    thread: Option<&artisan_api::ThreadId>,
) -> String {
    format!(
        "{}{}{}{}\n",
        label.map_or_else(
            || description.inline().unwrap_or_default().to_owned(),
            str::to_owned
        ),
        engine
            .map(|engine| format!(" on {}", engine.display_name()))
            .unwrap_or_default(),
        model.map(|model| format!(" ({model})")).unwrap_or_default(),
        thread
            .map(|thread| format!(", thread {thread}"))
            .unwrap_or_default(),
    )
}

async fn entries<T: serde::de::DeserializeOwned + Clone + Send + Sync + 'static>(
    doc: &Doc<'_>,
    collection: &Collection<T>,
) -> Result<(Vec<T>, u64)> {
    let limit = match doc.mode {
        Mode::Clipped(_) => Some(LISTED_ENTRIES),
        Mode::Sourced => None,
    };
    match collection {
        Collection::Inline(entries) => {
            let shown = limit.map_or(entries.len(), |limit| entries.len().min(limit as usize));
            Ok((entries[..shown].to_vec(), (entries.len() - shown) as u64))
        }
        Collection::Stored(reference) => {
            let mut records = Vec::new();
            let mut offset = 0;
            let wanted = limit.map_or(reference.count, |limit| {
                reference.count.min(u64::from(limit))
            });
            while offset < wanted {
                let page = doc
                    .store
                    .collection_range::<T>(
                        &reference.source.id,
                        CollectionQuery {
                            offset,
                            limit: Some(
                                u32::try_from(wanted - offset)
                                    .unwrap_or(u32::MAX)
                                    .min(artisan_api::COLLECTION_RANGE_ENTRIES),
                            ),
                        },
                    )
                    .await?
                    .ok_or_else(|| ResolveError::Gone {
                        id: reference.source.id.clone(),
                    })?;
                if page.end <= offset {
                    return Err(ResolveError::Stalled {
                        id: reference.source.id.clone(),
                        offset,
                    });
                }
                offset = page.end;
                records.extend(page.records);
            }
            Ok((records, reference.count - offset))
        }
    }
}

fn more_entries(doc: &mut Doc<'_>, more: u64, noun: &str) {
    if more > 0 {
        doc.push_str(&format!(
            "_{more} more {noun}; use read_thread_item with its item ID to read them all._\n"
        ));
    }
}

async fn question_block(
    doc: &mut Doc<'_>,
    questions: &Collection<artisan_api::Question>,
    answer_text: Option<&TextContent>,
) -> Result<()> {
    let (questions, more) = entries(doc, questions).await?;
    for question in &questions {
        doc.line("? ", &question.text).await?;
    }
    more_entries(doc, more, "questions");
    if let Some(answer) = answer_text {
        doc.line("answer: ", answer).await?;
    }
    Ok(())
}

async fn plan_block(
    doc: &mut Doc<'_>,
    steps: &Collection<artisan_api::PlanStep>,
    proposal: Option<&TextContent>,
) -> Result<()> {
    if let Some(proposal) = proposal {
        doc.line("", proposal).await?;
    }
    let (steps, more) = entries(doc, steps).await?;
    for step in &steps {
        doc.line(&format!("- [{:?}] ", step.status), &step.text)
            .await?;
    }
    more_entries(doc, more, "steps");
    Ok(())
}

async fn tool_summary(doc: &mut Doc<'_>, tool: &ToolKind) -> Result<()> {
    match tool {
        ToolKind::Shell { command, .. } => doc.line("$ ", command).await?,
        ToolKind::Read { path, .. } => doc.push_str(&format!("read {path}\n")),
        ToolKind::ViewImage { path } => doc.push_str(&format!("viewed {path}\n")),
        ToolKind::Edit { changes } => {
            let paths: Vec<String> = match changes {
                Collection::Inline(changes) => {
                    changes.iter().map(|change| change.path.clone()).collect()
                }
                Collection::Stored(stored) => vec![format!("{} files", stored.count)],
            };
            doc.push_str(&format!("edited {}\n", paths.join(", ")));
        }
        ToolKind::Search { pattern, path, .. } => doc.push_str(&format!(
            "searched {pattern}{}\n",
            path.as_deref()
                .map(|path| format!(" in {path}"))
                .unwrap_or_default()
        )),
        ToolKind::WebFetch { url } => doc.push_str(&format!("fetched {url}\n")),
        ToolKind::WebSearch { query } => {
            doc.push_str(&format!("searched the web for {query}\n"));
        }
        ToolKind::Mcp {
            server,
            tool,
            input,
        } => named_input(doc, &format!("{server}.{tool}"), input.as_ref()).await?,
        ToolKind::Workflow { name, input } => {
            let name = name
                .as_ref()
                .map_or_else(|| "Workflow".to_owned(), |name| format!("Workflow {name}"));
            named_input(doc, &name, input.as_ref()).await?;
        }
        ToolKind::Other { name, input } => named_input(doc, name, input.as_ref()).await?,
        ToolKind::AgentMessage {
            recipient,
            text: body,
            ..
        } => doc.line(&format!("message to {recipient}: "), body).await?,
        ToolKind::Agents {
            action, addresses, ..
        } => doc.push_str(&format!("{action:?} agents {}\n", addresses.join(", ")).to_lowercase()),
    }
    Ok(())
}

async fn named_input(doc: &mut Doc<'_>, name: &str, input: Option<&TextContent>) -> Result<()> {
    if let Some(input) = input {
        doc.line(&format!("{name} "), input).await
    } else {
        doc.push_str(&format!("{name}\n"));
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/transcript/unit.rs"]
mod tests;
