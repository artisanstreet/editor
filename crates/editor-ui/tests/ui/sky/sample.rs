#[path = "../../../src/ui/sky/sample.rs"]
mod sample;

fn parameters(width: f32, height: f32, scale: f32, time: f32) -> [f32; 36] {
    [
        width,
        height,
        time,
        scale,
        17.0 / 255.0,
        17.0 / 255.0,
        17.0 / 255.0,
        1.0,
        0.163_456_72,
        0.001828275,
        -0.052028373,
        1.0,
        0.243_426_04,
        -0.000731804,
        -0.089916567,
        1.0,
        0.366_245_7,
        -0.011359377,
        -0.113_559,
        1.0,
        0.571_379_2,
        -0.026_343_03,
        -0.093_552_39,
        1.0,
        0.805_291_95,
        -0.018475078,
        -0.042172065,
        1.0,
        0.94,
        0.97,
        0.99,
        16.4,
        0.4,
        0.0,
        0.0,
        0.0,
    ]
}

#[test]
fn the_perceptual_gradient_preserves_the_supplied_palette_without_banding() {
    let p = parameters(1600.0, 1000.0, 1.0, 0.0);
    for (height, rgb) in [
        (0.0, [7, 11, 36]),
        (0.24, [16, 26, 74]),
        (0.46, [31, 58, 122]),
        (0.68, [78, 122, 176]),
        (0.88, [168, 195, 220]),
    ] {
        let color = sample::sky_color(&p, height);
        assert!((0..3).all(|c| (color[c] * 255.0 - rgb[c] as f32).abs() < 0.02));
    }
    let mut previous = sample::sky_color(&p, 0.0);
    for row in 1..1000 {
        let color = sample::sky_color(&p, row as f32 / 1000.0);
        assert!((0..3).all(|c| color[c] >= previous[c] - 0.00001));
        assert!((0..3).all(|c| (color[c] - previous[c]).abs() < 0.006));
        previous = color;
    }
}

#[test]
fn the_curved_rim_leaves_the_composer_foreground_dark_at_every_scale() {
    for (width, height) in [(600.0, 480.0), (1600.0, 1000.0), (2400.0, 700.0)] {
        for scale in [1.0, 1.25, 2.0] {
            let p = parameters(width * scale, height * scale, scale, 0.0);
            let unit = p[0].min(p[1] * 1.4);
            for u in [0.2, 0.35, 0.5, 0.65, 0.8] {
                let x = (u - 0.5) * p[0] / unit;
                let rim_y = p[32] + 0.16 * x * x * unit / p[1];
                let rim_light = (-3..=3)
                    .map(|offset| {
                        sample::shade(&p, [u, rim_y + offset as f32 * 0.0005 * unit / p[1]])[0]
                    })
                    .fold(0.0_f32, f32::max);
                let below = sample::shade(&p, [u, rim_y + 0.05]);
                assert!(rim_light > 0.8, "the rim burns white: {rim_light}");
                assert!((0..3).all(|c| (below[c] - p[4 + c]).abs() < 0.005));
                let sky = sample::shade(&p, [u, rim_y - 0.1]);
                assert!(sky[2] > sky[0] + 0.08);
            }
        }
    }
}

#[test]
fn the_sky_blends_to_grey_and_preserves_the_rounded_frame() {
    for scale in [1.0, 1.25, 2.0] {
        for time in [0.0, 10.0, 3600.0, 86400.0] {
            let p = parameters(1200.0 * scale, 800.0 * scale, scale, time);
            for uv in [[0.0, 0.5], [1.0, 0.5], [0.5, 0.0], [0.5, 1.0]] {
                let color = sample::shade(&p, uv);
                assert!((0..3).all(|c| (color[c] - p[4 + c] * color[3]).abs() < 0.00001));
            }
            for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
                assert_eq!(sample::shade(&p, uv), [0.0; 4]);
            }
            for row in 0..30 {
                for column in 0..30 {
                    let color = sample::shade(
                        &p,
                        [(column as f32 + 0.5) / 30.0, (row as f32 + 0.5) / 30.0],
                    );
                    assert!(
                        color
                            .iter()
                            .all(|v| v.is_finite() && (0.0..=2.0).contains(v))
                    );
                }
            }
        }
    }
}

#[test]
fn the_small_burn_is_alive_while_the_sky_and_foreground_stay_quiet() {
    let difference = |time: f32, delta: f32, offset: f32| {
        let a = parameters(1600.0, 1000.0, 1.0, time);
        let b = parameters(1600.0, 1000.0, 1.0, time + delta);
        let mut change = 0.0;
        for column in 0..100 {
            let u = 0.15 + column as f32 * 0.007;
            let x = (u - 0.5) * 1600.0 / 1400.0;
            let uv = [u, 0.4 + (0.16 * x * x + offset) * 1.4];
            let first = sample::shade(&a, uv);
            let later = sample::shade(&b, uv);
            change += (0..3).map(|c| (first[c] - later[c]).abs()).sum::<f32>();
        }
        change / 300.0
    };
    for time in [0.0, 10.0, 40.0] {
        let burn = difference(time, 0.3, -0.002);
        let frame = difference(time, 1.0 / 60.0, -0.002);
        let sky = difference(time, 0.3, -0.10);
        let foreground = difference(time, 0.3, 0.05);
        assert!(burn > 0.003, "the thin edge changes over time: {burn}");
        assert!(
            frame < 0.02 && frame < burn * 0.30,
            "adjacent frames stay smooth: {frame}, {burn}"
        );
        assert!(sky < burn * 0.1, "the clear sky stays calm");
        assert!(
            foreground < 0.0001,
            "the composer sits in a quiet foreground"
        );
    }
}

#[test]
fn gradient_slopes_remain_continuous_without_flat_spots_at_the_color_stops() {
    let p = parameters(1600.0, 1000.0, 1.0, 0.0);
    let epsilon = 0.0002;
    for stop in [0.24, 0.46, 0.68] {
        let before = sample::sky_color(&p, stop - epsilon);
        let at = sample::sky_color(&p, stop);
        let after = sample::sky_color(&p, stop + epsilon);
        for channel in 0..3 {
            let left = (at[channel] - before[channel]) / epsilon;
            let right = (after[channel] - at[channel]) / epsilon;
            assert!(
                left > 0.1 && right > 0.1,
                "a palette stop must not flatten the sky: {stop}, {channel}, {left}, {right}"
            );
            assert!(
                (left - right).abs() < 0.03,
                "the slope must not jump at a palette stop: {stop}, {channel}, {left}, {right}"
            );
        }
    }
}

#[test]
fn per_channel_dithering_preserves_sub_byte_color_after_eight_bit_quantization() {
    let mut noise_sum = [0.0; 3];
    let mut quantized_sum = [0.0; 3];
    let mut decorrelated = 0;
    let target = 0.1013;
    for y in 0..64 {
        for x in 0..64 {
            let noise = sample::dither([x as f32, y as f32]);
            for channel in 0..3 {
                assert!(noise[channel].abs() <= 0.5 / 255.0);
                noise_sum[channel] += noise[channel];
                quantized_sum[channel] += ((target + noise[channel]) * 255.0).round() / 255.0;
            }
            decorrelated += usize::from((noise[0] - noise[1]).abs() > 0.00001);
        }
    }
    for channel in 0..3 {
        assert!((noise_sum[channel] / 4096.0).abs() < 0.00002);
        assert!((quantized_sum[channel] / 4096.0 - target).abs() < 0.00003);
    }
    assert!(decorrelated > 4000);
}

#[test]
fn the_foreground_tracks_the_card_color_without_a_second_surface_gradient() {
    for background in [[0.03, 0.03, 0.03], [0.12, 0.10, 0.09], [0.80, 0.82, 0.84]] {
        let mut p = parameters(1600.0, 1000.0, 1.0, 0.0);
        p[4..7].copy_from_slice(&background);
        for uv in [[0.2, 0.65], [0.5, 0.6], [0.7, 0.8], [0.5, 0.95]] {
            let color = sample::shade(&p, uv);
            assert!((0..3).all(|c| (color[c] - background[c]).abs() < 0.002));
        }
    }
}
