use tracing_error::SpanTrace;

#[derive(Debug, Clone)]
pub struct Spans(SpanTrace);

impl Spans {
    #[must_use]
    pub fn capture() -> Self {
        Self(SpanTrace::capture())
    }
}

impl std::fmt::Display for Spans {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut spans = Vec::new();
        self.0.with_spans(|metadata, fields| {
            spans.push(if fields.is_empty() {
                metadata.name().to_owned()
            } else {
                format!("{}{{{fields}}}", metadata.name())
            });
            true
        });
        if spans.is_empty() {
            return f.write_str("outside any span");
        }
        spans.reverse();
        f.write_str(&spans.join(" > "))
    }
}
