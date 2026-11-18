#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextReading {
    pub model_name: String,
    pub context_tokens: u64,
    pub window_tokens: u64,
}

impl ContextReading {
    pub fn new(model_name: String, context_tokens: u64, window_tokens: u64) -> Option<Self> {
        (window_tokens > 0).then_some(Self {
            model_name,
            context_tokens,
            window_tokens,
        })
    }

    pub fn percent(&self) -> f64 {
        (self.context_tokens as f64 / self.window_tokens as f64 * 100.0).clamp(0.0, 100.0)
    }

    pub fn percent_sentence(&self) -> String {
        format!(
            "The context window for {} is {}% full.",
            self.model_name,
            self.percent().round()
        )
    }

    pub fn capacity_sentence(&self) -> String {
        format!(
            "{} has a context window of {} tokens.",
            self.model_name,
            compact_tokens(self.window_tokens)
        )
    }
}

pub fn compact_tokens(count: u64) -> String {
    let value = count as f64;
    let (scaled, suffix) = if value >= 999_500.0 {
        (value / 1e6, "M")
    } else if value >= 999.5 {
        (value / 1e3, "K")
    } else {
        (value, "")
    };
    let digits = match scaled < 10.0 && !suffix.is_empty() {
        true => 1,
        false => 0,
    };
    format!("{}{suffix}", editor_ui::locale::trimmed(scaled, digits))
}

pub fn reading(
    host: &crate::core::HostData,
    thread: &artisan_api::ThreadId,
    agent: Option<Option<(u64, Option<String>)>>,
) -> Option<ContextReading> {
    let run = host.runs.get(thread)?;
    let (used, model) = match agent {
        Some(agent) => {
            let (used, model) = agent?;
            (used, model.or_else(|| run.model.clone())?)
        }
        None => (run.usage.context_used?, run.model.clone()?),
    };
    let known = host
        .engines
        .get(&run.engine)
        .and_then(|engine| engine.models.iter().find(|known| known.id == model));
    let window = if run.model.as_deref() == Some(model.as_str()) {
        run.usage.context_window
    } else {
        None
    }
    .or_else(|| known.and_then(|known| known.context_window))?;
    let name = known.map_or_else(|| model.clone(), |known| known.name.clone());
    ContextReading::new(name, used, window)
}
