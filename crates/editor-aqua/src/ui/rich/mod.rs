#[cfg(feature = "test-support")]
pub use media::audio::AudioFixture;

pub mod actions;
pub mod charts;
pub(crate) mod data;
mod kit;
pub(crate) mod narrative;

#[derive(Debug)]
pub(crate) enum Prepared {
    Chart(charts::Prepared),
    Callout(narrative::callout::Callout),
    Compare(narrative::compare::Compare),
    Stats(data::stats::Stats),
    CodeRef(code::code_ref::CodeRefBlock),
    AnnotatedCode(code::annotated::AnnotatedCode),
    FileTree(code::file_tree::FileTree),
    Image(std::sync::Arc<media::image::ImageBlock>),
    ImageCompare(media::image_compare::ImageCompare),
    Audio(media::audio::AudioBlock),
    Model(media::model::ModelBlock),
}

impl Prepared {
    pub(crate) fn resident_bytes(&self) -> usize {
        match self {
            Self::Chart(value) => value.resident_bytes(),
            Self::Callout(value) => value.resident_bytes(),
            Self::Compare(value) => value.resident_bytes(),
            Self::Stats(value) => value.resident_bytes(),
            Self::CodeRef(value) => value.resident_bytes(),
            Self::AnnotatedCode(value) => value.resident_bytes(),
            Self::FileTree(value) => value.resident_bytes(),
            Self::Image(value) => value.resident_bytes(),
            Self::ImageCompare(value) => value.resident_bytes(),
            Self::Audio(value) => value.resident_bytes(),
            Self::Model(value) => value.resident_bytes(),
        }
    }
}

fn prepare_kind(kind: &str, value: &serde_json::Value) -> Option<Result<Prepared, String>> {
    use serde::Deserialize;
    if let Some(result) = charts::prepare(kind, value) {
        return Some(result.map(Prepared::Chart));
    }
    Some(match kind {
        "callout" => narrative::callout::Callout::deserialize(value)
            .map(Prepared::Callout)
            .map_err(|error| error.to_string()),
        "compare" => narrative::compare::Compare::deserialize(value)
            .map(Prepared::Compare)
            .map_err(|error| error.to_string()),
        "stats" => data::stats::Stats::deserialize(value)
            .map(Prepared::Stats)
            .map_err(|error| error.to_string()),
        "code-ref" => code::code_ref::CodeRefBlock::deserialize(value)
            .map(Prepared::CodeRef)
            .map_err(|error| error.to_string()),
        "annotated-code" => code::annotated::AnnotatedCode::deserialize(value)
            .map(Prepared::AnnotatedCode)
            .map_err(|error| error.to_string()),
        "file-tree" => code::file_tree::FileTree::deserialize(value)
            .map(Prepared::FileTree)
            .map_err(|error| error.to_string()),
        "image" => media::image::ImageBlock::deserialize(value)
            .map(|image| Prepared::Image(std::sync::Arc::new(image)))
            .map_err(|error| error.to_string()),
        "image-compare" => media::image_compare::ImageCompare::deserialize(value)
            .map(Prepared::ImageCompare)
            .map_err(|error| error.to_string()),
        "audio" => media::audio::AudioBlock::deserialize(value)
            .map(Prepared::Audio)
            .map_err(|error| error.to_string()),
        "model" => media::model::ModelBlock::deserialize(value)
            .map(Prepared::Model)
            .map_err(|error| error.to_string()),
        _ => return None,
    })
}

#[derive(Debug)]
pub(crate) enum RichFence {
    Pending { kind: String },
    Ready(Envelope),
    Invalid { note: String },
}

#[derive(Debug)]
pub(crate) struct Envelope {
    pub(crate) block: Prepared,
    pub(crate) title: Option<String>,
    pub(crate) subtitle: Option<String>,
    pub(crate) caption: Option<String>,
}

impl RichFence {
    pub(crate) fn resident_bytes(&self) -> usize {
        match self {
            Self::Pending { kind } => kind.capacity(),
            Self::Invalid { note } => note.capacity(),
            Self::Ready(envelope) => {
                envelope.block.resident_bytes()
                    + envelope.title.as_ref().map_or(0, String::capacity)
                    + envelope.subtitle.as_ref().map_or(0, String::capacity)
                    + envelope.caption.as_ref().map_or(0, String::capacity)
            }
        }
    }
}

pub(crate) fn prepare(language: Option<&str>, source: &str, closed: bool) -> Option<RichFence> {
    let kind = language?.strip_prefix("artisan-")?;
    if kind.is_empty() {
        return None;
    }
    if !closed {
        return Some(RichFence::Pending {
            kind: kind.replace('-', " "),
        });
    }
    let parsed = if source.len() > crate::ui::markdown::MAX_MARKDOWN_SOURCE_BYTES {
        Err(format!(
            "the spec is larger than {} KiB",
            crate::ui::markdown::MAX_MARKDOWN_SOURCE_BYTES / 1024
        ))
    } else {
        serde_json::from_str::<serde_json::Value>(source)
            .map_err(|error| format!("the JSON does not parse: {error}"))
            .and_then(|value| prepare_value(kind, &value))
    };
    Some(match parsed {
        Ok(prepared) => RichFence::Ready(prepared),
        Err(error) => RichFence::Invalid {
            note: format!("Couldn't draw artisan-{kind}: {error}"),
        },
    })
}

fn prepare_value(kind: &str, value: &serde_json::Value) -> Result<Envelope, String> {
    if !value.is_object() {
        return Err("the spec must be a JSON object".to_owned());
    }
    validate_value_budget(value)?;
    let block = prepare_kind(kind, value)
        .unwrap_or_else(|| Err(format!("`{kind}` is not a rich block kind")))?;
    let field = |name: &str| {
        value
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Ok(Envelope {
        block,
        title: field("title"),
        subtitle: field("subtitle"),
        caption: field("caption"),
    })
}

fn validate_value_budget(root: &serde_json::Value) -> Result<(), String> {
    const VALUES: usize = 32768;
    const DEPTH: usize = 32;
    let mut pending = vec![(root, 0usize)];
    let mut visited = 0;
    while let Some((value, depth)) = pending.pop() {
        visited += 1;
        if visited > VALUES || depth > DEPTH {
            return Err("rich preview exceeds 32768 JSON values or 32 nesting levels".into());
        }
        let count = match value {
            serde_json::Value::Array(values) => values.len(),
            serde_json::Value::Object(values) => values.len(),
            _ => 0,
        };
        if pending.len().saturating_add(count) > VALUES - visited {
            return Err("rich preview exceeds 32768 JSON values".into());
        }
        match value {
            serde_json::Value::Array(values) => {
                pending.extend(values.iter().map(|value| (value, depth + 1)))
            }
            serde_json::Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    Ok(())
}
pub(crate) mod code;
pub(crate) mod media;
