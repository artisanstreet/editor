use super::*;

fn gated(engines: &[EngineId]) -> Vec<ModelRow> {
    engines
        .iter()
        .flat_map(|engine| [ModelRow::Engine(*engine), ModelRow::Gate(*engine)])
        .collect()
}

const VIEWPORT: f32 = 300.0;

fn spied_after_jump(rows: &[ModelRow], engine: EngineId, reduced: bool) -> Option<EngineId> {
    let layout = ListLayout::new(rows);
    let max = layout.max_scroll(VIEWPORT);
    let mut scroll = ModelScroll::default();
    scroll.open(0.0);
    let mut kept = scroll.offset(0.0, max, Instant::now(), reduced);
    scroll.jump(engine, layout.anchor(rows, engine, VIEWPORT).unwrap());
    let start = Instant::now();
    for step in 0..=20 {
        kept = scroll.offset(kept, max, start + JUMP_DURATION * step / 10, reduced);
    }
    assert!(!scroll.jumping(), "the jump settles");
    scroll
        .target()
        .or_else(|| layout.spied(rows, kept, VIEWPORT))
}

#[test]
fn a_jump_to_a_middle_engine_that_cannot_reach_the_top_keeps_that_engine_selected() {
    let rows = gated(&[EngineId::Claude, EngineId::Codex, EngineId::OpenCode]);
    let layout = ListLayout::new(&rows);
    let anchor = layout.anchor(&rows, EngineId::Codex, VIEWPORT).unwrap();
    assert_eq!(
        anchor,
        layout.max_scroll(VIEWPORT),
        "Codex's section can only scroll as far as the end of the list"
    );
    assert_eq!(
        spied_after_jump(&rows, EngineId::Codex, false),
        Some(EngineId::Codex)
    );
    assert_eq!(
        spied_after_jump(&rows, EngineId::Codex, true),
        Some(EngineId::Codex)
    );
}

#[test]
fn scrolling_after_a_jump_hands_the_selection_back_to_the_scroll_position() {
    let rows = gated(&[EngineId::Claude, EngineId::Codex, EngineId::OpenCode]);
    let layout = ListLayout::new(&rows);
    let max = layout.max_scroll(VIEWPORT);
    let mut scroll = ModelScroll::default();
    scroll.open(0.0);
    scroll.jump(
        EngineId::Codex,
        layout.anchor(&rows, EngineId::Codex, VIEWPORT).unwrap(),
    );
    let landed = scroll.offset(0.0, max, Instant::now(), true);
    assert_eq!(scroll.target(), Some(EngineId::Codex));
    scroll.offset(landed - 40.0, max, Instant::now(), true);
    assert_eq!(scroll.target(), None);
}
