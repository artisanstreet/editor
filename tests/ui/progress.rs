//! Behavioral coverage for the static native GPUI progress primitive.
//!
//! The recipe assertions below compare [`ProgressStyle::resolve`] output and
//! rendered bounds against independently transcribed legacy facts (literal
//! pixels, literal OKLCH source colors, literal fill shares), never against
//! the primitive's own derivation, so the coverage cannot pass circularly.
//! Bounds tests pin layout geometry only: pixel paint, platform
//! accessibility, and animation are not claimable from them (see the
//! primitive's module documentation).

use artisan_ui::progress::{ProgressFraction, ProgressStyle, progress, progress_indicator};
use artisan_ui::theme::{ArtisanTheme, Oklch, SurfaceStep, ThemeMode};
use gpui::{
    Context, Div, InteractiveElement, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px,
};

const HOST_SELECTOR: &str = "progress-host";
const TRACK_SELECTOR: &str = "progress-track-under-test";
const EMPTY_FILL_SELECTOR: &str = "progress-empty-fill";
const PARTIAL_FILL_SELECTOR: &str = "progress-partial-fill";
const FULL_FILL_SELECTOR: &str = "progress-full-fill";
const OVERSHOOT_FILL_SELECTOR: &str = "progress-overshoot-fill";
const UNDERSHOOT_FILL_SELECTOR: &str = "progress-undershoot-fill";

/// Legacy facts transcribed verbatim from `progress.svelte`
/// (`h-2 rounded-full bg-primary/20` over an opaque `bg-primary`).
const LEGACY_TRACK_HEIGHT_PX: f32 = 8.0;
const LEGACY_PILL_RADIUS_PX: f32 = 9999.0;
const LEGACY_TRACK_ALPHA: f32 = 0.2;

/// The reached usage-details share family, on a 320 px host basis.
const HOST_WIDTH_PX: f32 = 320.0;
const PARTIAL_SHARE: f32 = 0.25;

#[test]
fn progress_fraction_saturates_out_of_domain_inputs() {
    assert_eq!(ProgressFraction::default(), ProgressFraction::EMPTY);
    assert_eq!(
        ProgressFraction::new(f32::NAN),
        ProgressFraction::EMPTY,
        "non-finite readings must not poison layout"
    );
    assert_eq!(
        ProgressFraction::new(f32::NEG_INFINITY),
        ProgressFraction::EMPTY
    );
    assert_eq!(
        ProgressFraction::new(f32::INFINITY),
        ProgressFraction::EMPTY,
        "no non-finite reading may read as full"
    );
    assert_eq!(ProgressFraction::new(-0.25), ProgressFraction::EMPTY);
    assert_eq!(ProgressFraction::new(0.0), ProgressFraction::EMPTY);
    assert_eq!(
        ProgressFraction::new(PARTIAL_SHARE).value().to_bits(),
        PARTIAL_SHARE.to_bits(),
        "in-domain shares must pass through unchanged"
    );
    assert_eq!(ProgressFraction::new(1.0), ProgressFraction::FULL);
    assert_eq!(ProgressFraction::new(1.75), ProgressFraction::FULL);
}

#[test]
fn progress_style_pins_exact_audited_geometry() {
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let style = ProgressStyle::resolve(theme);

        assert_eq!(style.track_height, px(LEGACY_TRACK_HEIGHT_PX));
        assert_eq!(
            style.track_height,
            theme.spacing.steps(2.0),
            "the legacy `h-2` is two 4 px spacing steps"
        );
        assert_eq!(
            style.corner_radius,
            px(LEGACY_PILL_RADIUS_PX),
            "the legacy pill is gpui's `rounded_full` token"
        );
    }
}

#[test]
fn progress_paint_resolves_from_the_exact_mode_primary_token() {
    // Guard the token mapping itself before deriving expectations from it:
    // light `--primary` is `--surface-900`; dark `--primary` is
    // `--surface-200` (`theme.css`; INVENTORY §5.1).
    let light_theme = ArtisanTheme::for_mode(ThemeMode::Light);
    let dark_theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    assert_eq!(light_theme.colors.primary, SurfaceStep::S900.oklch());
    assert_eq!(dark_theme.colors.primary, SurfaceStep::S200.oklch());

    let light = ProgressStyle::resolve(light_theme);
    let dark = ProgressStyle::resolve(dark_theme);

    // Transcribed independently from the ramp: light `oklch(0.21 0.006
    // 285.885)`, dark `oklch(0.92 0.004 286.32)`. The legacy `bg-primary/20`
    // track carries that primary at exactly 20% alpha over the opaque
    // `bg-primary` fill.
    let light_primary = Oklch::new(0.21, 0.006, 285.885);
    let dark_primary = Oklch::new(0.92, 0.004, 286.32);

    assert_eq!(
        light.track_color,
        light_primary.with_alpha(LEGACY_TRACK_ALPHA).to_paint()
    );
    assert_eq!(
        dark.track_color,
        dark_primary.with_alpha(LEGACY_TRACK_ALPHA).to_paint()
    );
    assert_eq!(
        light.track_color.alpha.to_bits(),
        LEGACY_TRACK_ALPHA.to_bits(),
        "the track alpha must be exactly 20%"
    );
    assert_eq!(
        dark.track_color.alpha.to_bits(),
        LEGACY_TRACK_ALPHA.to_bits()
    );

    assert_eq!(light.fill_color, light_primary.to_paint());
    assert_eq!(dark.fill_color, dark_primary.to_paint());
    assert_ne!(
        light.fill_color, dark.fill_color,
        "light and dark primary paints must differ"
    );
    assert_ne!(
        light.track_color, light.fill_color,
        "the /20 track must differ from its opaque fill"
    );
}

#[test]
fn compile_only_constructor_chains_styled_refinements() {
    // Compile-only API-shape evidence, mirroring the badge coverage: one
    // resolved recipe feeds both constructors and later refinements chain
    // onto the returned Divs.
    let style = ProgressStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark));
    let track = progress(style, ProgressFraction::FULL)
        .max_w(px(96.0))
        .h(px(4.0));
    let fill = progress_indicator(style, ProgressFraction::EMPTY).w(px(12.0));
    let _ = (track, fill);
}

/// A reached-shaped usage-details body: one full-width bar in a fixed host.
struct TrackProbe {
    style: ProgressStyle,
}

impl Render for TrackProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(HOST_WIDTH_PX))
            .h(px(48.0))
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(
                progress(self.style, ProgressFraction::FULL)
                    .debug_selector(|| TRACK_SELECTOR.to_string()),
            )
    }
}

/// One indicator per audited share, each laid out in an 8 px-tall,
/// full-width row: exactly the containing basis (`h-2` × `w-full`) the
/// composite track supplies, so these bounds are the bounded fill geometry
/// itself rather than helper-host behavior.
struct FillSharesProbe {
    style: ProgressStyle,
}

impl FillSharesProbe {
    fn row(&self, selector: &'static str, fill: ProgressFraction) -> Div {
        div().w_full().h(px(LEGACY_TRACK_HEIGHT_PX)).child(
            progress_indicator(self.style, fill).debug_selector(move || selector.to_string()),
        )
    }
}

impl Render for FillSharesProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .w(px(HOST_WIDTH_PX))
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(self.row(EMPTY_FILL_SELECTOR, ProgressFraction::EMPTY))
            .child(self.row(PARTIAL_FILL_SELECTOR, ProgressFraction::new(PARTIAL_SHARE)))
            .child(self.row(FULL_FILL_SELECTOR, ProgressFraction::FULL))
            .child(self.row(OVERSHOOT_FILL_SELECTOR, ProgressFraction::new(42.0)))
            .child(self.row(UNDERSHOOT_FILL_SELECTOR, ProgressFraction::new(-7.0)))
    }
}

/// A caller-chained max-width refinement overriding the full-width recipe
/// default, exercising the later-values-win contract.
struct CappedTrackProbe {
    style: ProgressStyle,
}

impl Render for CappedTrackProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            progress(self.style, ProgressFraction::FULL)
                .max_w(px(96.0))
                .debug_selector(|| TRACK_SELECTOR.to_string()),
        )
    }
}

#[gpui::test]
fn track_is_8px_tall_and_fills_the_host_width(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| TrackProbe {
        style: ProgressStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let track = cx
        .debug_bounds(TRACK_SELECTOR)
        .expect("track must paint inspectable bounds");

    assert_eq!(track.size.height, px(LEGACY_TRACK_HEIGHT_PX));
    assert_eq!(host.size.width, px(HOST_WIDTH_PX));
    assert_eq!(track.size.width, host.size.width);
    assert_eq!(track.origin, host.origin, "the bar starts flush");
}

#[gpui::test]
fn indicators_render_the_audited_shares_on_an_8px_basis(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| FillSharesProbe {
        style: ProgressStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Light)),
    });

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let empty = cx
        .debug_bounds(EMPTY_FILL_SELECTOR)
        .expect("empty fill must paint inspectable bounds");
    let partial = cx
        .debug_bounds(PARTIAL_FILL_SELECTOR)
        .expect("partial fill must paint inspectable bounds");
    let full = cx
        .debug_bounds(FULL_FILL_SELECTOR)
        .expect("full fill must paint inspectable bounds");
    let overshoot = cx
        .debug_bounds(OVERSHOOT_FILL_SELECTOR)
        .expect("overshoot fill must paint inspectable bounds");
    let undershoot = cx
        .debug_bounds(UNDERSHOOT_FILL_SELECTOR)
        .expect("undershoot fill must paint inspectable bounds");

    // Empty and below-range shares paint no width; a quarter share is a
    // quarter of the shared basis; full and finite above-range shares span
    // the whole basis.
    assert_eq!(empty.size.width, px(0.0));
    assert_eq!(undershoot.size.width, px(0.0));
    assert_eq!(
        partial.size.width,
        px(HOST_WIDTH_PX * PARTIAL_SHARE),
        "a quarter share is a quarter of the basis"
    );
    assert_eq!(full.size.width, host.size.width);
    assert_eq!(overshoot.size.width, host.size.width);

    for fill in [empty, partial, full, overshoot, undershoot] {
        assert_eq!(
            fill.size.height,
            px(LEGACY_TRACK_HEIGHT_PX),
            "fills span their full track"
        );
        assert_eq!(
            fill.origin.x, host.origin.x,
            "the legacy shift leaves the visible share flush left"
        );
    }
}

#[gpui::test]
fn caller_max_width_refinement_caps_the_track(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|_, _| CappedTrackProbe {
        style: ProgressStyle::resolve(ArtisanTheme::for_mode(ThemeMode::Dark)),
    });

    let track = cx
        .debug_bounds(TRACK_SELECTOR)
        .expect("track must paint inspectable bounds");

    // The caller's chained refinement wins over the full-width recipe
    // default while the audited height holds.
    assert_eq!(track.size.width, px(96.0));
    assert_eq!(track.size.height, px(LEGACY_TRACK_HEIGHT_PX));
}
