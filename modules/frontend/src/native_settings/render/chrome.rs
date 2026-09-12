//! Shared settings surface chrome: the notification and telemetry captions,
//! the page header, section shell, card and row recipes, and the nav rail
//! primitives.
//!
//! Split from `native_settings/render.rs`; the sibling `sections` child paints
//! the section bodies through these helpers, so they are `pub(super)`.

use super::*;

/// Returns the desktop gap-notice copy for one notification gap.
///
/// `None` means the legacy surface paints no notice. The copy below is the
/// desktop branch; the browser branch needs a runtime-surface input that has
/// no Rust owner yet.
#[must_use]
pub const fn notification_gap_notice(
    gap: SystemNotificationGap,
) -> Option<(&'static str, &'static str)> {
    match gap {
        SystemNotificationGap::Blocked => Some((
            "Blocked by your system",
            "Artisan asked and your system refused. Allow notifications for Artisan in your operating system's notification settings, then check again.",
        )),
        SystemNotificationGap::Unprompted => Some((
            "Not allowed yet",
            "The permission prompt was closed without an answer, so nothing can be posted yet. Asking again is safe.",
        )),
        SystemNotificationGap::None | SystemNotificationGap::Unsupported => None,
    }
}

/// Returns the telemetry caption painted beside one category switch.
#[must_use]
pub const fn telemetry_choice_caption(choice: TelemetryPreference) -> &'static str {
    match choice {
        TelemetryPreference::Unset => "Not decided",
        TelemetryPreference::Enabled => "On",
        TelemetryPreference::Disabled => "Off",
    }
}

// --- Shared section chrome --------------------------------------------------
//
// header.svelte: h1 at text-xl semibold with a mt-1.5 text-sm muted
// description. section.svelte: mt-10 block with a text-sm medium heading,
// optional baseline action, and optional intro. card.svelte: the moulded
// gradient well with hairline dividers; GPUI paints the flat compact_card
// recipe instead of the CSS gradient (see the surface-225/200 note on
// settings_card). row.svelte: the sm: side-by-side row; the native window is
// always wide, so only the side-by-side arrangement is painted.

/// Paints the page header (`header.svelte`).
pub(super) fn settings_header(
    theme: &artisan_ui::theme::ArtisanTheme,
    title: String,
    description: &str,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(20.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.foreground.to_paint())
                .child(title),
        )
        .child(
            div()
                .mt(px(6.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(description.to_owned()),
        )
}

/// Paints one section block (`section.svelte`).
pub(super) fn settings_section_shell(
    theme: &artisan_ui::theme::ArtisanTheme,
    anchor: &str,
    title: &str,
    intro: Option<&str>,
    action: Option<AnyElement>,
    body: Div,
) -> Div {
    let selector = format!("settings-section-{anchor}");
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.0));
    head = head.child(
        div()
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.foreground.to_paint())
            .child(title.to_owned()),
    );
    if let Some(action) = action {
        head = head.child(action);
    }
    let mut section = div()
        .flex()
        .flex_col()
        .mt(px(40.0))
        .debug_selector(move || selector.clone())
        .child(head);
    if let Some(intro) = intro {
        section = section.child(
            div()
                .mt(px(4.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(intro.to_owned()),
        );
    }
    section.child(div().mt(px(12.0)).child(body))
}

/// Paints one card (`card.svelte`).
///
/// Legacy: `rounded-xl` gradient well (`from-surface-225 to-surface-200`,
/// `dark:from-surface-800 dark:to-surface-925`) with `divide-y
/// divide-border/40` hairlines. The `surface-*` steps exist in
/// `artisan-ui` (`SurfaceStep::S200`/`S225`/`S800`/`S925`) but GPUI paints no
/// CSS gradient, so the flat `compact_card` recipe stands in; dividers reuse
/// the theme border at full alpha.
pub(super) fn settings_card(theme: &artisan_ui::theme::ArtisanTheme, blocks: Vec<Div>) -> Div {
    let style = CardStyle::resolve(*theme);
    let border = theme.colors.border.to_paint();
    let mut card = compact_card(style).w_full().gap(px(0.0)).py(px(0.0));
    for (index, block) in blocks.into_iter().enumerate() {
        let mut band = compact_card_content(style).w_full().child(block);
        if index > 0 {
            band = band.border_t_1().border_color(border);
        }
        card = card.child(band);
    }
    card
}

/// Paints one row (`row.svelte`): title plus description with an optional
/// trailing control.
pub(super) fn settings_row(
    theme: &artisan_ui::theme::ArtisanTheme,
    title: &str,
    description: &str,
    control: Option<AnyElement>,
) -> Div {
    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(24.0))
        .py(px(14.0));
    row = row.child(
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .gap(px(2.0))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(title.to_owned()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(description.to_owned()),
            ),
    );
    if let Some(control) = control {
        row = row.child(div().flex_shrink_0().child(control));
    }
    row
}

/// Paints the rail heading and group labels (`+layout.svelte`, `nav.svelte`).
pub(super) fn nav_heading(theme: &artisan_ui::theme::ArtisanTheme) -> Div {
    div()
        .px(px(8.0))
        .pb(px(12.0))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.foreground.to_paint())
        .child("Settings")
}

/// Paints one uppercase group label (`ARTISAN`, `ENGINES`).
///
/// Legacy uppercases via CSS; GPUI has no text transform, so the labels are
/// stored pre-uppercased.
pub(super) fn nav_group_label(theme: &artisan_ui::theme::ArtisanTheme, label: &'static str) -> Div {
    div()
        .px(px(8.0))
        .pt(px(20.0))
        .pb(px(6.0))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(label)
}

/// Paints one nav row (`nav.svelte` `nav_link`).
///
/// The active row wears the opaque well: legacy paints the same surface
/// gradient as cards, approximated here with the flat muted fill. Icons are
/// omitted: no tabler-icon component exists in the native port yet. The
/// caller attaches the debug selector and the navigation click, so the rail
/// stays live without styling knowledge leaking into event wiring.
pub(super) fn nav_link(
    theme: &artisan_ui::theme::ArtisanTheme,
    label: String,
    active: bool,
) -> Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(28.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .gap(px(8.0))
        .text_sm();
    if active {
        row.bg(theme.colors.muted.to_paint())
            .text_color(theme.colors.foreground.to_paint())
            .font_weight(FontWeight::MEDIUM)
            .child(label)
    } else {
        row.text_color(theme.colors.muted_foreground.to_paint())
            .child(label)
    }
}

/// Paints the anchor sub-list under the active nav row.
///
/// The rail sits on the group border in legacy (`ml-[0.9375rem]`,
/// `border-l`, `pl-3`); the active-hash tick is omitted because the screen
/// carries no hash state (deep-link scroll is an orchestrator gap).
pub(super) fn anchor_list(
    theme: &artisan_ui::theme::ArtisanTheme,
    anchors: &[SettingsAnchor],
) -> Div {
    let mut list = div()
        .flex()
        .flex_col()
        .ml(px(15.0))
        .pl(px(12.0))
        .my(px(4.0))
        .border_l_1()
        .border_color(theme.colors.border.to_paint());
    for anchor in anchors {
        let selector = format!("settings-anchor-{}", anchor.hash);
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .debug_selector(move || selector.clone())
                .child(anchor.label.to_owned()),
        );
    }
    list
}
