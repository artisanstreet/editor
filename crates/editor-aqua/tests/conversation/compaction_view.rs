use aqua::UiApp;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_core::{DisplayItem, scene::GpuiTextShimmer};
use aqua_ui::{
    element::{Box as UiBox, Key},
    tree::hash_key,
};
use editor_aqua::{
    conversation::{
        compaction::{CompactionFrame, CompactionPhase},
        view::compaction,
    },
    ui::theme::ArtisanTheme,
};

fn frame(swap: Option<(CompactionPhase, f32)>) -> CompactionFrame {
    CompactionFrame {
        open: 1.0,
        draw: 1.0,
        label: 1.0,
        swap,
        pulse: None,
    }
}

fn node(app: &UiApp, key: &str) -> aqua_core::NodeId {
    let key = hash_key(Key::from(key));
    app.tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap()
}

const CLOCK: u64 = 41;

fn scopes(app: &mut UiApp) -> Vec<GpuiTextShimmer> {
    app.tree_mut()
        .emit_scene()
        .into_iter()
        .filter_map(|(_, item)| match item {
            DisplayItem::PushGpuiTextShimmer(shimmer) => Some(shimmer),
            _ => None,
        })
        .collect()
}

fn app() -> UiApp {
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    UiApp::new(fonts, SharedRuns::new())
}

#[test]
fn label_shimmers_only_while_it_reads_as_running() {
    let theme = ArtisanTheme::resolved();
    let mut app = app();
    let render = |phase, current, clock| {
        UiBox::new()
            .width(400.0)
            .height(40.0)
            .child(compaction::label(phase, None, current, theme, clock))
            .build()
            .with_key("row")
    };
    app.mount(|_| render(CompactionPhase::Running, frame(None), Some(CLOCK)))
        .unwrap();
    app.set_viewport(400.0, 40.0, 1.0);
    app.build_frame().unwrap();
    assert_eq!(scopes(&mut app).len(), 1, "a running compaction shimmers");

    let outgoing = frame(Some((CompactionPhase::Running, 0.25)));
    app.refresh(vec![render(CompactionPhase::Done, outgoing, Some(CLOCK))])
        .unwrap();
    app.build_frame().unwrap();
    assert_eq!(
        scopes(&mut app).len(),
        1,
        "the outgoing Compacting label keeps shimmering until it has faded"
    );

    let incoming = frame(Some((CompactionPhase::Running, 0.75)));
    app.refresh(vec![render(CompactionPhase::Done, incoming, Some(CLOCK))])
        .unwrap();
    app.build_frame().unwrap();
    assert!(
        scopes(&mut app).is_empty(),
        "the incoming Compacted label is still"
    );

    app.refresh(vec![render(CompactionPhase::Running, frame(None), None)])
        .unwrap();
    app.build_frame().unwrap();
    assert!(
        scopes(&mut app).is_empty(),
        "reduced motion leaves a running label still"
    );
}

#[test]
fn a_rebuilt_model_switch_label_never_paints_a_frame_without_its_shimmer() {
    let theme = ArtisanTheme::resolved();
    let mut app = app();
    let switch = artisan_api::ModelSwitch {
        from: artisan_api::NamedModel {
            engine: artisan_api::EngineId::Codex,
            name: "GPT 6.1 Sol".into(),
        },
        to: artisan_api::NamedModel {
            engine: artisan_api::EngineId::Claude,
            name: "Opus 5.5".into(),
        },
    };
    let render = |pulse: f32| {
        let mut current = frame(None);
        current.pulse = Some(pulse);
        UiBox::new()
            .width(400.0)
            .height(40.0)
            .child(compaction::label(
                CompactionPhase::Running,
                Some(&switch),
                current,
                theme,
                Some(CLOCK),
            ))
            .build()
            .with_key("row")
    };
    app.mount(|_| render(0.1)).unwrap();
    app.set_viewport(400.0, 40.0, 1.0);
    app.build_frame().unwrap();
    let mut clocks = aqua::render::text::ShimmerClocks::default();
    let mut paint = |app: &mut UiApp, now: f64| {
        let scopes = scopes(app);
        clocks.begin_frame();
        let phases: Vec<f32> = scopes
            .iter()
            .map(|shimmer| clocks.phase(shimmer, now))
            .collect();
        clocks.end_frame();
        phases
    };
    assert_eq!(paint(&mut app, 0.0), vec![0.0], "the sweep starts");
    let label = node(&app, "compaction-shimmer");
    let width = app.tree().rect(label).unwrap().w;
    for (step, now) in [(0.2, 0.5), (0.3, 1.0), (0.4, 1.5)] {
        app.refresh(vec![render(step)]).unwrap();
        app.build_frame().unwrap();
        let phases = paint(&mut app, now);
        assert_eq!(
            phases.len(),
            1,
            "the frame painted right after a rebuild still shimmers"
        );
        assert!(
            (phases[0] - (now / 2.0) as f32).abs() < 0.001,
            "the sweep continues across rebuilds instead of restarting: {phases:?} at {now}"
        );
    }
    let scope = scopes(&mut app)[0];
    assert!(
        (scope.map.bounds.w - width).abs() < 0.5,
        "the sweep spans the whole switch line"
    );
}
