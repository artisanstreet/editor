use crate::core::client::ForgeFault;
use crate::core::{ConnectionStatus, Core, HostData, HostId, HostInstance};
use crate::ui::kit::{Button, ButtonTone, Space, Spaced, TextRole};
use crate::ui::theme::ArtisanTheme;
use aqua_core::{DisplayItem, LiveRegion, Rect, Role};
use aqua_text::StyledSpan;
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Canvas, Element, Text},
    style::{Length, Style},
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

const MARK_DELAY: Duration = Duration::from_millis(500);
const MARK_FADE: Duration = Duration::from_millis(200);
const REASSURANCE_DELAY: Duration = Duration::from_secs(5);
const REASSURANCE_FADE: Duration = Duration::from_millis(500);
const TEXT_WIDTH: f32 = 440.0;

#[derive(Default)]
pub struct WordmarkLoader {
    mounted: Option<Instant>,
}

impl WordmarkLoader {
    pub(crate) fn reset(&mut self) {
        self.mounted = None;
    }

    pub(crate) fn build(
        &mut self,
        theme: ArtisanTheme,
        now: Instant,
        reduced_motion: bool,
        key: &'static str,
    ) -> Element {
        let mounted = *self.mounted.get_or_insert(now);
        let started = mounted + MARK_DELAY;
        let height = 48.0 * 2.2515;
        let width = height * (136.675 / 225.15);
        let source: std::sync::Arc<str> =
            editor_assets::get(editor_assets::AssetId::ARTISAN_WORDMARK_STACKED)
                .source
                .into();
        let foreground = theme.colors.text;
        let muted = theme.colors.text_muted;
        Canvas::new()
            .width(width)
            .height(height)
            .on_paint(move |bounds| {
                let vector = |color| DisplayItem::GpuiSvg {
                    bounds,
                    source: source.clone(),
                    tint: Some(color),
                    opacity: 1.0,
                };
                if reduced_motion {
                    return vec![vector(foreground.to_reference_paint())];
                }
                let elapsed = Instant::now().checked_duration_since(started);
                let Some(elapsed) = elapsed else {
                    return Vec::new();
                };
                let opacity = (elapsed.as_secs_f32() / MARK_FADE.as_secs_f32()).clamp(0.0, 1.0);
                let progress = (elapsed.as_secs_f32() / 3.0).fract();
                let band = bounds.w * 0.6;
                let slice_width = band / 9.0;
                let leading = (bounds.w + band).mul_add(progress, -band);
                let mut items = vec![vector(muted.fade(opacity).to_reference_paint())];
                for slice in 0..9 {
                    let left = slice_width.mul_add(slice as f32, leading);
                    let position = (slice as f32 + 0.5) / 9.0;
                    let strength = (position * std::f32::consts::TAU).cos().mul_add(-0.5, 0.5);
                    items.push(DisplayItem::PushGpuiContentMask {
                        bounds: Rect {
                            x: bounds.x + left,
                            y: bounds.y,
                            w: slice_width,
                            h: bounds.h,
                        },
                    });
                    items.push(vector(
                        foreground.fade(opacity * strength).to_reference_paint(),
                    ));
                    items.push(DisplayItem::PopClip);
                }
                items
            })
            .build()
            .with_key(key)
            .role(Role::ProgressIndicator)
            .aria_label("Loading")
    }

    pub(crate) fn deadline(&self, now: Instant, reduced_motion: bool) -> Option<Instant> {
        if reduced_motion {
            return None;
        }
        self.mounted
            .map(|mounted| mounted + MARK_DELAY)
            .filter(|deadline| *deadline > now)
    }

    pub(crate) fn animating(&self, now: Instant, reduced_motion: bool) -> bool {
        !reduced_motion
            && self
                .mounted
                .is_some_and(|mounted| now >= mounted + MARK_DELAY)
    }
}

#[must_use]
pub fn lost(status: &ConnectionStatus) -> Option<Element> {
    match status {
        ConnectionStatus::Live | ConnectionStatus::Connecting => None,
        ConnectionStatus::Reconnecting { .. } | ConnectionStatus::Offline { .. } => Some(
            UiBox::new()
                .width(0.0)
                .height(0.0)
                .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
                .build()
                .exclude_from_hit_testing()
                .with_key("forge-connection-lost")
                .role(Role::Text)
                .aria_label("Lost the connection to Forge. Reconnecting…")
                .aria_live(LiveRegion::Polite),
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    StartWsl,
    RetryNow,
    ManageForges,
}

type Commands = Rc<RefCell<Vec<(HostId, HostInstance, Command)>>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    NewThread(HostId),
    Thread(HostId, artisan_api::ThreadId),
}

#[derive(Default)]
pub struct ConnectionPresenter {
    owner: Option<Owner>,
    host: Option<(HostId, HostInstance)>,
    phase: Option<(bool, Instant)>,
    mark: WordmarkLoader,
    commands: Commands,
    last_failure: Option<String>,
}

fn command_button(
    key: &'static str,
    theme: ArtisanTheme,
    tone: ButtonTone,
    label: impl Into<String>,
    commands: &Commands,
    identity: &(HostId, HostInstance),
    command: Command,
) -> Button {
    let commands = Rc::clone(commands);
    let (host, instance) = identity.clone();
    Button::text(key, theme, tone, label)
        .debug_selector(key)
        .on_activate(move |_| {
            commands
                .borrow_mut()
                .push((host.clone(), instance, command));
        })
}

impl ConnectionPresenter {
    pub(crate) fn reset(&mut self) {
        self.owner = None;
        self.host = None;
        self.phase = None;
        self.mark.reset();
        self.commands.borrow_mut().clear();
        self.last_failure = None;
    }

    pub fn hold(&mut self, owner: Option<Owner>) {
        if self.owner != owner {
            self.reset();
            self.owner = owner;
        }
    }

    pub fn waiting(&mut self, theme: ArtisanTheme, now: Instant, reduced_motion: bool) -> Element {
        self.mark
            .build(theme, now, reduced_motion, "forge-connecting")
    }

    pub fn dispatch(&mut self, core: &mut Core) {
        for (id, instance, command) in std::mem::take(&mut *self.commands.borrow_mut()) {
            if self.host.as_ref() != Some(&(id.clone(), instance)) {
                continue;
            }
            match command {
                Command::StartWsl => {
                    if let Some(host) = core.host_mut_if_instance(&id, instance) {
                        host.start_wsl();
                    }
                }
                Command::RetryNow => {
                    if let Some(host) = core.host_if_instance(&id, instance) {
                        host.retry_now();
                    }
                }
                Command::ManageForges => {
                    core.navigate(crate::core::Screen::Settings(
                        crate::core::SettingsPage::Hosts,
                    ));
                }
            }
        }
    }

    pub fn build(
        &mut self,
        host: &HostData,
        theme: ArtisanTheme,
        now: Instant,
        reduced_motion: bool,
    ) -> Option<Element> {
        let identity = (host.id.clone(), host.instance());
        if self.host.as_ref() != Some(&identity) {
            self.phase = None;
            self.mark.reset();
            self.commands.borrow_mut().clear();
            self.host = Some(identity.clone());
        }
        let fault = match &host.status {
            ConnectionStatus::Live => {
                self.phase = None;
                self.mark.reset();
                self.last_failure = None;
                return None;
            }
            ConnectionStatus::Connecting | ConnectionStatus::Reconnecting { .. } => None,
            ConnectionStatus::Offline { fault, reason, .. } => {
                self.last_failure = Some(reason.clone());
                *fault
            }
        };
        let failure = match fault {
            Some(ForgeFault::Unauthorized) => Some((
                "Could not authenticate with Forge",
                "The paired credential was rejected. Add a fresh Forge invitation to pair this client again.",
            )),
            Some(ForgeFault::Incompatible) => Some((
                "Forge and Editor versions differ",
                "Update Forge or the Editor so both use the same protocol.",
            )),
            Some(
                ForgeFault::Unrecognised
                | ForgeFault::Unreachable
                | ForgeFault::Unreadable
                | ForgeFault::Failed,
            )
            | None => None,
        };
        if let Some((title, detail)) = failure {
            self.phase = None;
            self.mark.reset();
            let content = UiBox::new()
                .layout("column")
                .style(Style::new().align(AlignItems::CENTER))
                .spaced(Space::S8)
                .child(TextRole::Heading.text(&theme, title).build())
                .child(
                    UiBox::new()
                        .style(Style::new().max_width(Length::Px(TEXT_WIDTH)))
                        .child(label(&theme, detail, theme.colors.text_muted))
                        .build(),
                )
                .child(
                    command_button(
                        "manage-forges",
                        theme,
                        ButtonTone::Neutral,
                        "Forges",
                        &self.commands,
                        &identity,
                        Command::ManageForges,
                    )
                    .build(),
                )
                .build();
            return Some(content.debug_selector("new-thread-status"));
        }
        let reached = host.forge_version.is_some();
        let since = match self.phase {
            Some((phase, since)) if phase == reached => since,
            _ => {
                self.phase = Some((reached, now));
                now
            }
        };
        let mark = self
            .mark
            .build(theme, now, reduced_motion, "forge-connecting");
        let mut section = UiBox::new()
            .layout("column")
            .spaced(Space::S32)
            .style(Style::new().align(AlignItems::CENTER))
            .child(mark);
        if now.saturating_duration_since(since) >= REASSURANCE_DELAY {
            let (detail, action) = if let Some(wsl) = &host.wsl {
                let (detail, label) = if wsl.running {
                    (
                        format!("Forge isn't running in {}.", wsl.distro),
                        if wsl.starting {
                            "Starting Forge…".to_owned()
                        } else {
                            "Start Forge".to_owned()
                        },
                    )
                } else {
                    (
                        format!(
                            "WSL isn't running, so Forge in {} can't be reached.",
                            wsl.distro
                        ),
                        if wsl.starting {
                            format!("Starting {}…", wsl.distro)
                        } else {
                            format!("Start {}", wsl.distro)
                        },
                    )
                };
                let action = command_button(
                    "start-wsl-forge",
                    theme,
                    ButtonTone::Primary,
                    label,
                    &self.commands,
                    &identity,
                    Command::StartWsl,
                )
                .disabled(wsl.starting)
                .build();
                (detail, vec![action])
            } else {
                let detail = if reached {
                    "Connection dropped — getting you back…"
                } else {
                    "Connecting to Artisan Forge…"
                };
                let actions = self.last_failure.is_some().then(|| {
                    vec![
                        command_button(
                            "retry-forge",
                            theme,
                            ButtonTone::Primary,
                            "Try again now",
                            &self.commands,
                            &identity,
                            Command::RetryNow,
                        )
                        .build(),
                        command_button(
                            "manage-forges",
                            theme,
                            ButtonTone::Neutral,
                            "Forges",
                            &self.commands,
                            &identity,
                            Command::ManageForges,
                        )
                        .build(),
                    ]
                });
                (detail.to_owned(), actions.unwrap_or_default())
            };
            let first = "This is taking more time than expected…";
            let typography = TextRole::Label.style(&theme);
            let color = theme.colors.text_muted.to_reference_text_color();
            let color = [
                color.hue_degrees,
                color.saturation,
                color.lightness,
                color.alpha,
            ];
            let spans = vec![
                StyledSpan::new(first, TextRole::LabelStrong.style(&theme))
                    .gpui_source_color(color),
                StyledSpan::new(format!(" {detail}"), typography).gpui_source_color(color),
            ];
            let mut line = UiBox::new()
                .layout("column")
                .spaced(Space::S12)
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .max_width(Length::Px(TEXT_WIDTH)),
                )
                .child(Text::new(format!("{first} {detail}")).spans(spans).build());
            if let Some(error) = host.wsl.as_ref().and_then(|wsl| wsl.error.as_ref()) {
                line = line.child(label(
                    &theme,
                    &format!("Couldn't start it: {error}"),
                    theme.colors.destructive_text,
                ));
            }
            if host.wsl.is_none()
                && let Some(reason) = &self.last_failure
            {
                line = line.child(label(
                    &theme,
                    &format!("Last attempt: {reason}"),
                    theme.colors.text_muted,
                ));
            }
            if !action.is_empty() {
                line = line.child(
                    UiBox::new()
                        .layout("row")
                        .spaced(Space::S8)
                        .children(action)
                        .build(),
                );
            }
            let elapsed = now
                .saturating_duration_since(since + REASSURANCE_DELAY)
                .as_secs_f32()
                / REASSURANCE_FADE.as_secs_f32();
            let t = elapsed.clamp(0.0, 1.0);
            let opacity = if reduced_motion {
                1.0
            } else if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
            };
            section = section.child(line.opacity(opacity).build());
        }
        Some(section.build().debug_selector("new-thread-status"))
    }

    pub fn deadline(&self, now: Instant, reduced_motion: bool) -> Option<Instant> {
        let phase = self
            .phase
            .map(|(_, since)| since + REASSURANCE_DELAY)
            .filter(|deadline| *deadline > now);
        match (phase, self.mark.deadline(now, reduced_motion)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    pub(crate) fn animating(&self, now: Instant, reduced_motion: bool) -> bool {
        !reduced_motion
            && (self.mark.animating(now, false)
                || self.phase.is_some_and(|(_, since)| {
                    now >= since + REASSURANCE_DELAY
                        && now < since + REASSURANCE_DELAY + REASSURANCE_FADE
                }))
    }
}

fn label(theme: &ArtisanTheme, value: &str, ink: crate::ui::theme::Oklch) -> Element {
    TextRole::Label
        .text(theme, value.to_owned())
        .gpui_source_color(ink.to_reference_text_color())
        .wrap(true)
        .build()
}
