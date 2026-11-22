#[test]
fn opencode_agent_reports_variant_even_with_legacy_effort() {
    let mut info = forge_runs::AgentInfo {
        agent: artisan_api::ItemId::new(),
        thread: artisan_api::ThreadId::new(),
        task: "Review".into(),
        harness: artisan_api::EngineId::OpenCode,
        model: Some("opencode-go/muse".into()),
        effort: Some("medium".into()),
        variant: Some("xhigh".into()),
        permissions: artisan_api::Permissions::Auto,
        state: forge_runs::AgentState::Working,
        result: None,
        error: None,
        warnings: Vec::new(),
    };
    let output = super::agent(&info);
    assert!(output.contains("opencode-go/muse, xhigh"));
    assert!(!output.contains("medium"));
    info.variant = None;
    assert!(!super::agent(&info).contains("medium"));
    info.harness = artisan_api::EngineId::Codex;
    assert!(super::agent(&info).contains("medium"));
}

#[test]
fn catalog_exposes_variant_ids_defaults_and_exact_context_choices() {
    let models: Vec<artisan_api::Model> = serde_json::from_value(serde_json::json!([{
        "id": "opencode/muse", "name": "Muse", "efforts": [], "default_effort": null,
        "variants": [{"id": "xhigh", "label": "Extra high"}], "default_variant": "xhigh",
        "context_window": 262_144,
        "context_windows": [{"tokens": 262_144}, {"tokens": 1_048_576, "override_tokens": 1_000_000}],
        "supports_images": true, "description": "Contributor", "lab": "Meta", "group": "OpenCode",
        "speeds": [{"id": "fast", "label": "Fast", "description": "Lower latency", "default": true}],
        "effort_notes": [{"effort": "max", "special": true, "advisory": "More usage", "description": "Maximum effort"}]
    }])).unwrap();
    let output = super::models(&models).unwrap();
    assert!(output.contains("OpenCode reasoning choices use variant"));
    let json = output
        .split("```json\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let catalog: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(catalog[0]["variants"][0]["id"], "xhigh");
    assert_eq!(catalog[0]["default_variant"], "xhigh");
    assert_eq!(catalog[0]["context_window"], 262_144);
    assert_eq!(
        catalog[0]["context_windows"][1]["override_tokens"],
        1_000_000
    );
    assert_eq!(catalog[0]["speeds"][0]["default"], true);
    assert_eq!(catalog[0]["effort_notes"][0]["advisory"], "More usage");
    assert_eq!(catalog[0]["supports_images"], true);
    assert_eq!(catalog[0]["description"], "Contributor");
    assert_eq!(catalog[0]["lab"], "Meta");
    assert_eq!(catalog[0]["group"], "OpenCode");
}
