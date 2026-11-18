use editor_aqua::frame::overview::{AGENTS_KEY, Overview, Subagent, card};
use editor_aqua::inspector::agent_card;
use std::rc::Rc;

#[test]
fn a_thread_with_dozens_of_agents_keeps_them_in_a_scrolling_list_inside_the_window() {
    let agents = (0..30)
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
    let overview = Overview {
        project: Some("editor".into()),
        machine: None,
        branch: None,
        context: None,
        agents,
    };
    let element = card(
        &editor_aqua::ui::theme::ArtisanTheme::resolved(),
        &overview,
        true,
        Rc::new(|_| {}),
    )
    .expect("the overview shows");
    let app = test_support::editor::ui::mounted(
        400.0,
        600.0,
        aqua_ui::element::Box::new()
            .layout("column")
            .width(400.0)
            .height(600.0)
            .child(element)
            .build(),
    );
    let list = test_support::editor::ui::node_with_key(&app, AGENTS_KEY);
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
