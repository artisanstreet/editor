#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "the Settings rail follows the carved prototype's geometry, which has no kit tokens"
)]

use std::{rc::Rc, sync::Arc, time::Instant};

use aqua_core::{DisplayItem, Role, Transform2D};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Button, Canvas, Element, Scroll},
    style::{Length, Style},
};
use artisan_api::{InstallState, SignInState};
use editor_core::{HostData, SettingsPage, SettingsSection};
use editor_ui::{
    carved::{
        Carve, RADIUS_MD, Type,
        glyph::{Icon, glyph},
    },
    chrome::card::hex,
    engine::engine_icon,
    kit::{Hover, IconSize, live},
    theme::ArtisanTheme,
};

const ITEM: f32 = 36.0;
const ITEM_GAP: f32 = 2.0;
const GROUP_GAP: f32 = 36.0;
const LAMP_WIDTH: f32 = 44.0;
const LAMP_HEIGHT: f32 = 42.0;
const INSET: f32 = 16.0;
const SLIDE_MS: f32 = 250.0;
const LABEL: Type = Type::sans(14.4, 20.0, 400);

const TOOLS: [(SettingsPage, Icon); 8] = [
    (SettingsPage::General, Icon::Gear),
    (SettingsPage::Account, Icon::User),
    (SettingsPage::About, Icon::Info),
    (SettingsPage::Hosts, Icon::Laptop),
    (SettingsPage::Threads, Icon::Messages),
    (SettingsPage::GlobalPrompt, Icon::FileText),
    (
        SettingsPage::Section(SettingsSection::McpServers),
        Icon::Plug,
    ),
    (SettingsPage::Section(SettingsSection::Skills), Icon::Tool),
];

#[derive(Debug, Default)]
pub struct NavMotion {
    page: Option<SettingsPage>,
    previous: Option<SettingsPage>,
    opened: Option<Instant>,
}

impl NavMotion {
    pub(crate) fn sync(&mut self, page: SettingsPage, now: Instant) {
        if self.page != Some(page) {
            self.previous = self.page;
            self.page = Some(page);
            self.opened = Some(now);
        }
    }

    pub(crate) fn animating(&self, now: Instant) -> bool {
        self.opened.is_some_and(|start| {
            now.saturating_duration_since(start).as_secs_f32() * 1000.0 < SLIDE_MS
        })
    }

    fn progress(&self, now: Instant, reduced: bool) -> f32 {
        if reduced {
            return 1.0;
        }
        let elapsed = self.opened.map_or(SLIDE_MS, |start| {
            now.saturating_duration_since(start).as_secs_f32() * 1000.0
        });
        editor_ui::picker_motion::smooth_out((elapsed / SLIDE_MS).min(1.0))
    }
}

fn highlighted(page: SettingsPage, current: SettingsPage) -> bool {
    page == current || (matches!(current, SettingsPage::Section(_)) && page == current.parent())
}

fn offset(page: SettingsPage, engines: &[SettingsPage]) -> Option<f32> {
    let step = ITEM + ITEM_GAP;
    let tools = TOOLS.len();
    #[expect(
        clippy::cast_precision_loss,
        reason = "the settings rail holds a dozen entries"
    )]
    let at = |index: usize| index as f32 * step;
    TOOLS
        .iter()
        .position(|(tool, _)| highlighted(*tool, page))
        .map(at)
        .or_else(|| {
            engines
                .iter()
                .position(|engine| *engine == page)
                .map(|index| at(tools) - ITEM_GAP + GROUP_GAP + at(index))
        })
}

fn lamp_source(ink: editor_ui::theme::Oklch) -> String {
    let ink = hex(ink);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 44 42"><defs><linearGradient id="g" x1="0" y1="0" x2="44" y2="0" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="{ink}" stop-opacity="0.34"/><stop offset="0.28" stop-color="{ink}" stop-opacity="0.11"/><stop offset="0.55" stop-color="{ink}" stop-opacity="0.022"/><stop offset="0.78" stop-color="{ink}" stop-opacity="0"/></linearGradient><radialGradient id="m" gradientUnits="userSpaceOnUse" cx="14.08" cy="21" r="30.8" gradientTransform="translate(14.08 21) scale(1 0.6545) translate(-14.08 -21)"><stop offset="0" stop-color="#fff" stop-opacity="1"/><stop offset="0.42" stop-color="#fff" stop-opacity="0.5"/><stop offset="0.68" stop-color="#fff" stop-opacity="0.1"/><stop offset="0.88" stop-color="#fff" stop-opacity="0"/></radialGradient><mask id="k" maskUnits="userSpaceOnUse" x="0" y="0" width="44" height="42"><rect width="44" height="42" fill="url(#m)"/></mask><filter id="b" x="-10" y="-10" width="64" height="62" filterUnits="userSpaceOnUse"><feGaussianBlur stdDeviation="2.5"/></filter><linearGradient id="v" x1="0" y1="12" x2="0" y2="30" gradientUnits="userSpaceOnUse"><stop offset="0" stop-color="{ink}" stop-opacity="0"/><stop offset="0.5" stop-color="{ink}" stop-opacity="0.55"/><stop offset="1" stop-color="{ink}" stop-opacity="0"/></linearGradient></defs><g mask="url(#k)"><rect width="44" height="42" fill="url(#g)" filter="url(#b)"/></g><rect x="0" y="12" width="1.5" height="18" rx="0.75" fill="url(#v)"/></svg>"##
    )
}

fn lamp(theme: &ArtisanTheme, shift: f32) -> Element {
    let source: Arc<str> = lamp_source(Carve::of(theme).ink).into();
    Canvas::new()
        .width(LAMP_WIDTH)
        .height(LAMP_HEIGHT)
        .style(
            Style::new()
                .absolute_at(Some(-INSET), Some((ITEM - LAMP_HEIGHT) / 2.0), None, None)
                .transform(Transform2D::translate(0.0, shift)),
        )
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![DisplayItem::GpuiSvg {
                bounds,
                source: Arc::clone(&source),
                tint: None,
                opacity: 1.0,
            }]
        })
        .build()
        .exclude_from_hit_testing()
}

struct Entry {
    page: SettingsPage,
    mark: Element,
    needs_sign_in: bool,
}

fn entry(
    theme: &ArtisanTheme,
    item: Entry,
    current: SettingsPage,
    shift: Option<f32>,
    emit: &Rc<dyn Fn(SettingsPage)>,
) -> Element {
    let carve = Carve::of(theme);
    let page = item.page;
    let selected = highlighted(page, current);
    let action = Rc::clone(emit);
    let selector = format!(
        "sidebar-settings-{}",
        page.label().to_lowercase().replace(' ', "-")
    );
    let words = LABEL
        .text(theme, carve.ink, page.label().to_owned())
        .wrap(false)
        .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
        .build();
    let mut button = Button::new()
        .width(Length::Pct(100.0))
        .height(ITEM)
        .gap(10.0)
        .radius(RADIUS_MD)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .padding_insets(aqua_core::Insets {
                    left: 10.0,
                    right: 10.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(live::layer(Hover::Neutral, theme, RADIUS_MD, false))
        .child(item.mark)
        .child(words);
    if item.needs_sign_in {
        button = button.child(
            UiBox::new()
                .width(7.0)
                .height(7.0)
                .style(
                    Style::new()
                        .radius(Length::Px(3.5))
                        .background(carve.warn.to_paint())
                        .flex_shrink(0.0),
                )
                .build(),
        );
    }
    let row = live::track(
        button
            .on_activate(move || action(page))
            .build()
            .with_key(selector.clone())
            .role(Role::Button)
            .aria_label(page.label())
            .cursor(aqua_core::CursorIcon::Pointer)
            .focus_visible_style(editor_ui::carved::button::focus(theme)),
    )
    .debug_selector(selector)
    .aria_selected(selected);
    UiBox::new()
        .width(Length::Pct(100.0))
        .child(row)
        .children(shift.filter(|_| selected).map(|shift| lamp(theme, shift)))
        .build()
}

fn needs_sign_in(host: &HostData, status: &artisan_api::EngineStatus) -> bool {
    let key = status.engine.enabled_setting();
    let enabled = artisan_api::switch_decl(key).switch(host.setting(key)).on();
    enabled
        && matches!(status.install, InstallState::Installed { .. })
        && matches!(
            status.sign_in,
            SignInState::SignedOut | SignInState::Failed { .. }
        )
}

pub fn build(
    host: Option<&HostData>,
    current: SettingsPage,
    theme: &ArtisanTheme,
    motion: &NavMotion,
    now: Instant,
    reduced: bool,
    emit: &Rc<dyn Fn(SettingsPage)>,
) -> Element {
    let carve = Carve::of(theme);
    let engines: Vec<(SettingsPage, Entry)> = host
        .into_iter()
        .flat_map(|host| host.engines.values().map(move |status| (host, status)))
        .map(|(host, status)| {
            let page = SettingsPage::Engine(status.engine);
            (
                page,
                Entry {
                    page,
                    mark: IconSize::Md.glyph(
                        engine_icon(theme, status.engine),
                        carve.ink.to_reference_paint(),
                    ),
                    needs_sign_in: needs_sign_in(host, status),
                },
            )
        })
        .collect();
    let engine_pages: Vec<SettingsPage> = engines.iter().map(|(page, _)| *page).collect();
    let progress = motion.progress(now, reduced);
    let shift = offset(current, &engine_pages).map(|to| {
        let from = motion
            .previous
            .and_then(|previous| offset(previous, &engine_pages))
            .unwrap_or(to);
        (from - to) * (1.0 - progress)
    });
    let group = |entries: Vec<Entry>| {
        entries
            .into_iter()
            .fold(
                UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .style(Style::new().gap(ITEM_GAP)),
                |column, item| column.child(entry(theme, item, current, shift, emit)),
            )
            .build()
    };
    let tools = TOOLS
        .iter()
        .map(|&(page, icon)| Entry {
            page,
            mark: glyph(icon, 16.0, carve.ink_dim),
            needs_sign_in: false,
        })
        .collect();
    let rail = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(
            Style::new()
                .gap(GROUP_GAP)
                .padding_insets(aqua_core::Insets {
                    left: INSET,
                    right: 4.0,
                    top: 0.0,
                    bottom: 0.0,
                }),
        )
        .child(group(tools))
        .children(
            (!engines.is_empty())
                .then(|| group(engines.into_iter().map(|(_, entry)| entry).collect())),
        )
        .build();
    Scroll::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(rail)
        .build()
        .with_key("settings-dialog-nav")
}
