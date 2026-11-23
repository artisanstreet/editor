use std::rc::Rc;
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct WorkingPulse {
    started: Rc<std::cell::Cell<Option<u32>>>,
}

impl WorkingPulse {
    pub fn wrap(
        &self,
        child: aqua_ui::element::Element,
        reduced_motion: bool,
    ) -> aqua_ui::element::Element {
        use aqua_core::DisplayItem;
        use aqua_ui::{
            element::{Canvas, Element},
            style::Style,
        };
        let started = Rc::clone(&self.started);
        let prepaint = Rc::clone(&started);
        let before = Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_prepaint(move |_, _| {
                if prepaint.get().is_none() {
                    prepaint.set(Some(crate::ui::clock::pulse_ms()));
                }
            })
            .on_paint(move |_| {
                if reduced_motion {
                    Vec::new()
                } else {
                    vec![DisplayItem::PushGpuiQuadPulse(
                        aqua_core::scene::GpuiQuadPulse {
                            start_ms: started.get().unwrap_or(0),
                            period_ms: 1_600,
                            min_opacity: 0.4,
                        },
                    )]
                }
            })
            .build()
            .exclude_from_hit_testing();
        let after = Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
            .on_paint(move |_| {
                if reduced_motion {
                    Vec::new()
                } else {
                    vec![DisplayItem::PopGpuiQuadPulse]
                }
            })
            .build()
            .exclude_from_hit_testing();
        Element::fragment(vec![before, child, after])
    }
}

pub fn pulsing(
    child: aqua_ui::element::Element,
    pulse: aqua_core::scene::GpuiQuadPulse,
    reduced_motion: bool,
) -> aqua_ui::element::Element {
    use aqua_core::DisplayItem;
    use aqua_ui::{
        element::{Canvas, Element},
        style::Style,
    };
    if reduced_motion {
        return child;
    }
    let edge = || Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    let before = Canvas::new()
        .style(edge())
        .on_paint(move |_| vec![DisplayItem::PushGpuiQuadPulse(pulse)])
        .build()
        .exclude_from_hit_testing();
    let after = Canvas::new()
        .style(edge())
        .on_paint(|_| vec![DisplayItem::PopGpuiQuadPulse])
        .build()
        .exclude_from_hit_testing();
    Element::fragment(vec![before, child, after])
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionDuration {
    Quick,
    Fast,
    Medium,
}

impl MotionDuration {
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        match self {
            Self::Quick => Duration::from_millis(150),
            Self::Fast => Duration::from_millis(250),
            Self::Medium => Duration::from_millis(350),
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[expect(
    clippy::enum_variant_names,
    reason = "the names are the CSS easing curves these implement"
)]
pub enum MotionCurve {
    SmoothOut,
    EaseOut,
    EaseInOut,
}

impl MotionCurve {
    #[must_use]
    pub fn sample(self, progress: f64) -> f64 {
        if progress <= 0.0 {
            return 0.0;
        }
        if progress >= 1.0 {
            return 1.0;
        }

        let (x1, y1, x2, y2) = self.control_points();
        let target_x = progress;
        let mut lower = 0.0_f64;
        let mut upper = 1.0_f64;
        for _ in 0..20 {
            let t = lower.midpoint(upper);
            if cubic_axis(t, x1, x2) < target_x {
                lower = t;
            } else {
                upper = t;
            }
        }

        cubic_axis(lower.midpoint(upper), y1, y2)
    }
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the curve's 0..=1 output fits an f32"
    )]
    pub fn sample_f32(self, progress: f64) -> f32 {
        self.sample(progress) as f32
    }

    const fn control_points(self) -> (f64, f64, f64, f64) {
        match self {
            Self::SmoothOut => (0.22, 1.0, 0.36, 1.0),
            Self::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Self::EaseInOut => (0.42, 0.0, 0.58, 1.0),
        }
    }
}
#[must_use]
pub fn smooth_out(progress: f32) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the curve samples in f64 and feeds f32 paint values"
    )]
    let value = MotionCurve::SmoothOut.sample(f64::from(progress)) as f32;
    value
}

fn cubic_axis(t: f64, first: f64, second: f64) -> f64 {
    let inverse = 1.0 - t;
    (t * t).mul_add(
        t,
        (3.0 * inverse * t * t).mul_add(second, 3.0 * inverse * inverse * t * first),
    )
}
