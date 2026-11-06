use super::{Layout, Row, TerminalDialog, TerminalPalette, shell_prompt};
use crate::ui::{
    button::{Button, ButtonVariant},
    glass::{GlassStrength, glass_card_shadows, glass_layers},
    theme::{
        ArtisanTheme, Elevation, Opacity, RadiusStep, RadiusTokens, TextStep, TypographyTokens,
    },
};
use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
use aqua_ui::{
    collection::{
        CollectionDescriptor, CollectionError, CollectionLayout, CollectionRole, CollectionSource,
        CollectionViewport,
    },
    element::{Box as UiBox, Element, Key, Text},
    style::{Length, Style},
};
use editor_assets::AssetId;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TerminalAction {
    Retry,
    Copy,
    JumpToLatest,
    Hover(bool),
    Viewport(CollectionViewport),
}

struct Rows {
    dialog: Rc<RefCell<TerminalDialog>>,
    layout: Layout,
    theme: ArtisanTheme,
}

impl CollectionSource for Rows {
    fn len(&self) -> usize {
        self.layout.total()
    }
    fn key(&self, index: usize) -> Key {
        Key::Usize(index)
    }
    fn layout(&self, width: f32) -> CollectionLayout {
        CollectionLayout {
            content_width: width,
            column_widths: Vec::new(),
        }
    }
    fn role(&self) -> CollectionRole {
        CollectionRole::List
    }
    fn header(&self, _: &CollectionLayout) -> Option<Element> {
        None
    }
    fn validate(&self) -> Result<(), CollectionError> {
        Ok(())
    }
    fn row(&self, index: usize, _: &CollectionLayout) -> Element {
        let state = self.dialog.borrow();
        let Some(job) = &state.job else {
            return UiBox::new().height(18.0).build();
        };
        let theme = self.theme;
        let font = TextStyle {
            families: vec![FontFamily::Named(theme.typography.mono.family.to_owned())],
            size_px: TypographyTokens::size(TextStep::Terminal),
            line_height_px: 18.0,
            ..TextStyle::default()
        };
        let mut row = UiBox::new().width(Length::Pct(100.0)).style(
            Style::new()
                .min_height(Length::Px(18.0))
                .padding_insets(Insets {
                    left: 16.0,
                    right: 16.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        );
        match self.layout.row(index) {
            Some(Row::Command(index)) => {
                let prompt = shell_prompt(job.cwd.as_deref());
                let prefix = if index == 0 {
                    prompt.prompt.as_str()
                } else {
                    prompt.continuation
                };
                let command = job
                    .commands
                    .get(index)
                    .map_or("", |range| &job.command[range.clone()]);
                let mut spans = Vec::new();
                let highlighted = if index == 0 && prompt.coloured {
                    prefix.trim_end().trim_end_matches('$').len()
                } else {
                    0
                };
                if highlighted > 0 {
                    let mut bold = font.clone();
                    bold.weight = FontWeight::BOLD;
                    spans.push(
                        StyledSpan::new(&prefix[..highlighted], bold)
                            .gpui_source_color(TerminalPalette::for_theme(&theme).base[12]),
                    );
                }
                spans.push(StyledSpan::new(
                    format!("{}{command}", &prefix[highlighted..]),
                    font.clone(),
                ));
                row = row.child(
                    Text::new("")
                        .text_style(font)
                        .spans(spans)
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
                        .build(),
                );
            }
            Some(Row::Missing) => {
                row = row.child(
                    Text::new(if job.unverified {
                        "Live output only; Forge supplied no final snapshot."
                    } else {
                        "Earlier output is not kept; showing the newest."
                    })
                    .text_style(font)
                    .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                    .build(),
                )
            }
            Some(Row::Output(index)) => {
                if let Some(line) = job.screen.line(index) {
                    let fg = theme.colors.text.to_reference_text_color();
                    let spans = TerminalPalette::for_theme(&theme).spans(
                        line,
                        &font,
                        [fg.hue_degrees, fg.saturation, fg.lightness, fg.alpha],
                        [0.0, 0.0, 0.0, 0.0],
                    );
                    row = row.child(
                        Text::new("")
                            .text_style(font)
                            .spans(spans)
                            .gpui_source_color(theme.colors.text.to_reference_text_color())
                            .build(),
                    );
                }
            }
            Some(Row::Status) => {
                if job.status == artisan_api::Status::Running {
                    row = row.child(
                        UiBox::new()
                            .width(7.5)
                            .height(15.0)
                            .fill(theme.colors.text_muted.to_paint())
                            .build(),
                    );
                } else {
                    row = row.child(
                        Text::new(shell_prompt(job.cwd.as_deref()).prompt)
                            .text_style(font)
                            .gpui_source_color(theme.colors.text.to_reference_text_color())
                            .build(),
                    );
                }
            }
            None => {}
        }
        row.build()
    }
}

pub fn terminal_view(
    dialog: Rc<RefCell<TerminalDialog>>,
    theme: ArtisanTheme,
    viewport_height: f32,
    hovered: bool,
    copied: bool,
    command_source: Option<Element>,
    output_source: Option<Element>,
    action: Rc<dyn Fn(TerminalAction)>,
) -> Result<Element, &'static str> {
    let state = dialog.borrow();
    let Some(job) = &state.job else {
        return Err("terminal dialog is closed");
    };
    if job.command_source.is_some() && command_source.is_none() {
        return Err("terminal command requires its source view");
    }
    if job.output_source.is_some() && output_source.is_none() {
        return Err("terminal output requires its source view");
    }
    let output_stored = job.output_source.is_some();
    let layout = Layout::of(job);
    let height = (viewport_height * 0.62)
        .min(640.0)
        .min((viewport_height - 160.0).max(32.0));
    let mut content = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().padding_insets(Insets {
            top: 14.0,
            bottom: 14.0,
            left: 0.0,
            right: 0.0,
        }));
    if let Some(error) = &state.error {
        let retry = Rc::clone(&action);
        content = content.child(Text::new(error.clone()).build()).child(
            Button::text("terminal-retry", theme, "Retry")
                .on_activate(move |_| retry(TerminalAction::Retry))
                .build(),
        );
    } else if state.loading && job.data.is_none() {
        content = content.child(Text::new("Loading shell output…").build());
    }
    if let Some(source) = command_source {
        content = content.child(source);
    }
    if let Some(source) = output_source {
        content = content.child(source);
    }
    let following = state.following;
    drop(state);
    let source = Rc::new(Rows {
        dialog,
        layout,
        theme,
    });
    let mut descriptor = CollectionDescriptor::new(
        source,
        vec![
            Style::new()
                .height(Length::Px(height))
                .width(Length::Pct(100.0)),
        ],
        true,
        18.0,
        4,
    );
    let viewport_callback = Rc::clone(&action);
    descriptor.on_viewport = Some(Rc::new(move |viewport| {
        viewport_callback(TerminalAction::Viewport(viewport))
    }));
    content = content.child(Element::collection(descriptor).with_key("inspector-terminal-lines"));
    let copy = Rc::clone(&action);
    content = content.child(
        UiBox::new()
            .opacity(if hovered { 1.0 } else { 0.0 })
            .style(Style::new().absolute_at(None, Some(8.0), Some(8.0), None))
            .child(
                Button::icon(
                    "inspector-terminal-copy",
                    theme,
                    if copied {
                        AssetId::TABLER_CHECK
                    } else {
                        AssetId::TABLER_COPY
                    },
                    "Copy output",
                )
                .variant(ButtonVariant::Ghost)
                .disabled(output_stored)
                .on_activate(move |_| copy(TerminalAction::Copy))
                .build(),
            )
            .build(),
    );
    if !following {
        let latest = Rc::clone(&action);
        content = content.child(
            UiBox::new()
                .style(Style::new().absolute_at(None, None, Some(8.0), Some(8.0)))
                .child(
                    Button::icon(
                        "inspector-terminal-latest",
                        theme,
                        AssetId::TABLER_ARROW_BAR_TO_DOWN,
                        "Jump to latest",
                    )
                    .variant(ButtonVariant::Ghost)
                    .on_activate(move |_| latest(TerminalAction::JumpToLatest))
                    .build(),
                )
                .build(),
        );
    }
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let hover = Rc::clone(&action);
    Ok(UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .radius(radius)
        .clip(true)
        .border((1.0, theme.colors.text.at(Opacity::GlassBorder).to_paint()))
        .style(
            Style::new().shadows(
                glass_card_shadows(&theme, Elevation::Overlay)
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            ),
        )
        .child(glass_layers(&theme, GlassStrength::Strong, radius))
        .child(content.build())
        .build()
        .with_key("inspector-terminal-card")
        .on_hover_change(move |value| hover(TerminalAction::Hover(value))))
}
