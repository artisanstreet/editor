use artisan_api::{Collection, Item, ItemId, ItemKind, Phase, TextContent, ToolKind};
use forge_store::Store;
use std::fmt::Write as _;

const CLIPPED: usize = 4_000;
const WHOLE: usize = 64 * 1024;

const STRING_WRITE: &str = "invariant: writing to a String cannot fail";

#[derive(Debug, thiserror::Error)]
#[error("resolve the stored text")]
pub struct ResolveError(#[source] forge_store::ResolveTextError);

#[derive(Debug, thiserror::Error)]
#[error("render item {item}")]
pub struct RenderError {
    item: ItemId,
    #[source]
    source: ResolveError,
}

type Result<T> = std::result::Result<T, ResolveError>;

pub struct Options {
    pub whole: bool,
    pub output: bool,
    pub unbounded: bool,
}

pub async fn render(
    store: &Store,
    items: &[Item],
    options: &Options,
) -> std::result::Result<String, RenderError> {
    let mut out = String::new();
    for item in items {
        out.push_str(
            &block(store, item, options)
                .await
                .map_err(|source| RenderError {
                    item: item.id.clone(),
                    source,
                })?,
        );
        out.push('\n');
    }
    Ok(out)
}

async fn text(store: &Store, text: &TextContent, options: &Options) -> Result<String> {
    let resolved = store.resolve_text(text).await.map_err(ResolveError)?;
    Ok(clip(
        &resolved,
        match (options.unbounded, options.whole) {
            (true, _) => usize::MAX,
            (_, true) => WHOLE,
            _ => CLIPPED,
        },
    ))
}

fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n_{} more bytes; use read_thread_item with its item ID for byte-range continuation._",
        &text[..end],
        text.len() - end
    )
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

#[expect(clippy::too_many_lines, reason = "one transcript block per item kind")]
async fn block(store: &Store, item: &Item, options: &Options) -> Result<String> {
    let status = status_suffix(item.status);
    let head = |kind: &str| {
        format!(
            "**{kind}** · {}{status} `{}`\n\n",
            time(item.created_at),
            item.id
        )
    };
    Ok(match &item.kind {
        ItemKind::UserMessage { text: body, .. } => {
            format!("{}{}\n", head("You"), text(store, body, options).await?)
        }
        ItemKind::AssistantMessage { text: body, phase } => {
            let kind = match phase {
                Phase::Final => "Reply",
                Phase::Commentary => "Working note",
            };
            format!("{}{}\n", head(kind), text(store, body, options).await?)
        }
        ItemKind::Tool {
            tool,
            title,
            output,
            exit_code,
            ..
        } => {
            tool_block(
                store,
                head("Tool"),
                tool,
                title.as_deref(),
                output.as_ref(),
                *exit_code,
                options,
            )
            .await?
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
            let mut out = head("Agent");
            out.push_str(&agent_line(
                label.as_deref(),
                description,
                *engine,
                model.as_deref(),
                thread.as_ref(),
            ));
            if let Some(result) = result {
                writeln!(out, "result:\n{}", text(store, result, options).await?)
                    .expect(STRING_WRITE);
            }
            out
        }
        ItemKind::Question {
            questions,
            answer_text,
            ..
        } => {
            question_block(
                store,
                head("Question"),
                questions,
                answer_text.as_ref(),
                options,
            )
            .await?
        }
        ItemKind::Plan {
            steps, proposal, ..
        } => plan_block(store, head("Plan"), steps, proposal.as_ref(), options).await?,
        ItemKind::Image { caption, .. } => format!(
            "{}{}\n",
            head("Image"),
            match caption {
                Some(caption) => text(store, caption, options).await?,
                None => String::new(),
            }
        ),
        ItemKind::Compaction { summary, .. } => format!(
            "{}{}\n",
            head("Compacted"),
            match summary {
                Some(summary) => text(store, summary, options).await?,
                None => String::new(),
            }
        ),
        ItemKind::Notice { text: body, .. } => {
            format!("{}{}\n", head("Note"), text(store, body, options).await?)
        }
        ItemKind::Error { message, .. } => {
            format!(
                "{}{}\n",
                head("Error"),
                text(store, message, options).await?
            )
        }
    })
}

async fn tool_block(
    store: &Store,
    mut out: String,
    tool: &ToolKind,
    title: Option<&str>,
    output: Option<&artisan_api::Output>,
    exit_code: Option<i32>,
    options: &Options,
) -> Result<String> {
    if let Some(title) = title {
        writeln!(out, "{title}").expect(STRING_WRITE);
    }
    out.push_str(&tool_summary(store, tool, options).await?);
    if let Some(code) = exit_code {
        writeln!(out, "exit code {code}").expect(STRING_WRITE);
    }
    let failed = exit_code.is_some_and(|code| code != 0);
    if let Some(output) = output
        && (options.output || failed)
    {
        writeln!(
            out,
            "output:\n{}",
            text(store, &output.text, options).await?
        )
        .expect(STRING_WRITE);
    } else if output.is_some() {
        out.push_str("_Output left out; include_output=true shows it._\n");
    }
    Ok(out)
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

async fn question_block(
    store: &Store,
    mut out: String,
    questions: &Collection<artisan_api::Question>,
    answer_text: Option<&TextContent>,
    options: &Options,
) -> Result<String> {
    if let Collection::Inline(questions) = questions {
        for question in questions {
            writeln!(out, "? {}", text(store, &question.text, options).await?).expect(STRING_WRITE);
        }
    }
    if let Some(answer) = answer_text {
        writeln!(out, "answer: {}", text(store, answer, options).await?).expect(STRING_WRITE);
    }
    Ok(out)
}

async fn plan_block(
    store: &Store,
    mut out: String,
    steps: &Collection<artisan_api::PlanStep>,
    proposal: Option<&TextContent>,
    options: &Options,
) -> Result<String> {
    if let Some(proposal) = proposal {
        writeln!(out, "{}", text(store, proposal, options).await?).expect(STRING_WRITE);
    }
    if let Collection::Inline(steps) = steps {
        for step in steps {
            writeln!(
                out,
                "- [{:?}] {}",
                step.status,
                text(store, &step.text, options).await?
            )
            .expect(STRING_WRITE);
        }
    }
    Ok(out)
}

async fn tool_summary(store: &Store, tool: &ToolKind, options: &Options) -> Result<String> {
    Ok(match tool {
        ToolKind::Shell { command, .. } => {
            format!("$ {}\n", text(store, command, options).await?)
        }
        ToolKind::Read { path, .. } => format!("read {path}\n"),
        ToolKind::ViewImage { path } => format!("viewed {path}\n"),
        ToolKind::Edit { changes } => {
            let paths: Vec<String> = match changes {
                Collection::Inline(changes) => {
                    changes.iter().map(|change| change.path.clone()).collect()
                }
                Collection::Stored(stored) => vec![format!("{} files", stored.count)],
            };
            format!("edited {}\n", paths.join(", "))
        }
        ToolKind::Search { pattern, path, .. } => format!(
            "searched {pattern}{}\n",
            path.as_deref()
                .map(|path| format!(" in {path}"))
                .unwrap_or_default()
        ),
        ToolKind::WebFetch { url } => format!("fetched {url}\n"),
        ToolKind::WebSearch { query } => format!("searched the web for {query}\n"),
        ToolKind::Mcp {
            server,
            tool,
            input,
        } => format!(
            "{server}.{tool}{}\n",
            match input {
                Some(input) => format!(" {}", text(store, input, options).await?),
                None => String::new(),
            }
        ),
        ToolKind::Workflow { name, input } => format!(
            "Workflow{}{}\n",
            name.as_ref()
                .map_or_else(String::new, |name| format!(" {name}")),
            match input {
                Some(input) => format!(" {}", text(store, input, options).await?),
                None => String::new(),
            }
        ),
        ToolKind::Other { name, input } => format!(
            "{name}{}\n",
            match input {
                Some(input) => format!(" {}", text(store, input, options).await?),
                None => String::new(),
            }
        ),
        ToolKind::AgentMessage {
            recipient,
            text: body,
            ..
        } => {
            format!(
                "message to {recipient}: {}\n",
                text(store, body, options).await?
            )
        }
        ToolKind::Agents {
            action, addresses, ..
        } => format!("{action:?} agents {}\n", addresses.join(", ")).to_lowercase(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn times_read_as_utc_dates() {
        assert_eq!(super::time(0), "1970-01-01 00:00 UTC");
        assert_eq!(super::time(1_792_066_918_056), "2026-10-15 12:21 UTC");
    }
}
