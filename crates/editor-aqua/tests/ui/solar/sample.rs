#[path = "../../../src/ui/solar/sample.rs"]
mod sample;

fn parameters(width: f32, height: f32, time: f32) -> Vec<f32> {
    vec![
        width, height, time, 1.0, 0.06, 0.06, 0.06, 1.0, 0.25, 0.45, 0.65, 1.0, 0.9, 0.4, 0.1, 1.0,
        1.0, 0.9, 0.7, 1.0, 0.4, 0.0, 0.0, 0.0, 0.88, 0.2, 0.12, 1.0,
    ]
}

#[test]
fn resizing_and_long_running_animation_keep_every_channel_finite() {
    for (width, height) in [(360.0, 640.0), (1920.0, 1080.0), (3840.0, 2160.0)] {
        for time in [0.0, 7.5, 3600.0, 86400.0] {
            let p = parameters(width, height, time);
            for row in 0..25 {
                for column in 0..25 {
                    let color = sample::shade(
                        &p,
                        [(column as f32 + 0.5) / 25.0, (row as f32 + 0.5) / 25.0],
                    );
                    assert!(
                        color.iter().all(|c| c.is_finite() && *c >= 0.0),
                        "shader output must remain drawable at {width}x{height}, t={time}"
                    );
                    assert_eq!(color[3], 1.0, "the horizon must not expose another surface");
                }
            }
        }
    }
}

#[test]
fn motion_stays_near_the_horizon_and_edges_blend_into_the_surface() {
    let first = parameters(1600.0, 1000.0, 0.0);
    let later = parameters(1600.0, 1000.0, 8.0);
    let difference = |uv| {
        let a = sample::shade(&first, uv);
        let b = sample::shade(&later, uv);
        (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f32>()
    };
    let rim_motion: f32 = (1..40).map(|x| difference([x as f32 / 40.0, 0.4])).sum();
    let quiet_motion: f32 = (1..40).map(|x| difference([x as f32 / 40.0, 0.9])).sum();
    assert!(rim_motion > 0.01, "the burning edge changes over time");
    assert!(
        quiet_motion < rim_motion * 0.01,
        "the composer foreground stays quiet"
    );
    for uv in [[0.001, 0.4], [0.999, 0.4], [0.5, 0.001], [0.5, 0.999]] {
        let pixel = sample::shade(&later, uv);
        assert!(
            (0..3).all(|channel| (pixel[channel] - later[4 + channel]).abs() < 0.0001),
            "artwork blends seamlessly into each panel edge"
        );
    }
}

#[test]
fn the_rounded_mask_preserves_the_frame_at_every_scale() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        let mut p = parameters(800.0 * scale, 600.0 * scale, 2.0);
        p[3] = scale;
        p[21] = 16.4;
        for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
            assert_eq!(
                sample::shade(&p, uv),
                [0.0; 4],
                "the shader must leave every outer corner transparent"
            );
        }
        assert_eq!(
            sample::shade(&p, [0.5, 0.5])[3],
            1.0,
            "the inner face remains opaque"
        );
        let x = 16.4 / 800.0;
        let y = (0.5 / (600.0 * scale)).max(0.0);
        let alpha = sample::shade(&p, [x, y])[3];
        assert!(
            (alpha - 1.0).abs() < 0.001,
            "the straight edge starts one half-pixel inside the face"
        );
    }
}

#[test]
fn atmosphere_drifts_over_seconds_without_flickering_between_frames() {
    let difference = |time: f32, delta: f32| {
        let first = parameters(1600.0, 1000.0, time);
        let later = parameters(1600.0, 1000.0, time + delta);
        let mut change = 0.0;
        for column in 0..20 {
            for row in 0..10 {
                let uv = [0.15 + column as f32 * 0.035, 0.15 + row as f32 * 0.018];
                let a = sample::shade(&first, uv);
                let b = sample::shade(&later, uv);
                change += (0..3)
                    .map(|channel| (a[channel] - b[channel]).abs())
                    .sum::<f32>();
            }
        }
        change / 600.0
    };
    for time in [0.0, 10.0, 40.0] {
        let drift = difference(time, 4.0);
        let frame = difference(time, 1.0 / 60.0);
        assert!(
            drift > 0.001,
            "the atmosphere should visibly travel over seconds: {drift}"
        );
        assert!(
            frame < 0.001,
            "adjacent frames should remain smooth: {frame}"
        );
        assert!(frame < drift * 0.05, "movement should accumulate gradually");
    }
}

#[test]
fn dirty_burn_moves_locally_without_turning_into_tall_flames_or_flashing() {
    let motion = |time: f32, delta: f32, altitude: f32| {
        let first = parameters(1600.0, 1000.0, time);
        let later = parameters(1600.0, 1000.0, time + delta);
        let mut change = 0.0;
        for column in 0..100 {
            let u = 0.15 + column as f32 * 0.007;
            let x = (u - 0.5) * 1600.0 / 1400.0;
            let uv = [u, 0.4 + (0.16 * x * x - altitude) * 1.4];
            let a = sample::shade(&first, uv);
            let b = sample::shade(&later, uv);
            change += (0..3)
                .map(|channel| (a[channel] - b[channel]).abs())
                .sum::<f32>();
        }
        change / 300.0
    };
    for time in [0.0, 2.0, 10.0, 40.0] {
        let burn = motion(time, 0.3, 0.002);
        let distant = motion(time, 0.3, 0.025);
        let frame = motion(time, 1.0 / 60.0, 0.002);
        assert!(
            burn > 0.008,
            "the small burning edge must be visibly alive: {burn}"
        );
        assert!(
            distant < burn * 0.3,
            "fast motion must stay close to the rim: {distant}, {burn}"
        );
        assert!(
            frame < 0.015 && frame < burn * 0.25,
            "the burn must move smoothly without flashing: {frame}, {burn}"
        );
    }
}
