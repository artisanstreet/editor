use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Recorded,
    Synthetic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    Out {
        text: String,
    },
    In {
        text: String,
    },
    Request {
        method: String,
        path: String,
        reply: Value,
    },
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    source: Source,
    launches: Vec<Vec<Step>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Line {
    Transcript {
        source: Source,
    },
    Launch,
    Out {
        text: String,
    },
    In {
        text: String,
    },
    Request {
        method: String,
        path: String,
        reply: Value,
    },
    Closed,
}

impl From<Step> for Line {
    fn from(step: Step) -> Self {
        match step {
            Step::Out { text } => Self::Out { text },
            Step::In { text } => Self::In { text },
            Step::Request {
                method,
                path,
                reply,
            } => Self::Request {
                method,
                path,
                reply,
            },
            Step::Closed => Self::Closed,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("transcript line {line} is not a transcript record")]
    Line {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("a transcript starts with a `transcript` line naming its source")]
    MissingHeader,
    #[error("transcript line {line} repeats the `transcript` header")]
    RepeatedHeader { line: usize },
    #[error("transcript line {line} is a step before the first `launch`")]
    StepBeforeLaunch { line: usize },
    #[error("the transcript has no launch")]
    NoLaunch,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("trace line {line} is not a trace record")]
    Line {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("trace line {line} was cut to its first bytes of {bytes}, so it cannot be replayed")]
    Truncated { line: usize, bytes: usize },
    #[error("the trace dropped {records} records before line {line}, so it cannot be replayed")]
    Dropped { line: usize, records: usize },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TraceRecord {
    Out {
        text: String,
        truncated_from: Option<usize>,
    },
    In {
        text: String,
        truncated_from: Option<usize>,
    },
    Stderr,
    Note {
        text: String,
    },
    Dropped {
        records: usize,
    },
    End {
        dropped: usize,
    },
}

const CLOSING_STDIN: &str = "closing stdin";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrub {
    from: String,
    to: String,
}

impl Scrub {
    #[must_use]
    pub const fn new(from: String, to: String) -> Self {
        Self { from, to }
    }

    fn apply(&self, text: &str) -> String {
        let escaped = |text: &str| {
            let quoted = Value::from(text).to_string();
            quoted
                .strip_prefix('"')
                .and_then(|inner| inner.strip_suffix('"'))
                .map(str::to_owned)
                .expect("invariant: a JSON string is quoted")
        };
        text.replace(&escaped(&self.from), &escaped(&self.to))
            .replace(&self.from, &self.to)
    }
}

impl Transcript {
    #[must_use]
    pub const fn new(source: Source, launches: Vec<Vec<Step>>) -> Self {
        Self { source, launches }
    }

    #[must_use]
    pub const fn source(&self) -> Source {
        self.source
    }

    #[must_use]
    pub fn launch(&self, index: usize) -> Option<&[Step]> {
        self.launches.get(index).map(Vec::as_slice)
    }

    #[must_use]
    pub const fn launches(&self) -> usize {
        self.launches.len()
    }

    pub fn parse(text: &str) -> Result<Self, ParseError> {
        let mut source = None;
        let mut launches: Vec<Vec<Step>> = Vec::new();
        for (index, text) in text.lines().enumerate() {
            let line = index + 1;
            if text.trim().is_empty() {
                continue;
            }
            let record: Line =
                serde_json::from_str(text).map_err(|source| ParseError::Line { line, source })?;
            let step = match (record, source.is_some()) {
                (Line::Transcript { source: named }, false) => {
                    source = Some(named);
                    continue;
                }
                (Line::Transcript { .. }, true) => return Err(ParseError::RepeatedHeader { line }),
                (_, false) => return Err(ParseError::MissingHeader),
                (Line::Launch, true) => {
                    launches.push(Vec::new());
                    continue;
                }
                (Line::Out { text }, true) => Step::Out { text },
                (Line::In { text }, true) => Step::In { text },
                (
                    Line::Request {
                        method,
                        path,
                        reply,
                    },
                    true,
                ) => Step::Request {
                    method,
                    path,
                    reply,
                },
                (Line::Closed, true) => Step::Closed,
            };
            launches
                .last_mut()
                .ok_or(ParseError::StepBeforeLaunch { line })?
                .push(step);
        }
        let source = source.ok_or(ParseError::MissingHeader)?;
        if launches.is_empty() {
            return Err(ParseError::NoLaunch);
        }
        Ok(Self { source, launches })
    }

    #[must_use]
    pub fn render(&self) -> String {
        let header = std::iter::once(Line::Transcript {
            source: self.source,
        });
        let lines = self.launches.iter().flat_map(|launch| {
            std::iter::once(Line::Launch).chain(launch.iter().cloned().map(Line::from))
        });
        header
            .chain(lines)
            .map(|line| {
                let mut text = serde_json::to_string(&line)
                    .expect("invariant: a transcript line always serializes");
                text.push('\n');
                text
            })
            .collect()
    }
}

pub fn record(trace: &str, scrubs: &[Scrub]) -> Result<Vec<Step>, RecordError> {
    let scrubbed = |text: String| scrubs.iter().fold(text, |text, scrub| scrub.apply(&text));
    let mut steps = Vec::new();
    for (index, text) in trace.lines().enumerate() {
        let line = index + 1;
        if text.trim().is_empty() {
            continue;
        }
        let record: TraceRecord =
            serde_json::from_str(text).map_err(|source| RecordError::Line { line, source })?;
        match record {
            TraceRecord::Out {
                truncated_from: Some(bytes),
                ..
            }
            | TraceRecord::In {
                truncated_from: Some(bytes),
                ..
            } => return Err(RecordError::Truncated { line, bytes }),
            TraceRecord::Dropped { records } => {
                return Err(RecordError::Dropped { line, records });
            }
            TraceRecord::End { dropped } if dropped > 0 => {
                return Err(RecordError::Dropped {
                    line,
                    records: dropped,
                });
            }
            TraceRecord::Out {
                text,
                truncated_from: None,
            } => steps.push(Step::Out {
                text: scrubbed(text),
            }),
            TraceRecord::In {
                text,
                truncated_from: None,
            } => steps.push(Step::In {
                text: scrubbed(text),
            }),
            TraceRecord::Note { text } if text.starts_with(CLOSING_STDIN) => {
                steps.push(Step::Closed);
            }
            TraceRecord::Note { .. } | TraceRecord::Stderr | TraceRecord::End { .. } => {}
        }
    }
    Ok(steps)
}
