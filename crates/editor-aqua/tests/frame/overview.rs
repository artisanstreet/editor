use editor_aqua::frame::overview::{
    AGENTS_EDGE_BOTTOM, AGENTS_EDGE_TOP, AGENTS_KEY, Overview, OverviewPanel, Subagent,
    agents_edge_color, card,
};
use editor_aqua::inspector::agent_card;
use editor_aqua::ui::scroll_edge::{EdgePaint, EdgeSurface, ScrollEdges};
use editor_aqua::ui::theme::ArtisanTheme;
use std::rc::Rc;
use std::time::Instant;
use test_support::editor::ui::node_with_key;

fn overview_of(count: usize) -> Overview {
    let agents = (0..count)
        .map(|index| {
            let agent: artisan_api::InspectorAgent = serde_json::from_value(serde_json::json!({
                "id": format!("agent-{index}"),
                "label": format!("Agent {index}"),
                "description": format!("Task {index}"),
                "status": "running",
            }))
            .expect("an inspector agent");
            Subagent {
                id: agent.id.clone(),
                thread: None,
                label: agent.label.clone(),
                task: format!("Task {index}"),
                status: agent.status,
                engine: None,
                card: agent_card(&agent, None),
            }
        })
        .collect();
    Overview {
        project: Some("editor".into()),
        machine: Some("DESKTOP".into()),
        branch: Some("master".into()),
        context: None,
        agents,
    }
}

fn mounted_card(overview: &Overview, edges: &ScrollEdges, width: f32, height: f32) -> aqua::UiApp {
    let emit: Rc<dyn Fn(editor_aqua::inspector::InspectorAction)> = Rc::new(|_| {});
    let element = card(
        &ArtisanTheme::resolved(),
        overview,
        true,
        &emit,
        &OverviewPanel::in_window(height, edges),
    )
    .expect("the overview shows");
    test_support::editor::ui::mounted(
        width,
        height,
        aqua_ui::element::Box::new()
            .layout("column")
            .width(width)
            .height(height)
            .child(element)
            .build(),
    )
}

#[test]
fn a_thread_with_dozens_of_agents_keeps_them_in_a_scrolling_list_inside_the_window() {
    let overview = overview_of(30);
    let edges = ScrollEdges::default();
    let app = mounted_card(&overview, &edges, 400.0, 600.0);
    let list = node_with_key(&app, AGENTS_KEY);
    let bounds = app.tree().rect(list).expect("the agent list is laid out");
    assert!(
        bounds.y + bounds.h <= 600.0,
        "the agent list ends inside the window: {bounds:?}"
    );
    let texts = test_support::editor::ui::painted_texts(&app);
    assert!(
        texts.iter().any(|text| text == "Agent 29"),
        "the last agent is in the list, reachable by scrolling"
    );
}

fn shade(app: &aqua::UiApp, key: &str) -> f32 {
    let node = node_with_key(app, key);
    match app
        .tree()
        .paint(node)
        .and_then(|paint| paint.gpui_background)
    {
        Some(aqua_core::GpuiBackground::LinearGradient { stops, .. }) => {
            stops.iter().map(|stop| stop.color.a).fold(0.0, f32::max)
        }
        other => panic!("the edge {key} paints a gradient, not {other:?}"),
    }
}

fn settle_edges(app: &mut aqua::UiApp, edges: &ScrollEdges) {
    let color = agents_edge_color(&ArtisanTheme::resolved());
    EdgeSurface {
        edges,
        scroller: AGENTS_KEY,
        top: AGENTS_EDGE_TOP,
        bottom: AGENTS_EDGE_BOTTOM,
        top_color: color,
        bottom_color: color,
        paint: EdgePaint::Gradient,
    }
    .advance(app, true, Instant::now());
}

#[test]
fn a_short_window_keeps_the_overview_inside_it_and_shades_the_agents_scrolled_out_of_view() {
    let overview = overview_of(30);
    let edges = ScrollEdges::default();
    let height = 360.0;
    let mut app = mounted_card(&overview, &edges, 400.0, height);
    let panel = app
        .tree()
        .rect(node_with_key(&app, "artisan-thread-overview"))
        .expect("the overview is laid out");
    assert!(
        panel.y + panel.h <= height,
        "the overview ends inside a {height} px window: {panel:?}"
    );
    let list = node_with_key(&app, AGENTS_KEY);
    let viewport = app.tree().rect(list).expect("the agent list is laid out");
    let content = app
        .tree()
        .content_size(list)
        .expect("the agent list measures its content");
    assert!(
        content.h > viewport.h + 1.0,
        "the agents that don't fit stay in the list and scroll: {content:?} in {viewport:?}"
    );

    settle_edges(&mut app, &edges);
    test_support::assert_near!(shade(&app, AGENTS_EDGE_TOP), 0.0);
    assert!(
        shade(&app, AGENTS_EDGE_BOTTOM) > 0.9,
        "the bottom edge is shaded while agents are hidden below"
    );

    app.tree_mut()
        .scroll_to(list, aqua_core::Point::new(0.0, content.h - viewport.h));
    app.layout().expect("lay out the scrolled list");
    settle_edges(&mut app, &edges);
    assert!(
        shade(&app, AGENTS_EDGE_TOP) > 0.9,
        "the top edge is shaded once agents are hidden above"
    );
    test_support::assert_near!(shade(&app, AGENTS_EDGE_BOTTOM), 0.0);
}
