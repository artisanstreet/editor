//! `callout`: a note set apart, in the tone of its kind.

use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div, prelude::*, px,
};
use serde::Deserialize;
use std::fmt::Write as _;

use super::super::kit::{self, Tone};
use super::super::{BlockContext, Chrome, RichBlock};
use crate::theme::ArtisanTheme;
use artisan_assets::AssetId;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Kind {
    #[default]
    Note,
    Tip,
    Warning,
    Risk,
    Decision,
    Assumption,
}

impl Kind {
    fn tone(self) -> Tone {
        match self {
            Self::Note => Tone::Info,
            Self::Tip => Tone::Success,
            Self::Warning => Tone::Warning,
            Self::Risk => Tone::Danger,
            Self::Decision => Tone::Accent,
            Self::Assumption => Tone::Neutral,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Note => "Note",
            Self::Tip => "Tip",
            Self::Warning => "Warning",
            Self::Risk => "Risk",
            Self::Decision => "Decision",
            Self::Assumption => "Assumption",
        }
    }

    fn icon(self) -> AssetId {
        match self {
            Self::Note => AssetId::TABLER_MESSAGE_CIRCLE,
            Self::Tip => AssetId::TABLER_SPARKLES,
            Self::Warning | Self::Risk => AssetId::TABLER_ALERT_TRIANGLE,
            Self::Decision => AssetId::TABLER_TARGET_ARROW,
            Self::Assumption => AssetId::TABLER_QUESTION_MARK,
        }
    }
}

/// The `callout` spec.
#[derive(Debug, Deserialize)]
pub(super) struct Callout {
    #[serde(default)]
    kind: Kind,
    /// The envelope title, drawn on the callout's label line.
    #[serde(default)]
    title: Option<String>,
    body: String,
    #[serde(default)]
    collapsed: bool,
}

impl RichBlock for Callout {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        CalloutView {
            id: cx.key("callout"),
            theme: *cx.theme,
            kind: self.kind,
            heading: self.title.clone().map(SharedString::from),
            body: SharedString::from(self.body.clone()),
            collapsed: self.collapsed,
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        let heading = self
            .title
            .as_deref()
            .map_or_else(String::new, |title| format!(" {title}"));
        let mut out = format!("> **{}:**{heading}\n>\n", self.kind.label());
        for line in self.body.lines() {
            let _ = writeln!(out, "> {line}");
        }
        out
    }

    fn chrome(&self) -> Chrome {
        Chrome::Bare
    }

    fn owns_title(&self) -> bool {
        true
    }
}

#[derive(IntoElement)]
struct CalloutView {
    id: SharedString,
    theme: ArtisanTheme,
    kind: Kind,
    heading: Option<SharedString>,
    body: SharedString,
    collapsed: bool,
}

struct CalloutState {
    open: bool,
}

impl RenderOnce for CalloutView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let collapsible = self.collapsed;
        let state = kit::keyed(window, cx, gpui::ElementId::Name(self.id.clone()), || {
            CalloutState { open: !collapsible }
        });
        let open = state.read(cx).open;
        let tone = self.kind.tone();
        let color = tone.color(&theme);
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(kit::glyph(self.kind.icon(), 16.0, color.to_paint()))
            .child(
                div()
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(color.to_paint())
                    .child(self.kind.label()),
            )
            .children(self.heading.map(|heading| kit::title(&theme, heading)))
            .when(collapsible, |head| {
                head.child(div().flex_1()).child(kit::glyph(
                    if open {
                        AssetId::TABLER_CHEVRON_UP
                    } else {
                        AssetId::TABLER_CHEVRON_DOWN
                    },
                    14.0,
                    theme.colors.muted_foreground.to_paint(),
                ))
            });
        let head = div()
            .id(gpui::ElementId::Name(SharedString::from(format!(
                "{}-head",
                self.id
            ))))
            .when(collapsible, |head| {
                head.cursor_pointer().on_click(move |_, _, cx| {
                    state.update(cx, |state, cx| {
                        state.open = !state.open;
                        cx.notify();
                    });
                })
            })
            .child(head);
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .py(px(10.0))
            .pl(px(14.0))
            .pr(px(12.0))
            .rounded(kit::inner_radius())
            .border_l_2()
            .border_color(color.to_paint())
            .bg(color.with_alpha(0.07).to_paint())
            .child(head)
            .when(open, |callout| {
                callout.child(kit::paragraphs(&theme, &self.body))
            })
    }
}
