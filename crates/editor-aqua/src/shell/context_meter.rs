#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextReading {
    model_name: String,
    context_tokens: u64,
    window_tokens: u64,
}

impl ContextReading {
    fn new(model_name: String, context_tokens: u64, window_tokens: u64) -> Option<Self> {
        (window_tokens > 0).then_some(Self {
            model_name,
            context_tokens,
            window_tokens,
        })
    }

    pub(crate) fn percent(&self) -> f64 {
        (crate::measure::ratio(self.context_tokens, self.window_tokens) * 100.0).clamp(0.0, 100.0)
    }

    pub(crate) fn percent_sentence(&self) -> String {
        format!(
            "The context window for {} is {}% full.",
            self.model_name,
            self.percent().round()
        )
    }
}

pub(crate) enum Subject {
    Thread,
    Agent(Option<(u64, Option<String>)>),
}

pub(crate) fn reading(
    host: &crate::core::HostData,
    thread: &artisan_api::ThreadId,
    subject: Subject,
) -> Option<ContextReading> {
    let run = host.runs.get(thread)?;
    let (used, model) = match subject {
        Subject::Agent(agent) => {
            let (used, model) = agent?;
            (used, model.or_else(|| run.model.clone())?)
        }
        Subject::Thread => (run.usage.context_used?, run.model.clone()?),
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
