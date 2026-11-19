use super::*;

#[test]
fn context_choices_follow_reported_limits_and_default_without_override() {
    let listing = codex_listing(
        &[json!({"id":"gpt-test"})],
        &[("gpt-test".to_owned(), 272_000, 1_000_000)],
    );
    assert_eq!(listing.models[0].context_window, Some(272_000));
    assert_eq!(
        listing.models[0].context_windows,
        [
            ContextWindow {
                tokens: 272_000,
                override_tokens: None
            },
            ContextWindow {
                tokens: 1_000_000,
                override_tokens: Some(1_000_000)
            },
        ]
    );
    assert!(
        codex_listing(&[json!({"id":"unknown"})], &[]).models[0]
            .context_windows
            .is_empty()
    );
    let claude =
        claude_model(&json!({"id":"claude-test", "runtime":{"max_input_tokens":1_000_000}}))
            .unwrap();
    assert_eq!(
        claude.context_windows,
        [
            ContextWindow {
                tokens: 1_000_000,
                override_tokens: None
            },
            ContextWindow {
                tokens: 500_000,
                override_tokens: Some(500_000)
            },
        ]
    );
    let small = claude_model(&json!({"id":"claude-small", "runtime":{"max_input_tokens":200_000}}))
        .unwrap();
    assert_eq!(
        small.context_windows,
        [ContextWindow {
            tokens: 200_000,
            override_tokens: None
        }]
    );
}

#[test]
fn codex_cache_retains_base_and_extended_window() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("models_cache.json");
    std::fs::write(
        &path,
        json!({"models":[
            {"slug":"extended", "context_window":272_000, "max_context_window":1_000_000},
            {"slug":"standard", "context_window":400_000}
        ]})
        .to_string(),
    )
    .unwrap();
    assert_eq!(
        codex_context_windows(&path).unwrap(),
        [
            ("extended".to_owned(), 272_000, 1_000_000),
            ("standard".to_owned(), 400_000, 400_000),
        ]
    );
}

#[test]
fn codex_catalog_has_unique_nonempty_model_identities() {
    let listing = codex_listing(
        &[
            json!({"id":"m1"}),
            json!({"id":"m1"}),
            json!({"id":""}),
            json!({"id":"m2"}),
        ],
        &[],
    );
    assert_eq!(
        listing
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["m1", "m2"]
    );
}

#[test]
fn oversized_claude_lists_are_capped_before_collecting_and_joining() {
    let models: Vec<_> = (0..MAX_MODELS * 2)
        .map(|index| json!({"id":format!("model-{index}")}))
        .collect();
    let published =
        json!({"surfaces":{"cc":{"model_selector_config":[{"id":"cc","models":models}]}}});
    let served = json!({"catalog":{"surface":"cc","config":{"id":"cc","models":models}}});
    assert_eq!(claude_published_models(&published).len(), MAX_MODELS);
    assert_eq!(claude_served_models(&served).len(), MAX_MODELS);
    let listing = merge_claude(Some(&published), Some(&served));
    assert_eq!(listing.models.len(), MAX_MODELS);
    assert_eq!(
        listing.models.last().unwrap().id,
        format!("model-{}", MAX_MODELS - 1)
    );
}

#[test]
fn oversized_cached_catalogs_are_not_loaded() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.json");
    std::fs::write(&path, "x".repeat(MAX_OUTPUT_BYTES + 1)).unwrap();
    assert!(matches!(
        read_json_file(&path),
        Err(ReadCacheError::TooLarge { .. })
    ));
}

#[test]
fn a_corrupt_model_cache_is_an_error_not_an_absent_cache() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing.json");
    assert!(matches!(read_json_file(&missing), Ok(None)));
    let corrupt = directory.path().join("models_cache.json");
    std::fs::write(&corrupt, "{\"models\": [").unwrap();
    assert!(matches!(
        codex_context_windows(&corrupt),
        Err(ReadCacheError::Decode { path, .. }) if path == corrupt
    ));
}

fn recording(path: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("transcripts")
        .join(path);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

fn model<'a>(listing: &'a Listing, id: &str) -> &'a Model {
    listing
        .models
        .iter()
        .find(|model| model.id == id)
        .unwrap_or_else(|| panic!("{id} is listed"))
}

fn speeds(model: &Model) -> Vec<(&str, &str, Option<&str>, bool)> {
    model
        .speeds
        .iter()
        .map(|speed| {
            (
                speed.id.as_str(),
                speed.label.as_str(),
                speed.description.as_deref(),
                speed.default,
            )
        })
        .collect()
}

#[test]
fn codex_offers_its_fast_tier_and_says_what_each_effort_does() {
    let recording = recording("codex/model-list.jsonl");
    let rows: Vec<Value> = recording
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|message| message.get("id").and_then(Value::as_u64) == Some(1))
        .filter_map(|message| message.pointer("/result/data").cloned())
        .filter_map(|data| data.as_array().cloned())
        .flatten()
        .collect();
    let listing = finish(EngineId::Codex, codex_listing(&rows, &[]));
    let sol = model(&listing, "gpt-6.1-sol");
    assert_eq!(sol.name, "GPT 6.1 Sol", "the picker reads names, not ids");
    assert_eq!(
        sol.description.as_deref(),
        Some("Latest workhorse model for coding and everyday work.")
    );
    assert_eq!(
        speeds(sol),
        [
            ("standard", "Standard", None, true),
            ("priority", "Fast", Some("2x speed, increased usage"), false),
        ],
        "Fast sends the tier id Codex advertises"
    );
    let note = |effort: &str| {
        sol.effort_notes
            .iter()
            .find(|note| note.effort == effort)
            .and_then(|note| note.description.clone())
    };
    assert_eq!(
        note("ultra").as_deref(),
        Some("Maximum reasoning with automatic task delegation")
    );
    assert_eq!(sol.effort_notes.len(), sol.efforts.len());
    assert!(model(&listing, "gpt-6-sol").speeds.is_empty());
    assert!(
        !listing
            .models
            .iter()
            .any(|model| model.id == "codex-auto-review"),
        "hidden rows stay hidden"
    );
}

#[test]
fn claude_offers_fast_mode_and_warns_about_max_effort() {
    let served: Value = serde_json::from_str(&recording("claude/model-catalog-cc.json")).unwrap();
    let listing = merge_claude(None, Some(&served));
    let opus = model(&listing, "claude-opus-5-5");
    assert_eq!(
        opus.description.as_deref(),
        Some("For complex work and everyday tasks")
    );
    assert_eq!(
        opus.default_effort.as_deref(),
        Some("medium"),
        "the picker starts at the effort the catalog recommends"
    );
    assert_eq!(
        model(&listing, "claude-opus-4-7").default_effort.as_deref(),
        Some("xhigh")
    );
    assert_eq!(
        speeds(opus),
        [
            ("standard", "Standard", None, true),
            ("fast", "Fast", None, false)
        ]
    );
    let max = opus
        .effort_notes
        .iter()
        .find(|note| note.effort == "max")
        .expect("max says what it costs");
    assert_eq!(max.advisory.as_deref(), Some("3.5\u{d7} or more usage"));
    assert_eq!(
        max.description.as_deref(),
        Some(
            "Claude thinks longer with Max effort and can consume 3.5\u{d7} or more usage than Medium."
        )
    );
    assert!(
        opus.effort_notes.iter().all(|note| note.effort == "max"),
        "a Recommended badge is no warning: {:?}",
        opus.effort_notes
    );
    assert!(
        model(&listing, "claude-sonnet-5-5").speeds.is_empty(),
        "fast mode only where the account is served it"
    );
}

#[test]
fn opencode_models_are_grouped_by_gateway_and_credited_to_their_lab() {
    let models = serde_json::json!({ "data": [
        { "id": "glm-5.3", "providerID": "opencode-go", "family": "glm", "name": "GLM-5.3",
          "enabled": true, "status": "active", "time": { "released": 2 },
          "capabilities": { "input": ["text"] }, "limit": { "context": 1_000_000, "output": 131_072 },
          "variants": [{ "id": "low" }, { "id": "max" }] },
        { "id": "claude-opus-5-5", "providerID": "opencode", "family": "claude-opus",
          "name": "Claude Opus 5.5", "enabled": true, "status": "active", "time": { "released": 1 },
          "capabilities": { "input": ["text", "image"] }, "limit": { "context": 1_000_000, "output": 128_000 },
          "variants": [{ "id": "none" }, { "id": "high" }] },
        { "id": "gpt-5.3-codex-spark", "providerID": "opencode", "family": "gpt-codex-spark",
          "name": "GPT-5.3-Codex-Spark", "enabled": true, "status": "active", "time": { "released": 3 },
          "capabilities": { "input": ["text"] }, "limit": { "context": 400_000, "output": 128_000 },
          "variants": [] },
        { "id": "claude-sonnet-5-5", "providerID": "opencode", "family": "claude-sonnet",
          "name": "Claude Sonnet 5.5", "enabled": true, "status": "active", "time": { "released": 4 },
          "capabilities": { "input": ["text"] }, "limit": { "context": 1_000_000, "output": 64_000 },
          "variants": [] },
        { "id": "big-pickle", "providerID": "opencode", "name": "Big Pickle", "enabled": true,
          "status": "active", "time": { "released": 0 }, "capabilities": { "input": ["text"] },
          "limit": { "context": 200_000, "output": 32_000 }, "variants": [] },
        { "id": "gpt-4", "providerID": "opencode", "name": "GPT-4", "enabled": true,
          "status": "deprecated", "time": { "released": 0 }, "capabilities": { "input": ["text"] },
          "limit": { "context": 8_000, "output": 4_000 }, "variants": [] }
    ] });
    let gateways = serde_json::json!({ "data": [
        { "id": "opencode", "name": "OpenCode Console" },
        { "id": "opencode-go", "name": "OpenCode Go" }
    ] });
    let digest = models_dev_digest(&serde_json::json!({
        "opencode": { "name": "OpenCode", "models": {
            "claude-opus-5-5": { "description": "Claude for long-running work.",
                "family": "claude-opus", "canonical_model_id": "anthropic/claude-opus-5-5" },
            "claude-sonnet-5-5": { "family": "claude-sonnet",
                "canonical_model_id": "anthropic/claude-sonnet-5-5" },
            "gpt-5.5": { "family": "gpt", "canonical_model_id": "openai/gpt-5.5" }
        } },
        "opencode-go": { "name": "OpenCode Go", "models": {
            "glm-5.3": { "description": "Flagship GLM.", "family": "glm",
                "canonical_model_id": "zhipuai/glm-5.3" }
        } }
    }));
    let listing = opencode_listing(&models, Some(&gateways), Some(&digest));
    let ids: Vec<_> = listing
        .models
        .iter()
        .map(|model| model.id.as_str())
        .collect();
    assert_eq!(
        ids,
        [
            "opencode-go/glm-5.3",
            "opencode/claude-sonnet-5-5",
            "opencode/claude-opus-5-5",
            "opencode/gpt-5.3-codex-spark",
            "opencode/big-pickle",
        ],
        "gateways in OpenCode's order, the lab with the latest release first, \
         each lab newest first, deprecated left out"
    );
    let glm = model(&listing, "opencode-go/glm-5.3");
    assert_eq!(glm.group.as_deref(), Some("OpenCode Go"));
    assert_eq!(glm.lab.as_deref(), Some("Z.ai"));
    assert_eq!(glm.description.as_deref(), Some("Flagship GLM."));
    let opus = model(&listing, "opencode/claude-opus-5-5");
    let variants: Vec<_> = opus
        .variants
        .iter()
        .map(|variant| (variant.id.as_str(), variant.label.as_str()))
        .collect();
    assert_eq!(variants, [("none", "Off"), ("high", "High")]);
    assert!(opus.efforts.is_empty(), "variants are not efforts");
    assert!(opus.supports_images);
    assert_eq!(opus.lab.as_deref(), Some("Anthropic"));
    assert_eq!(opus.group.as_deref(), Some("OpenCode Console"));
    let spark = model(&listing, "opencode/gpt-5.3-codex-spark");
    assert_eq!(spark.lab.as_deref(), Some("OpenAI"), "trimmed family");
    assert_eq!(model(&listing, "opencode/big-pickle").lab, None);
}

#[test]
fn opencode_default_is_reported_or_unknown_never_guessed() {
    let meta = json!({"data":[{"id":"toggle", "providerID":"test", "variants":[{"id":"none"},{"id":"thinking"}]}]});
    let listing = opencode_listing(&meta, None, None);
    assert_eq!(listing.models[0].default_variant, None);
    let mut reported = meta;
    reported["data"][0]["defaultVariant"] = json!("thinking");
    assert_eq!(
        opencode_listing(&reported, None, None).models[0]
            .default_variant
            .as_deref(),
        Some("thinking")
    );
    reported["data"][0]["defaultVariant"] = json!("absent");
    assert_eq!(
        opencode_listing(&reported, None, None).models[0].default_variant,
        None
    );
}
