//! Focused parity coverage for the deterministic gradient-avatar policy.
//!
//! The source is included directly so this harness stays dependency-free and
//! can run with `rustc --test` before the shared frontend registrations exist.

#![allow(clippy::float_cmp)]

#[path = "../../modules/frontend/src/gradient_avatar.rs"]
mod gradient_avatar;

use gradient_avatar::{
    AVATAR_CELLS, AVATAR_PALETTE, BAYER4, GradientAvatarColor, GradientAvatarRun, MAXIMUM_DENSITY,
    gradient_avatar_color_for, gradient_avatar_runs, gradient_avatar_seed_hash,
    gradient_avatar_svg,
};

#[test]
fn palette_is_the_exact_chromatic_tailwind_order() {
    let expected = [
        (
            "red",
            "oklch(57.7% 0.245 27.325)",
            "oklch(70.4% 0.191 22.216)",
        ),
        (
            "orange",
            "oklch(64.6% 0.222 41.116)",
            "oklch(75% 0.183 55.934)",
        ),
        (
            "amber",
            "oklch(66.6% 0.179 58.318)",
            "oklch(82.8% 0.189 84.429)",
        ),
        (
            "yellow",
            "oklch(68.1% 0.162 75.834)",
            "oklch(85.2% 0.199 91.936)",
        ),
        (
            "lime",
            "oklch(64.8% 0.2 131.684)",
            "oklch(84.1% 0.238 128.85)",
        ),
        (
            "green",
            "oklch(62.7% 0.194 149.214)",
            "oklch(79.2% 0.209 151.711)",
        ),
        (
            "emerald",
            "oklch(59.6% 0.145 163.225)",
            "oklch(76.5% 0.177 163.223)",
        ),
        (
            "teal",
            "oklch(60% 0.118 184.704)",
            "oklch(77.7% 0.152 181.912)",
        ),
        (
            "cyan",
            "oklch(60.9% 0.126 221.723)",
            "oklch(78.9% 0.154 211.53)",
        ),
        (
            "sky",
            "oklch(58.8% 0.158 241.966)",
            "oklch(74.6% 0.16 232.661)",
        ),
        (
            "blue",
            "oklch(54.6% 0.245 262.881)",
            "oklch(70.7% 0.165 254.624)",
        ),
        (
            "indigo",
            "oklch(51.1% 0.262 276.966)",
            "oklch(67.3% 0.182 276.935)",
        ),
        (
            "violet",
            "oklch(54.1% 0.281 293.009)",
            "oklch(70.2% 0.183 293.541)",
        ),
        (
            "purple",
            "oklch(55.8% 0.288 302.321)",
            "oklch(71.4% 0.203 305.504)",
        ),
        (
            "fuchsia",
            "oklch(59.1% 0.293 322.896)",
            "oklch(74% 0.238 322.16)",
        ),
        (
            "pink",
            "oklch(59.2% 0.249 0.584)",
            "oklch(71.8% 0.202 349.761)",
        ),
        (
            "rose",
            "oklch(58.6% 0.253 17.585)",
            "oklch(71.2% 0.194 13.428)",
        ),
    ];

    assert_eq!(AVATAR_PALETTE.len(), 17);
    assert_eq!(GradientAvatarColor::ALL, AVATAR_PALETTE);
    for (entry, (name, from, to)) in AVATAR_PALETTE.iter().zip(expected) {
        assert_eq!((entry.name, entry.from, entry.to), (name, from, to));
    }
}

#[test]
fn bayer_grid_and_avatar_constants_are_pinned() {
    assert_eq!(AVATAR_CELLS, 16);
    assert_eq!(MAXIMUM_DENSITY, 0.9);
    assert_eq!(
        BAYER4,
        [
            [0.03125, 0.53125, 0.15625, 0.65625],
            [0.78125, 0.28125, 0.90625, 0.40625],
            [0.21875, 0.71875, 0.09375, 0.59375],
            [0.96875, 0.46875, 0.84375, 0.34375],
        ]
    );
}

#[test]
fn hash_uses_utf16_code_units_for_ascii_bmp_and_supplementary_text() {
    let cases = [
        ("", 0x811c9dc5_u32),
        ("a", 0xe40c292c_u32),
        ("hello", 0x4f9f2cab_u32),
        ("Å", 0x400b2700_u32),
        ("😀", 0xcb31c4b8_u32),
        ("A😀é", 0x7d547f90_u32),
    ];

    for (seed, expected) in cases {
        assert_eq!(
            gradient_avatar_seed_hash(seed),
            expected,
            "UTF-16 FNV-1a hash for {seed:?}"
        );
    }
}

#[test]
fn seeds_select_typed_palette_entries_by_hash_bucket() {
    let cases = [
        ("", "orange"),
        ("a", "amber"),
        ("hello", "indigo"),
        ("😀", "yellow"),
        ("A😀é", "lime"),
    ];

    for (seed, expected_name) in cases {
        let color = gradient_avatar_color_for(seed);
        assert_eq!(color.name, expected_name, "palette for seed {seed:?}");
        assert_eq!(
            color,
            AVATAR_PALETTE[(gradient_avatar_seed_hash(seed) as usize) % 17]
        );
    }
}

#[test]
fn lit_runs_pin_diagonal_geometry_merging_and_count() {
    let runs = gradient_avatar_runs();

    assert_eq!(runs.len(), 92);
    assert_eq!(
        runs.first(),
        Some(&GradientAvatarRun {
            x: 0,
            y: 0,
            width: 1
        })
    );
    assert_eq!(
        runs.get(1),
        Some(&GradientAvatarRun {
            x: 2,
            y: 0,
            width: 1,
        })
    );
    assert_eq!(
        runs.get(2),
        Some(&GradientAvatarRun {
            x: 4,
            y: 0,
            width: 12,
        })
    );
    assert_eq!(
        runs.last(),
        Some(&GradientAvatarRun {
            x: 15,
            y: 15,
            width: 1,
        })
    );

    let row_counts: Vec<usize> = (0..AVATAR_CELLS)
        .map(|y| runs.iter().filter(|run| run.y == y).count())
        .collect();
    assert_eq!(
        row_counts,
        vec![3, 7, 5, 7, 5, 8, 7, 6, 7, 6, 7, 4, 8, 4, 6, 2]
    );

    // Every run is horizontal, non-empty, and is followed by a dark cell or
    // the row boundary; adjacent lit cells must have been merged.
    for run in &runs {
        assert!(run.width > 0);
        assert!(run.x + run.width <= AVATAR_CELLS);
    }
    for pair in runs.windows(2) {
        if pair[0].y == pair[1].y {
            assert!(pair[0].x + pair[0].width < pair[1].x);
        }
    }
}

#[test]
fn svg_has_exact_standalone_framing_and_selected_colors() {
    let svg = gradient_avatar_svg("a", None);
    let color = gradient_avatar_color_for("a");

    assert!(svg.starts_with(
        &format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" width=\"100%\" height=\"100%\" preserveAspectRatio=\"xMidYMid slice\" shape-rendering=\"crispEdges\" aria-hidden=\"true\"><rect width=\"16\" height=\"16\" fill=\"{}\"/><g fill=\"{}\">",
            color.from, color.to
        )
    ));
    assert!(svg.ends_with("</g></svg>"));
    assert_eq!(svg.matches("<svg ").count(), 1);
    assert_eq!(
        svg.matches("<rect ").count(),
        gradient_avatar_runs().len() + 1
    );
    assert_eq!(svg.matches("<g ").count(), 1);
}

#[test]
fn accessibility_branches_and_attribute_escaping_match_typescript() {
    let hidden = gradient_avatar_svg("seed", None);
    assert!(hidden.contains(" aria-hidden=\"true\">") && !hidden.contains("role=\"img\""));

    let title = "A&B\" <tag> ' >";
    let labelled = gradient_avatar_svg("seed", Some(title));
    assert!(labelled.contains(" role=\"img\" aria-label=\"A&amp;B&quot; &lt;tag> ' >\"><rect"));
    assert!(!labelled.contains("aria-hidden"));

    // Some(empty) is present, unlike None, and therefore remains a labelled
    // image with an empty aria-label.
    assert!(gradient_avatar_svg("seed", Some("")).contains(" role=\"img\" aria-label=\"\"><rect"));
}

#[test]
fn repeated_results_are_byte_stable() {
    let seeds = ["", "host-name", "Å", "😀", "A😀é"];
    for seed in seeds {
        let expected_svg = gradient_avatar_svg(seed, Some("stable"));
        let expected_color = gradient_avatar_color_for(seed);
        let expected_runs = gradient_avatar_runs();
        for _ in 0..8 {
            assert_eq!(gradient_avatar_svg(seed, Some("stable")), expected_svg);
            assert_eq!(gradient_avatar_color_for(seed), expected_color);
            assert_eq!(gradient_avatar_runs(), expected_runs);
        }
    }
}
