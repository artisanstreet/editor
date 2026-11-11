#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextReading {
    pub model_name: String,
    pub context_tokens: u64,
    pub window_tokens: u64,
}

pub use crate::ui::progress::Tone as ContextTone;

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

    pub fn tone(&self) -> ContextTone {
        ContextTone::of(self.percent())
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
    let text = if scaled < 10.0 && !suffix.is_empty() {
        format!("{scaled:.1}")
    } else {
        format!("{scaled:.0}")
    };
    format!("{}{suffix}", text.strip_suffix(".0").unwrap_or(&text))
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

pub fn ring(
    reading: &ContextReading,
    theme: crate::ui::theme::ArtisanTheme,
    hover: impl Fn(bool) + 'static,
) -> aqua_ui::element::Element {
    use aqua_core::vector::{GpuiPath, GpuiPathStyle, LineCap, LineJoin, PathCommand};
    use aqua_core::{DisplayItem, GpuiBackground, Point, Transform2D};
    use aqua_ui::{
        AlignItems, JustifyContent,
        element::{Box as UiBox, Canvas},
        style::Style,
    };
    let percent = reading.percent();
    let share = (percent / 100.0) as f32;
    let track = theme.colors.gray.step(4).to_reference_paint();
    let color = match reading.tone() {
        ContextTone::Normal => theme.colors.action_top,
        ContextTone::Warning => theme.colors.banner_warning.text,
        ContextTone::Danger => theme.colors.banner_error.text,
    }
    .to_reference_paint();
    let arc = Canvas::new()
        .width(16.0)
        .height(16.0)
        .on_paint(move |bounds| {
            let path = |share: f32, color| {
                if share <= 0.0 {
                    return None;
                }
                let degrees = -90.0_f32 + (share * 360.0).min(359.99);
                let radians = degrees.to_radians();
                let commands = vec![
                    PathCommand::Move(Point::new(8.0, 2.25)),
                    PathCommand::Arc {
                        radii: Point::new(5.75, 5.75),
                        rotation_radians: 0.0,
                        large: share >= 0.5,
                        sweep: true,
                        to: Point::new(8.0 + 5.75 * radians.cos(), 8.0 + 5.75 * radians.sin()),
                    },
                ];
                Some(DisplayItem::GpuiPath {
                    transformed_window_logical_bounds: bounds,
                    path: std::sync::Arc::new(GpuiPath {
                        commands_local: commands,
                        origin_window_logical: Point::new(bounds.x, bounds.y),
                        element_transform: Transform2D::IDENTITY,
                        style: GpuiPathStyle::Stroke {
                            width: 2.5,
                            cap: LineCap::Butt,
                            join: LineJoin::Miter,
                            dash: Vec::new(),
                            dash_offset: 0.0,
                        },
                        background: GpuiBackground::Solid(color),
                    }),
                })
            };
            [path(1.0, track), path(share, color)]
                .into_iter()
                .flatten()
                .collect()
        })
        .build();
    UiBox::new()
        .width(24.0)
        .height(24.0)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .child(arc)
        .build()
        .with_key("artisan-native-context-meter")
        .debug_selector("artisan-native-context-meter")
        .role(aqua_core::Role::ProgressIndicator)
        .aria_label("Context window usage")
        .aria_value(format!(
            "{percent:.0}%: {} of {} tokens for {}",
            reading.context_tokens, reading.window_tokens, reading.model_name
        ))
        .on_hover_change(hover)
}
