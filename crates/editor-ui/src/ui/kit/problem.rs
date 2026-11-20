use crate::ui::kit::surface::tint;
use crate::ui::kit::{IconSize, Space, Spaced, Surface, TextRole, Tone, Tooltip};
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity};
use aqua_core::GpuiBackground;
use aqua_core::LiveRegion;
use aqua_ui::element::{Box as UiBox, Element, Key, Text};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use editor_assets::AssetId;
use editor_core::Problem;

const SURFACE: Surface = Surface::Callout(Tone::Danger);
const DETAIL_DELAY: f64 = 0.25;
const DETAIL_WIDTH: f32 = 420.0;

#[derive(Debug)]
pub struct ProblemNotice {
    key: Key,
    theme: ArtisanTheme,
    problem: Problem,
    actions: Vec<Element>,
    base: Option<Oklch>,
}

impl ProblemNotice {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the notice stores this theme copy and paints from it when built"
    )]
    pub fn new(key: impl Into<Key>, theme: ArtisanTheme, problem: &Problem) -> Self {
        Self {
            key: key.into(),
            theme,
            problem: problem.clone(),
            actions: Vec::new(),
            base: None,
        }
    }

    #[must_use]
    pub fn action(mut self, action: Element) -> Self {
        self.actions.push(action);
        self
    }

    #[must_use]
    pub fn over(mut self, base: Oklch) -> Self {
        self.base = Some(base);
        self
    }

    #[must_use]
    pub fn build(self) -> Element {
        let theme = self.theme;
        let color = tint(&theme, Tone::Danger);
        let icon = UiBox::new()
            .height(TextRole::Label.style(&theme).line_height_px)
            .style(Style::new().align(AlignItems::CENTER).flex_shrink(0.0))
            .child(IconSize::Md.glyph(
                AssetId::TABLER_ALERT_TRIANGLE,
                color.at(Opacity::ToneIcon).to_reference_paint(),
            ))
            .build();
        let headline = TextRole::Label
            .text(&theme, self.problem.headline().to_owned())
            .gpui_source_color(color.to_reference_text_color())
            .wrap(true)
            .build();
        let mut lines = UiBox::new()
            .layout("column")
            .spaced(Space::S8)
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .flex_shrink(1.0)
                    .min_width(Length::Pct(0.0)),
            )
            .child(headline);
        let details = self
            .problem
            .detail()
            .map(|detail| details(&theme, detail.to_owned()));
        if !self.actions.is_empty() || details.is_some() {
            lines = lines.child(
                UiBox::new()
                    .layout("row")
                    .spaced(Space::S12)
                    .style(
                        Style::new()
                            .align(AlignItems::CENTER)
                            .justify(JustifyContent::START),
                    )
                    .children(self.actions)
                    .children(details)
                    .build(),
            );
        }
        let body = UiBox::new()
            .layout("row")
            .spaced(Space::S8)
            .style(Style::new().align(AlignItems::START))
            .child(icon)
            .child(lines.build())
            .build();
        let notice = SURFACE
            .build(&theme, vec![body])
            .style(Style::new().width(Length::Pct(100.0)))
            .with_key(self.key)
            .aria_live(LiveRegion::Polite);
        let notice = match self.problem.detail() {
            Some(detail) => notice.aria_description(detail.to_owned()),
            None => notice,
        };
        match self.base {
            Some(base) => UiBox::new()
                .radius(SURFACE.radius_px())
                .style(
                    Style::new()
                        .width(Length::Pct(100.0))
                        .gpui_background(GpuiBackground::Solid(base.to_reference_paint())),
                )
                .child(notice)
                .build(),
            None => notice,
        }
    }
}

fn details(theme: &ArtisanTheme, detail: String) -> Element {
    let theme = *theme;
    TextRole::Caption
        .text(&theme, "Details".to_owned())
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .build()
        .tooltip_with(DETAIL_DELAY, crate::ui::hint_card::presence(), move || {
            Surface::Tooltip(Tooltip::Card)
                .build(
                    &theme,
                    vec![
                        Text::new(detail.clone())
                            .text_style(TextRole::Caption.style(&theme))
                            .gpui_source_color(theme.colors.text.to_reference_text_color())
                            .wrap(true)
                            .selectable(true)
                            .build(),
                    ],
                )
                .style(Style::new().width(Length::Px(DETAIL_WIDTH)))
        })
}
