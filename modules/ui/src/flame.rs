//! Varde's button material (`@utility button-flame` and its `button-green` /
//! `button-red` variants in `varde/modules/common/ui/src/global.css`),
//! ported for native fills.
//!
//! Each CSS face is a three-stop vertical gradient: bright at the top, mid at
//! 48 %, deep at the bottom. GPUI's gradient primitive takes two stops, so
//! the face is two stacked layers that meet at the mid stop, each rounded on
//! its outer corners only. The white top highlight, the dark bottom shade and
//! Varde's dark-mode `--shadow-sm` map onto box shadows. The button rim is
//! left out: fills painted with this material carry no border.

#![forbid(unsafe_code)]

use gpui::{
    BoxShadow, Div, Hsla, ParentElement as _, Pixels, Styled as _, div, linear_color_stop,
    linear_gradient, px, relative, rgb, rgb_to_hsla, rgba,
};

use crate::gradient::VERTICAL_ANGLE_DEGREES;

/// Where the mid stop sits on the vertical gradient line.
pub const MATERIAL_MID_STOP: f32 = 0.48;

/// The three stops of one material face, as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialFace {
    /// Top stop.
    pub bright: u32,
    /// Stop at [`MATERIAL_MID_STOP`].
    pub mid: u32,
    /// Bottom stop, also the base under the face.
    pub deep: u32,
}

impl MaterialFace {
    /// `button-green`.
    pub const GREEN: Self = Self {
        bright: 0x0015_803d,
        mid: 0x0008_7344,
        deep: 0x0006_5f46,
    };
    /// `button-flame`, Varde's default action face.
    pub const FLAME: Self = Self {
        bright: 0x00ee_741f,
        mid: 0x00d9_4c12,
        deep: 0x00ab_2913,
    };
    /// `button-red`.
    pub const RED: Self = Self {
        bright: 0x00ef_5555,
        mid: 0x00cc_3030,
        deep: 0x0099_1b1b,
    };

    /// The face at `level` (`0.0..=1.0`) along green → flame → red, each
    /// stop blended channel by channel.
    #[must_use]
    pub fn green_to_red(level: f32) -> Self {
        let level = if level.is_finite() {
            level.clamp(0.0, 1.0)
        } else {
            0.0
        };
        if level <= 0.5 {
            Self::GREEN.mix(Self::FLAME, level * 2.0)
        } else {
            Self::FLAME.mix(Self::RED, (level - 0.5) * 2.0)
        }
    }

    fn mix(self, other: Self, amount: f32) -> Self {
        Self {
            bright: mix_hex(self.bright, other.bright, amount),
            mid: mix_hex(self.mid, other.mid, amount),
            deep: mix_hex(self.deep, other.deep, amount),
        }
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "each 8-bit channel is blended in f32 and rounded back within 0..=255"
)]
fn mix_hex(from: u32, to: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let start = ((from >> shift) & 0xff) as f32;
        let end = ((to >> shift) & 0xff) as f32;
        (start + (end - start) * amount).round().clamp(0.0, 255.0) as u32
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn paint(color: u32) -> Hsla {
    rgb_to_hsla(rgb(color))
}

/// The face's box shadows: `inset 0 1px 0` white at 28 %,
/// `inset 0 -2px 0` `rgb(75 15 0)` at 28 %, and Varde's dark-mode
/// `--shadow-sm` (`0 2px 6px -1px` `#080808` at 28 %).
#[must_use]
pub fn material_shadows() -> Vec<BoxShadow> {
    vec![
        // 0x47 is 28 % alpha.
        BoxShadow::new(px(0.0), px(1.0), rgb_to_hsla(rgba(0xffff_ff47))).inset(),
        BoxShadow::new(px(0.0), px(-2.0), rgb_to_hsla(rgba(0x4b0f_0047))).inset(),
        BoxShadow::new(px(0.0), px(2.0), rgb_to_hsla(rgba(0x0808_0847)))
            .blur_radius(px(6.0))
            .spread_radius(px(-1.0)),
    ]
}

/// A block painted with `face` at corner `radius`. The caller sizes and
/// positions it; the gradient layers fill it behind any children.
#[must_use]
pub fn material_face(face: MaterialFace, radius: Pixels) -> Div {
    let upper = div()
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .top(px(0.0))
        .h(relative(MATERIAL_MID_STOP))
        .rounded_tl(radius)
        .rounded_tr(radius)
        .bg(linear_gradient(
            VERTICAL_ANGLE_DEGREES,
            linear_color_stop(paint(face.bright), 0.0),
            linear_color_stop(paint(face.mid), 1.0),
        ));
    let lower = div()
        .absolute()
        .left(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .h(relative(1.0 - MATERIAL_MID_STOP))
        .rounded_bl(radius)
        .rounded_br(radius)
        .bg(linear_gradient(
            VERTICAL_ANGLE_DEGREES,
            linear_color_stop(paint(face.mid), 0.0),
            linear_color_stop(paint(face.deep), 1.0),
        ));
    div()
        .relative()
        .rounded(radius)
        .bg(paint(face.deep))
        .shadow(material_shadows())
        .child(upper)
        .child(lower)
}

#[cfg(test)]
mod tests {
    use super::{MATERIAL_MID_STOP, MaterialFace, material_shadows};

    #[test]
    fn the_face_keeps_varde_geometry() {
        let shadows = material_shadows();
        assert_eq!(shadows.len(), 3);
        assert!(shadows[0].inset && shadows[1].inset && !shadows[2].inset);
        assert!((shadows[0].color.alpha - 0.28).abs() < 0.01);
        assert!((MATERIAL_MID_STOP - 0.48).abs() < f32::EPSILON);
    }

    #[test]
    fn the_level_runs_green_through_flame_to_red() {
        assert_eq!(MaterialFace::green_to_red(0.0), MaterialFace::GREEN);
        assert_eq!(MaterialFace::green_to_red(0.5), MaterialFace::FLAME);
        assert_eq!(MaterialFace::green_to_red(1.0), MaterialFace::RED);
        assert_eq!(MaterialFace::green_to_red(2.0), MaterialFace::RED);
        assert_eq!(MaterialFace::green_to_red(f32::NAN), MaterialFace::GREEN);
        let quarter = MaterialFace::green_to_red(0.25);
        assert_ne!(quarter, MaterialFace::GREEN);
        assert_ne!(quarter, MaterialFace::FLAME);
    }
}
