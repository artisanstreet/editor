use crate::core::{ConnectionStatus, Core, HostData, HostId, HostInstance};
use crate::ui::theme::ArtisanTheme;
use aqua_core::{DisplayItem, Rect, Role};
use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
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

#[derive(Default)]
pub struct WordmarkLoader {
    mounted: Option<Instant>,
}

impl WordmarkLoader {
    pub fn reset(&mut self) {
        self.mounted = None;
    }

    pub fn build(
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

    pub fn deadline(&self, now: Instant, reduced_motion: bool) -> Option<Instant> {
        if reduced_motion {
            return None;
        }
        self.mounted
            .map(|mounted| mounted + MARK_DELAY)
            .filter(|deadline| *deadline > now)
    }

    pub fn animating(&self, now: Instant, reduced_motion: bool) -> bool {
        !reduced_motion
            && self
                .mounted
                .is_some_and(|mounted| now >= mounted + MARK_DELAY)
    }
}

#[derive(Default)]
pub struct ConnectionPresenter {
    host: Option<(HostId, HostInstance)>,
    phase: Option<(bool, Instant)>,
    mark: WordmarkLoader,
    starts: Rc<RefCell<Vec<(HostId, HostInstance)>>>,
}

impl ConnectionPresenter {
    pub fn reset(&mut self) {
        self.host = None;
        self.phase = None;
        self.mark.reset();
        self.starts.borrow_mut().clear();
    }

    pub fn waiting(&mut self, theme: ArtisanTheme, now: Instant, reduced_motion: bool) -> Element {
        self.mark
            .build(theme, now, reduced_motion, "forge-connecting")
    }

    pub fn dispatch(&mut self, core: &mut Core) {
        for (id, instance) in std::mem::take(&mut *self.starts.borrow_mut()) {
            if self.host.as_ref() == Some(&(id.clone(), instance)) {
                if let Some(host) = core.host_mut_if_instance(&id, instance) {
                    host.start_wsl();
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
            self.reset();
            self.host = Some(identity.clone());
        }
        let code = match &host.status {
            ConnectionStatus::Live => {
                self.phase = None;
                self.mark.reset();
                return None;
            }
            ConnectionStatus::Connecting => None,
            ConnectionStatus::Offline { code, .. } => *code,
        };
        let failure = match code {
            Some(artisan_api::ErrorCode::Unauthorized) => Some((
                "Could not authenticate with Forge",
                "The paired credential was rejected. Add a fresh Forge invitation to pair this client again.",
            )),
            Some(artisan_api::ErrorCode::VersionMismatch) => Some((
                "Forge and Editor versions differ",
                "Update Forge or the Editor so both use the same protocol.",
            )),
            _ => None,
        };
        if let Some((title, detail)) = failure {
            self.phase = None;
            self.mark.reset();
            let content = UiBox::new()
                .layout("column")
                .style(Style::new().align(AlignItems::CENTER))
                .child(label(
                    &theme,
                    title,
                    28.0,
                    theme.colors.text.to_reference_text_color(),
                ))
                .child(
                    UiBox::new()
                        .style(Style::new().max_width(Length::Px(440.0)).margin_insets(
                            aqua_core::Insets {
                                top: 8.0,
                                ..Default::default()
                            },
                        ))
                        .child(label(
                            &theme,
                            detail,
                            15.0,
                            theme.colors.text_muted.to_reference_text_color(),
                        ))
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
            .gap(40.0)
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
                let starts = Rc::clone(&self.starts);
                let action = crate::ui::button::Button::text("start-wsl-forge", theme, label)
                    .variant(crate::ui::button::ButtonVariant::Primary)
                    .disabled(wsl.starting)
                    .debug_selector("start-wsl-forge")
                    .on_activate(move |_| starts.borrow_mut().push(identity.clone()))
                    .build();
                (detail, Some(action))
            } else if reached {
                ("Connection dropped — getting you back…".to_owned(), None)
            } else {
                ("Connecting to Artisan Forge…".to_owned(), None)
            };
            let first = "This is taking more time than expected…";
            let typography = text_style(&theme, 14.0, FontWeight::NORMAL);
            let color = theme.colors.text_muted.to_reference_text_color();
            let color = [
                color.hue_degrees,
                color.saturation,
                color.lightness,
                color.alpha,
            ];
            let spans = vec![
                StyledSpan::new(first, text_style(&theme, 14.0, FontWeight::MEDIUM))
                    .gpui_source_color(color),
                StyledSpan::new(format!(" {detail}"), typography).gpui_source_color(color),
            ];
            let mut line = UiBox::new()
                .layout("column")
                .gap(12.0)
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .max_width(Length::Px(440.0)),
                )
                .child(Text::new(format!("{first} {detail}")).spans(spans).build());
            if let Some(error) = host.wsl.as_ref().and_then(|wsl| wsl.error.as_ref()) {
                line = line.child(label(
                    &theme,
                    &format!("Couldn't start it: {error}"),
                    14.0,
                    theme.colors.destructive_text.to_reference_text_color(),
                ));
            }
            if let Some(action) = action {
                line = line.child(action);
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

    pub fn animating(&self, now: Instant, reduced_motion: bool) -> bool {
        !reduced_motion
            && (self.mark.animating(now, false)
                || self.phase.is_some_and(|(_, since)| {
                    now >= since + REASSURANCE_DELAY
                        && now < since + REASSURANCE_DELAY + REASSURANCE_FADE
                }))
    }
}

fn text_style(theme: &ArtisanTheme, size: f32, weight: FontWeight) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: size,
        weight,
        ..TextStyle::default()
    }
}

fn label(theme: &ArtisanTheme, value: &str, size: f32, color: aqua_core::GpuiTextColor) -> Element {
    Text::new(value.to_owned())
        .text_style(text_style(theme, size, FontWeight::NORMAL))
        .gpui_source_color(color)
        .build()
}
