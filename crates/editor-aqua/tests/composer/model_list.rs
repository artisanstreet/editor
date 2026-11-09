use artisan_api::EngineId;
use editor_aqua::composer::model_list::{ListLayout, ModelScroll};
use editor_aqua::composer::picker::ModelRow;
use std::time::{Duration, Instant};

const VIEWPORT: f32 = 120.0;

fn model(engine: EngineId, id: &str) -> ModelRow {
    ModelRow::Model {
        engine,
        id: id.to_owned(),
        index: 0,
    }
}

fn rows() -> Vec<ModelRow> {
    let mut rows = vec![ModelRow::Engine(EngineId::Claude)];
    rows.extend((0..6).map(|i| model(EngineId::Claude, &format!("c{i}"))));
    rows.push(ModelRow::Engine(EngineId::Codex));
    rows.extend((0..6).map(|i| model(EngineId::Codex, &format!("x{i}"))));
    rows
}

#[test]
fn the_engine_bar_follows_the_section_under_it_and_lands_on_the_last_at_the_end() {
    let rows = rows();
    let layout = ListLayout::new(&rows);
    assert_eq!(layout.spied(&rows, 0.0, VIEWPORT), Some(EngineId::Claude));
    let codex = layout.anchor(&rows, EngineId::Codex, VIEWPORT).unwrap();
    assert_eq!(layout.spied(&rows, codex, VIEWPORT), Some(EngineId::Codex));
    assert_eq!(
        layout.spied(&rows, codex - 60.0, VIEWPORT),
        Some(EngineId::Claude)
    );
    assert_eq!(
        layout.spied(&rows, layout.max_scroll(VIEWPORT), VIEWPORT),
        Some(EngineId::Codex)
    );
}

#[test]
fn opening_centres_the_current_model_without_scrolling_past_the_ends() {
    let rows = rows();
    let layout = ListLayout::new(&rows);
    let first = layout.centred(&rows, EngineId::Claude, "c0", VIEWPORT).unwrap();
    assert_eq!(first, 0.0);
    let last = layout.centred(&rows, EngineId::Codex, "x5", VIEWPORT).unwrap();
    assert_eq!(last, layout.max_scroll(VIEWPORT));
    assert!(layout.centred(&rows, EngineId::Codex, "gone", VIEWPORT).is_none());
}

#[test]
fn a_reader_scrolling_during_a_jump_takes_over() {
    let mut scroll = ModelScroll::default();
    let now = Instant::now();
    scroll.open(0.0);
    scroll.jump(EngineId::Codex, 200.0);
    let first = scroll.offset(0.0, 400.0, now, false);
    let second = scroll.offset(first, 400.0, now + Duration::from_millis(100), false);
    assert!(second > first);
    assert_eq!(scroll.offset(second + 40.0, 400.0, now, false), second + 40.0);
    assert!(!scroll.jumping());
}

#[test]
fn coming_back_from_a_settings_pane_restores_the_list_position() {
    let mut scroll = ModelScroll::default();
    let now = Instant::now();
    scroll.open(0.0);
    assert_eq!(scroll.offset(180.0, 400.0, now, false), 180.0);
    scroll.hide();
    assert_eq!(scroll.offset(0.0, 400.0, now, false), 180.0);
}

#[test]
fn a_jump_right_after_the_wheel_moved_the_list_still_runs() {
    let start = Instant::now();
    let mut scroll = ModelScroll::default();
    scroll.open(0.0);
    scroll.offset(0.0, 600.0, start, false);
    scroll.jump(EngineId::Codex, 400.0);
    assert_eq!(scroll.offset(120.0, 600.0, start, false), 120.0);
    assert!(scroll.jumping());
    let landed = scroll.offset(120.0, 600.0, start + Duration::from_millis(400), false);
    assert_eq!(landed, 400.0);
    assert!(!scroll.jumping());
}
