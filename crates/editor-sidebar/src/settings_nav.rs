//! The sidebar while Settings is open: the way back, then the settings
//! pages grouped by where they are kept (this Editor, the active host, the
//! host's engines), on the same rows and hover pill as New thread.

use crate::Sidebar;
use artisan_api::{EngineStatus, InstallState, SignInState};
use editor_core::{Screen, SettingsPage};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, Context, Div, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div, px,
};

/// The Editor's own pages, kept in this device's `settings.json`.
const EDITOR_PAGES: [(SettingsPage, AssetId); 4] = [
    (SettingsPage::General, AssetId::TABLER_SETTINGS),
    (SettingsPage::Account, AssetId::TABLER_SHIELD_LOCK),
    (SettingsPage::Hosts, AssetId::TABLER_DEVICE_LAPTOP),
    (SettingsPage::About, AssetId::TABLER_LIST_DETAILS),
];

/// The active host's pages, kept by its Forge.
const HOST_PAGES: [(SettingsPage, AssetId); 3] = [
    (SettingsPage::Threads, AssetId::TABLER_MESSAGES),
    (SettingsPage::GlobalPrompt, AssetId::TABLER_FILE_TEXT),
    (SettingsPage::Extensions, AssetId::TABLER_PLUG_CONNECTED),
];

/// A subsection row's height on the rail.
const SECTION_ROW: f32 = 32.0;
/// The caret's inset from its row's top and bottom.
const CARET_INSET: f32 = 6.0;
/// The caret's length: the row less its inset at each end.
const CARET_HEIGHT: f32 = SECTION_ROW - 2.0 * CARET_INSET;

/// Where the rail's caret was and is, so a change of subsection slides it
/// and opening a category grows it in once.
#[derive(Clone, Copy, Debug, Default)]
pub struct SettingsCaret {
    parent: Option<SettingsPage>,
    from: Option<usize>,
    to: Option<usize>,
    serial: u64,
}

/// What an engine still needs before it can run, said beside its row.
const fn engine_need(status: &EngineStatus) -> Option<&'static str> {
    match (&status.install, &status.sign_in) {
        (InstallState::Absent, _) => Some("Install"),
        (InstallState::Failed { .. }, _) => Some("Failed"),
        (InstallState::Downloading { .. } | InstallState::Verifying { .. }, _) => {
            Some("Installing")
        }
        (_, SignInState::SignedOut | SignInState::Failed { .. }) => Some("Sign in"),
        _ => None,
    }
}

impl Sidebar {
    /// A group's heading, as the history's day headings are drawn.
    fn settings_heading(label: impl Into<SharedString>, theme: &ArtisanTheme) -> Div {
        div()
            .h(px(21.0))
            .mt(px(14.0))
            .flex()
            .items_center()
            .px(px(8.0))
            .text_xs()
            .text_color(theme.colors.text_muted.to_paint())
            .truncate()
            .child(label.into())
    }

    fn settings_link(
        &self,
        target: SettingsPage,
        icon: AssetId,
        current: SettingsPage,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let label = target.label();
        let id = format!(
            "sidebar-settings-{}",
            label.to_lowercase().replace(' ', "-")
        );
        let expanded = target == current.parent();
        let sections = target.sections();
        let theme = *cx.theme();
        // An open category keeps the foreground label and only the shared
        // hover pill; the chevron and its children say it is open.
        let row = Self::nav_row(id.clone(), id, &self.nav_hover, icon, label, false, cx)
            .aria_selected(expanded)
            .when(!sections.is_empty(), |row| row.aria_expanded(expanded))
            .when(!sections.is_empty(), |row| {
                row.child(div().flex_1()).child(
                    asset_glyph(if expanded {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_RIGHT
                    })
                    .size(px(12.0))
                    .text_color(theme.colors.text_muted.to_paint()),
                )
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.navigate(Screen::Settings(target), cx);
            }));
        div().flex().flex_col().child(row).when(expanded, |group| {
            group.child(self.settings_subcategories(target, current, cx))
        })
    }

    /// Only the active category expands; the rail marks its selected subsection.
    #[expect(
        clippy::too_many_lines,
        reason = "rail rows, caret animation and expansion read as one navigation unit"
    )]
    fn settings_subcategories(
        &self,
        parent: SettingsPage,
        current: SettingsPage,
        cx: &Context<Self>,
    ) -> Div {
        let theme = *cx.theme();
        let sections = parent.sections();
        let height = f32::from(u16::try_from(sections.len()).unwrap_or(u16::MAX)) * SECTION_ROW;
        let mut rail = div()
            .id(SharedString::from(format!(
                "settings-subcategories-{parent:?}"
            )))
            .debug_selector(move || format!("settings-subcategories-{parent:?}"))
            .relative()
            .ml(px(16.0))
            .my(px(4.0))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(1.0))
                    .bg(theme.colors.line.to_paint()),
            );
        for &section in &sections {
            let target = SettingsPage::Section(section);
            let selected = current == target;
            let id = format!("sidebar-settings-section-{:?}-{}", parent, section.name());
            rail = rail.child(
                div()
                    .id(SharedString::from(id.clone()))
                    .debug_selector(move || id)
                    .role(gpui::Role::Button)
                    .aria_label(section.label())
                    .aria_selected(selected)
                    .focusable()
                    .tab_index(0)
                    .cursor_pointer()
                    .relative()
                    .flex_none()
                    .h(px(SECTION_ROW))
                    .pl(px(17.0))
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .text_size(theme.typography.control_text)
                    .text_color(if selected {
                        theme.colors.text.to_paint()
                    } else {
                        theme.colors.text_muted.to_paint()
                    })
                    // Children only lift their text; the triggers own the
                    // pill.
                    .hover(move |style| style.text_color(theme.colors.text.to_paint()))
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.navigate(Screen::Settings(target), cx);
                    })),
            );
        }
        if height == 0.0 {
            return div();
        }
        // One caret for the rail: it grows in on the row it lands on when the
        // category opens, then slides from step to step.
        let index = sections
            .iter()
            .position(|section| current == SettingsPage::Section(*section));
        let caret = {
            let mut caret = self.settings_caret.borrow_mut();
            if caret.parent != Some(parent) {
                *caret = SettingsCaret {
                    parent: Some(parent),
                    from: None,
                    to: index,
                    serial: caret.serial.wrapping_add(1),
                };
            } else if caret.to != index {
                caret.from = caret.to;
                caret.to = index;
                caret.serial = caret.serial.wrapping_add(1);
            }
            *caret
        };
        if let Some(to) = caret.to {
            let row_top = |index: usize| {
                f32::from(u16::try_from(index).unwrap_or(u16::MAX)).mul_add(SECTION_ROW, CARET_INSET)
            };
            let marker = div()
                .absolute()
                .left_0()
                .w(px(2.0))
                .rounded(px(1.0))
                .bg(theme.colors.text.to_paint())
                .top(px(row_top(to)))
                .h(px(CARET_HEIGHT));
            rail = rail.child(if cx.reduce_motion() {
                marker.into_any_element()
            } else {
                let from = caret.from.map(row_top);
                let target = row_top(to);
                marker
                    .with_animation(
                        SharedString::from(format!("settings-caret-{}", caret.serial)),
                        Animation::new(editor_ui::motion::MotionDuration::Quick.as_duration())
                            .with_easing(editor_ui::picker_motion::smooth_out),
                        move |marker, progress| match from {
                            Some(from) => marker.top(px((target - from).mul_add(progress, from))),
                            None => marker
                                .top(px((CARET_HEIGHT / 2.0).mul_add(1.0 - progress, target)))
                                .h(px(CARET_HEIGHT * progress))
                                .opacity(progress),
                        },
                    )
                    .into_any_element()
            });
        }
        // The category id persists while choosing its children, so expansion
        // plays once and respects the app's reduced-motion preference.
        div().child(if cx.reduce_motion() {
            rail.into_any_element()
        } else {
            rail.with_animation(
                "settings-category-expand",
                Animation::new(editor_ui::motion::MotionDuration::Quick.as_duration())
                    .with_easing(editor_ui::picker_motion::smooth_out),
                move |rail, progress| rail.h(px(height * progress)).opacity(progress),
            )
            .into_any_element()
        })
    }

    pub(crate) fn render_settings_nav(&self, page: SettingsPage, cx: &Context<Self>) -> Div {
        div().flex().flex_col().child(self.settings_pages(page, cx))
    }

    fn settings_pages(&self, page: SettingsPage, cx: &Context<Self>) -> Div {
        let theme = *cx.theme();
        let mut nav = div().flex().flex_col().gap(px(2.0));
        nav = nav.child(Self::settings_heading("Editor", &theme));
        for (target, icon) in EDITOR_PAGES {
            nav = nav.child(self.settings_link(target, icon, page, cx));
        }
        let Some(host) = self.host.clone() else {
            return nav;
        };
        nav = nav.child(Self::settings_heading(host.read(cx).name.clone(), &theme));
        for (target, icon) in HOST_PAGES {
            nav = nav.child(self.settings_link(target, icon, page, cx));
        }
        let engines: Vec<EngineStatus> = host.read(cx).engines.values().cloned().collect();
        if engines.is_empty() {
            return nav;
        }
        nav = nav.child(Self::settings_heading("Engines", &theme));
        for status in engines {
            let engine = status.engine;
            let target = SettingsPage::Engine(engine);
            let expanded = target == page.parent();
            let id = format!("sidebar-settings-engine-{engine}");
            let need = engine_need(&status).map(|need| {
                div()
                    .flex_none()
                    .text_size(theme.typography.label_text)
                    .text_color(theme.colors.text_muted.to_paint())
                    .child(need)
            });
            nav = nav.child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        Self::nav_row(
                            id.clone(),
                            id,
                            &self.nav_hover,
                            editor_ui::engine_icon(&theme, engine),
                            engine.display_name(),
                            false,
                            cx,
                        )
                        .aria_selected(expanded)
                        .aria_expanded(expanded)
                        .child(div().flex_1())
                        .children(need)
                        .child(
                            asset_glyph(if expanded {
                                AssetId::TABLER_CHEVRON_DOWN
                            } else {
                                AssetId::TABLER_CHEVRON_RIGHT
                            })
                            .size(px(12.0))
                            .text_color(theme.colors.text_muted.to_paint()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.navigate(Screen::Settings(target), cx);
                        })),
                    )
                    .when(expanded, |group| {
                        group.child(self.settings_subcategories(target, page, cx))
                    }),
            );
        }
        nav
    }
}
