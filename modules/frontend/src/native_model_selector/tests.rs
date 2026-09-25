//! Model-selector interaction, label, and rendering tests.
//!
//! Extracted verbatim from `native_model_selector.rs` during the module split.

#![expect(
    clippy::float_cmp,
    reason = "test assertions compare the exact pixel arithmetic the UI performs; an epsilon would weaken the regression coverage"
)]
use super::render::gradient_highlights;
use super::state::{humanize_variant, rebase_selection_policy};
use super::*;

#[gpui::test]
fn clicks_in_engine_tabs_and_preview_do_not_dismiss_the_menu(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .unwrap(),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-native-model-selector-search")
            .is_none()
    );
    assert!(cx.debug_bounds("artisan-model-selector-retry").is_none());
    let tab = cx
        .debug_bounds("artisan-native-model-selector-engine-claude")
        .unwrap();
    cx.simulate_click(tab.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(view.read(app).state.is_open());
        assert_eq!(view.read(app).state.active_engine(), "claude");
        let picker = view.read(app);
        let surface = picker.engine_surface_bounds.borrow().unwrap();
        let expected_left = f64::from(f32::from(tab.left() - surface.left()));
        assert_eq!(
            picker.engine_indicator.borrow().indicator_left(),
            expected_left
        );
        assert_eq!(
            picker.engine_indicator_transition.borrow().unwrap().to_left,
            expected_left,
            "light animation coordinates must stay relative to the engine strip"
        );
    });
    let menu = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_MENU_SELECTOR)
        .unwrap();
    cx.simulate_click(
        point(menu.right() - px(15.0), menu.center().y),
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    cx.update(|_, app| assert!(view.read(app).state.is_open()));
    cx.simulate_click(
        point(menu.right() + px(15.0), menu.center().y),
        gpui::Modifiers::none(),
    );
    cx.run_until_parked();
    cx.update(|_, app| assert!(!view.read(app).state.is_open()));
}

#[gpui::test]
fn wheel_events_accumulate_once_and_precise_pixels_cancel_inertia(cx: &mut gpui::TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .unwrap(),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let row = cx
        .debug_bounds("artisan-native-model-selector-row-codex-sol")
        .unwrap();
    let expected = cx.update(|window, app| {
        app.set_reduce_motion(false);
        let picker = view.read(app);
        let maximum = f32::from(picker.menu_scroll.max_offset_for_scrollbar().y);
        assert!(maximum > 0.0, "the fixture must scroll");
        (-3.0 * f32::from(window.line_height())).clamp(-maximum, 0.0)
    });
    cx.simulate_event(ScrollWheelEvent {
        position: row.center(),
        delta: gpui::ScrollDelta::Lines(point(0.0, -3.0)),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.update(|_, app| assert_eq!(view.read(app).model_scroll.target(), expected));
    let expected_pixel = cx.update(|_, app| {
        let picker = view.read(app);
        (f32::from(picker.menu_scroll.scroll_px_offset_for_scrollbar().y) - 7.0)
            .clamp(-f32::from(picker.menu_scroll.max_offset_for_scrollbar().y), 0.0)
    });
    cx.simulate_event(ScrollWheelEvent {
        position: row.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-7.0))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.update(|_, app| {
        let picker = view.read(app);
        assert_eq!(f32::from(picker.menu_scroll.scroll_px_offset_for_scrollbar().y), expected_pixel);
        assert!(!picker.model_scroll.active());
    });
}
#[gpui::test]
fn option_hover_slides_across_rows_and_clears_on_surface_departure(cx: &mut gpui::TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .unwrap(),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let thinking = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
    cx.simulate_click(thinking.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| app.set_reduce_motion(false));
    let light = cx
        .debug_bounds("artisan-model-policy-option-Thinking-light")
        .unwrap();
    let medium = cx
        .debug_bounds("artisan-model-policy-option-Thinking-medium")
        .unwrap();
    cx.simulate_mouse_move(light.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.simulate_mouse_move(medium.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = view.read(app);
        let hover = picker.axis_hover.borrow();
        assert_eq!(hover.active_id(), Some("medium"));
        assert!(
            hover.transition().is_some(),
            "sibling row departure must not reset the slide"
        );
    });
    cx.simulate_mouse_move(point(px(950.0), px(750.0)), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| assert!(!view.read(app).axis_hover.borrow().visible()));
}
fn state_with_fixture_catalog() -> NativeModelSelectorState {
    NativeModelSelectorState::new(
        NativeModelCatalog::from_manifest_json(include_str!(
            "../../../../tests/fixtures/model_catalog.json"
        ))
        .expect("the discovery fixture must decode"),
        None,
    )
}

#[test]
fn hidden_harnesses_are_skipped_by_the_picker() {
    let snapshot = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .expect("fixture catalog");
    assert!(
        snapshot.manifest.harness("hermes").is_none(),
        "hermes is not a shipped harness"
    );

    // Hiding the first harness must move the default tab to the next
    // visible engine instead of leaving an empty panel.
    let mut snapshot = snapshot;
    for harness in &mut snapshot.manifest.harnesses {
        if harness.id == "codex" {
            harness.hidden = true;
        }
    }
    let state = NativeModelSelectorState::new(snapshot, None);
    assert_eq!(state.active_engine(), "claude");
}

#[test]
fn keyboard_commit_selects_policy_and_preserves_native_values() {
    let mut state = state_with_fixture_catalog();
    state.press_trigger();
    assert_eq!(state.highlighted_model_id(), Some("codex-astra"));
    state.handle_key(NativeModelSelectorKey::ArrowDown);
    let event = state
        .handle_key(NativeModelSelectorKey::Enter)
        .expect("enter commits the highlighted row");
    let NativeModelSelectorEvent::SelectPolicy(policy) = event else {
        panic!("expected a policy event");
    };
    assert_eq!(policy.model_id, "codex-sol");
    assert_eq!(policy.native_model_id, "gpt-5.6-sol");
    assert_eq!(
        policy
            .reasoning_effort
            .as_ref()
            .map(|value| value.native_value.as_str()),
        Some("low")
    );
    assert!(!state.is_open());
}

#[test]
fn escape_and_tab_cancel_preview_without_emitting_policy() {
    let mut state = state_with_fixture_catalog();
    state.press_trigger();
    state.handle_key(NativeModelSelectorKey::ArrowDown);
    state.handle_key(NativeModelSelectorKey::Escape);
    assert!(!state.is_open());

    state.press_trigger();
    state.handle_key(NativeModelSelectorKey::ArrowDown);
    state.handle_key(NativeModelSelectorKey::Tab);
    assert!(!state.is_open());
}

#[test]
fn favorite_event_is_explicit_and_does_not_fake_authoritative_state() {
    let mut state = state_with_fixture_catalog();
    let event = state
        .toggle_favorite("codex-sol")
        .expect("offline model emits favorite intent");
    assert_eq!(
        event,
        NativeModelSelectorEvent::SetFavorite(SetFavorite {
            model_id: "codex-sol".to_owned(),
            favorite: true,
        })
    );
    assert!(!state.snapshot().is_favorite("codex-sol"));
}

#[test]
fn offline_model_can_be_selected_without_runtime_readiness() {
    let mut state = state_with_fixture_catalog();
    assert!(!state.snapshot().selectability("codex-sol").is_available());
    state.press_trigger();
    state.preview_model("codex-sol");
    assert_eq!(state.previewed_model_id(), Some("codex-sol"));
    assert!(matches!(
        state.select_model("codex-sol"),
        Some(NativeModelSelectorEvent::SelectPolicy(policy)) if policy.model_id == "codex-sol"
    ));
    assert!(!state.is_open());
}

#[test]
fn invalid_option_is_rejected_without_runtime_readiness() {
    let mut state = state_with_fixture_catalog();
    state.press_trigger();
    state.preview_model("codex-sol");
    assert!(matches!(
        state.choose_option(NativePolicyAxis::Thinking, "not-in-the-catalog"),
        Err(NativePolicyValidationError::InvalidThinkingOption { .. })
    ));
    assert!(matches!(
        state.choose_option(NativePolicyAxis::Variant, "not-in-the-catalog"),
        Err(NativePolicyValidationError::UnknownModel(model_id)) if model_id == "not-in-the-catalog"
    ));
}

#[test]
fn xhigh_thinking_value_uses_source_label() {
    let wire_id = "xhigh";
    assert_eq!(humanize_variant(wire_id), "Extra High");
}

#[test]
fn full_trigger_label_includes_variable_context_and_keeps_no_separators() {
    let snapshot = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .expect("real catalog");
    let mut policy = snapshot
        .selection_policy_for_model("codex-sol")
        .expect("codex policy");
    policy.reasoning_effort = Some(NativeOptionValue {
        id: "xhigh".to_owned(),
        native_value: "xhigh".to_owned(),
    });
    policy.speed = Some(NativeOptionValue {
        id: "fast".to_owned(),
        native_value: "fast".to_owned(),
    });
    policy.context_window = Some(crate::native_model_catalog::NativeContextSelection {
        id: "extended".to_owned(),
        native_suffix: "1m".to_owned(),
        native_config: Some(crate::native_model_catalog::NativeContextConfig {
            model_context_window: 1_050_000,
        }),
    });

    let state = NativeModelSelectorState::new(snapshot, Some(policy));
    let label = state.trigger_label();
    assert_eq!(label.plain_text(), "GPT 5.6 Sol 1M Extra High Fast");
    assert_eq!(label.tokens().len(), 4);
    assert_eq!(label.tokens()[0].role, NativeModelLabelRole::Name);
    assert_eq!(
        label.tokens()[3].role,
        NativeModelLabelRole::Gradient(crate::speed_presentation::FAST_GRADIENT)
    );
    let plain = label.plain_text();
    for separator in ['\u{b7}', '\u{2022}', '|'] {
        assert!(
            !plain.contains(separator),
            "label must not separate: {plain:?}"
        );
    }
}

#[test]
fn million_token_context_uses_millions_even_when_catalog_says_1000k() {
    let catalog_json = include_str!("../../../../tests/fixtures/model_catalog.json")
        .replace("\"label\": \"1M\"", "\"label\": \"1000K\"");
    let snapshot = NativeModelCatalog::from_manifest_json(&catalog_json).expect("real catalog");
    let mut policy = snapshot
        .selection_policy_for_model("codex-sol")
        .expect("codex policy");
    policy.context_window = Some(crate::native_model_catalog::NativeContextSelection {
        id: "extended".to_owned(),
        native_suffix: "1m".to_owned(),
        native_config: None,
    });

    let label = model_display_label(&snapshot, &policy);
    assert!(label.plain_text().contains(" 1M"));
    assert!(!label.plain_text().contains("1000K"));
}

#[test]
fn full_trigger_label_omits_missing_context_and_default_speed() {
    let snapshot = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .expect("real catalog");
    let mut policy = snapshot
        .selection_policy_for_model("codex-gpt-5-5")
        .expect("gpt-5.5 policy");
    policy.reasoning_effort = Some(NativeOptionValue {
        id: "xhigh".to_owned(),
        native_value: "xhigh".to_owned(),
    });
    // The model carries no variable window, and standard is its default
    // speed, so both tokens stay absent.
    let label = model_display_label(&snapshot, &policy);
    assert_eq!(label.plain_text(), "GPT 5.5 Extra High");
    assert_eq!(label.tokens().len(), 2);
    assert!(
        label
            .tokens()
            .iter()
            .all(|token| !matches!(token.role, NativeModelLabelRole::Gradient(_)))
    );
}

#[test]
fn superfast_speed_token_uses_the_neon_gradient() {
    let mut snapshot = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .expect("real catalog");
    let model = snapshot
        .manifest
        .models
        .iter_mut()
        .find(|model| model.id == "codex-sol")
        .expect("codex-sol exists");
    model
        .capabilities
        .speed_options
        .push(crate::native_model_catalog::NativeSpeedOption {
            availability: "always".to_owned(),
            consumption_basis: "standard".to_owned(),
            consumption_multiplier: None,
            input_consumption_multiplier: None,
            output_consumption_multiplier: None,
            default: false,
            description: "Fastest tier.".to_owned(),
            disabled: None,
            id: "superfast".to_owned(),
            label: "Provider accelerated".to_owned(),
            native_value: "superfast".to_owned(),
            source_url: None,
            speed_multiplier: None,
            verified_at: None,
        });
    let mut policy = snapshot
        .selection_policy_for_model("codex-sol")
        .expect("codex policy");
    policy.speed = Some(NativeOptionValue {
        id: "superfast".to_owned(),
        native_value: "superfast".to_owned(),
    });
    // Pin the effort so this test measures the speed token/gradient, not
    // whatever thinking default the catalog currently ships.
    policy.reasoning_effort = Some(NativeOptionValue {
        id: "high".to_owned(),
        native_value: "high".to_owned(),
    });

    let label = model_display_label(&snapshot, &policy);
    assert_eq!(label.plain_text(), "GPT 5.6 Sol 272K High Superfast");
    assert_eq!(
        label.tokens().last().expect("speed token").role,
        NativeModelLabelRole::Gradient(crate::speed_presentation::SUPERFAST_GRADIENT)
    );
}

#[test]
fn gradient_highlights_cover_every_character_left_to_right() {
    let text = "Fast";
    let highlights = gradient_highlights(text, crate::speed_presentation::FAST_GRADIENT);
    assert_eq!(highlights.len(), text.len());
    let mut cursor = 0;
    for (range, _) in &highlights {
        assert_eq!(range.start, cursor);
        cursor = range.end;
    }
    assert_eq!(cursor, text.len());
    let expected_start = Some(rgb_to_hsla(rgb(
        crate::speed_presentation::FAST_GRADIENT.start()
    )));
    let expected_end = Some(rgb_to_hsla(rgb(
        crate::speed_presentation::FAST_GRADIENT.end()
    )));
    assert_eq!(highlights[0].1.color, expected_start);
    assert_eq!(highlights[3].1.color, expected_end);
}

#[gpui::test]
fn trigger_mounts_the_full_label_with_a_gradient_speed_token(cx: &mut gpui::TestAppContext) {
    let snapshot = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .expect("real catalog");
    let mut policy = snapshot
        .selection_policy_for_model("codex-sol")
        .expect("codex policy");
    policy.reasoning_effort = Some(NativeOptionValue {
        id: "xhigh".to_owned(),
        native_value: "xhigh".to_owned(),
    });
    policy.speed = Some(NativeOptionValue {
        id: "fast".to_owned(),
        native_value: "fast".to_owned(),
    });
    policy.context_window = Some(crate::native_model_catalog::NativeContextSelection {
        id: "extended".to_owned(),
        native_suffix: "1m".to_owned(),
        native_config: Some(crate::native_model_catalog::NativeContextConfig {
            model_context_window: 1_050_000,
        }),
    });

    let (view, cx) = cx.add_window_view(move |_, cx| {
        NativeModelSelector::new(snapshot, Some(policy), ThemeMode::Dark, cx)
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .expect("trigger paints with the full label");
    assert!(f32::from(trigger.size.width) > 0.0);
    cx.update(|_, app| {
        assert_eq!(
            view.read(app).state.trigger_label().plain_text(),
            "GPT 5.6 Sol 1M Extra High Fast"
        );
    });
}

#[test]
fn rebase_keeps_explicit_native_profile_for_saved_policies() {
    let snapshot = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .expect("real catalog");
    let mut policy = snapshot
        .selection_policy_for_model("codex-sol")
        .expect("codex policy");
    policy.profile_id = Some("default".to_owned());
    let rebased = rebase_selection_policy(&snapshot, &policy).expect("rebased");
    assert_eq!(rebased.model_id, "codex-sol");
    assert_eq!(rebased.profile_id.as_deref(), Some("default"));

    let mut state = NativeModelSelectorState::new(snapshot, None);
    assert!(state.set_policy(Some(policy)));
    assert_eq!(
        state.policy().and_then(|set| set.profile_id.as_deref()),
        Some("default")
    );
}

#[test]
fn offline_rows_keep_runtime_readiness_out_of_picker_selection() {
    let mut state = state_with_fixture_catalog();
    let groups = state.model_groups();
    let row = groups
        .iter()
        .flat_map(|group| &group.models)
        .find(|model| model.id == "codex-sol")
        .expect("catalog model remains readable while offline");
    assert!(!row.available);
    assert!(!state.model_definition_disabled("codex-sol"));
    assert!(state.select_model("codex-sol").is_some());
    assert!(state.local_error.is_none());
}

#[gpui::test]
fn pointer_click_selects_offline_model_without_runtime_configuration(
    cx: &mut gpui::TestAppContext,
) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .expect("real catalog"),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let row = cx
        .debug_bounds("artisan-native-model-selector-row-codex-sol")
        .expect("offline model row is painted");
    cx.simulate_click(row.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let selector = view.read(app);
        assert!(!selector.state.is_open());
        assert_eq!(selector.state.selected_model_id(), Some("codex-sol"));
    });
}
#[gpui::test]
fn policy_popup_floats_and_accepts_clicks_outside_parent_bounds(cx: &mut gpui::TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .unwrap(),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let menu = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_MENU_SELECTOR)
        .unwrap();
    assert_eq!(menu.size.width, px(480.0));
    assert!(menu.size.height <= px(264.0));
    let control = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
    assert_eq!(control.size.height, px(24.0));
    cx.simulate_click(control.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert_eq!(
        cx.debug_bounds(NATIVE_MODEL_SELECTOR_MENU_SELECTOR)
            .unwrap(),
        menu
    );
    let popup = cx.debug_bounds("artisan-model-policy-options").unwrap();
    cx.update(|_, app| {
        assert_eq!(
            f32::from(view.read(app).axis_menu_scroll.max_offset().y),
            0.0,
            "fitting policy options must not gain scroll extent from visual layers"
        );
    });
    let ids = cx.update(|_, app| {
        view.read(app)
            .axis_options(NativePolicyAxis::Thinking, &view.read(app).preview_view())
            .into_iter()
            .filter(|option| !option.disabled)
            .map(|option| option.id)
            .collect::<Vec<_>>()
    });
    let (id, bounds) = ids
        .into_iter()
        .find_map(|id| {
            let bounds = cx.debug_bounds(Box::leak(
                format!("artisan-model-policy-option-Thinking-{id}").into_boxed_str(),
            ))?;
            (popup.contains(&bounds.center()) && !menu.contains(&bounds.center()))
                .then_some((id, bounds))
        })
        .expect("an option is visible beyond the parent picker");
    cx.simulate_click(bounds.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = view.read(app);
        assert!(picker.state.is_open());
        assert!(picker.state.open_axis.is_none());
        assert_eq!(
            picker
                .state
                .policy()
                .unwrap()
                .reasoning_effort
                .as_ref()
                .unwrap()
                .id,
            id
        );
    });
}

#[gpui::test]
fn policy_option_tooltip_is_a_full_side_overlay(cx: &mut gpui::TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .expect("real catalog"),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let thinking = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
    cx.simulate_click(thinking.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    let option_id = cx.update(|_, app| {
        view.read(app)
            .axis_options(NativePolicyAxis::Thinking, &view.read(app).preview_view())
            .into_iter()
            .find(|option| option.id == "ultra")
            .map(|option| option.id)
            .expect("the catalog fixture has a described policy option")
    });
    let option = cx
        .debug_bounds(Box::leak(
            format!("artisan-model-policy-option-Thinking-{option_id}").into_boxed_str(),
        ))
        .expect("described option is painted");
    cx.update(|_, app| app.set_reduce_motion(false));
    cx.simulate_mouse_move(option.center(), None, gpui::Modifiers::none());
    cx.run_until_parked();
    cx.executor()
        .advance_clock(Duration::from_millis(PICKER_TOOLTIP_SHOW_DELAY_MS));
    cx.run_until_parked();

    let tooltip = cx
        .debug_bounds("artisan-native-model-selector-option-tooltip")
        .expect("tooltip is mounted after the source delay");
    assert!(
        tooltip.left() >= option.right() + px(OPTION_TOOLTIP_GAP_PX)
            || tooltip.right() <= option.left() - px(OPTION_TOOLTIP_GAP_PX),
        "tooltip should be side-anchored with the source 8px gap"
    );
    assert!(
        tooltip.size.height >= px(96.0),
        "wrapped advisory/description must not be clipped to one line"
    );
    assert!(tooltip.size.width <= px(OPTION_TOOLTIP_WIDTH_PX));
}

#[gpui::test]
fn settings_exit_can_be_reopened_and_switched_before_completion(cx: &mut gpui::TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .unwrap(),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();

    cx.update(|window, app| {
        view.update(app, |picker, cx| {
            picker.toggle_axis(NativePolicyAxis::Thinking, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| app.set_reduce_motion(false));
    cx.update(|window, app| {
        view.update(app, |picker, cx| {
            picker.toggle_axis(NativePolicyAxis::Thinking, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = view.read(app);
        assert!(picker.state.open_axis.is_none());
        assert_eq!(
            picker.axis_menu_motion.borrow().phase(),
            PickerMenuPhase::Closing
        );
        assert!(!picker.axis_is_interactive(NativePolicyAxis::Thinking));
    });
    assert!(
        cx.debug_bounds("artisan-model-policy-options").is_some(),
        "exit remains mounted"
    );
    cx.update(|window, app| {
        view.update(app, |picker, cx| {
            picker.toggle_axis(NativePolicyAxis::Thinking, window, cx);
        });
    });
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(
            view.read(app)
                .state
                .is_axis_open(NativePolicyAxis::Thinking)
        );
    });
    cx.executor().advance_clock(Duration::from_millis(110));
    cx.run_until_parked();
    cx.update(|_, app| {
        assert_eq!(
            view.read(app).axis_menu_motion.borrow().phase(),
            PickerMenuPhase::Open
        );
    });
    cx.update(|window, app| {
        view.update(app, |picker, cx| {
            picker.toggle_axis(NativePolicyAxis::Thinking, window, cx);
        });
    });
    cx.run_until_parked();

    cx.update(|window, app| {
        view.update(app, |picker, cx| {
            picker.toggle_axis(NativePolicyAxis::Speed, window, cx);
        });
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(110));
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = view.read(app);
        assert!(picker.state.is_axis_open(NativePolicyAxis::Speed));
        assert_eq!(
            picker.axis_menu_motion.borrow().phase(),
            PickerMenuPhase::Open
        );
    });
    cx.update(|window, app| {
        view.update(app, |picker, cx| {
            picker.toggle_axis(NativePolicyAxis::Speed, window, cx);
        });
    });
    cx.run_until_parked();
    cx.executor().advance_clock(Duration::from_millis(110));
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-model-policy-options").is_none(),
        "finished exit unmounts"
    );
}

#[gpui::test]
fn settings_scroll_only_when_the_real_options_exceed_the_viewport(cx: &mut gpui::TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .unwrap(),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    let thinking = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
    cx.simulate_click(thinking.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| assert_eq!(view.read(app).axis_menu_scroll.max_offset().y, px(0.0)));
    cx.simulate_resize(gpui::size(px(1000.0), px(280.0)));
    cx.run_until_parked();
    cx.update(|_, app| {
        assert!(
            view.read(app).axis_menu_scroll.max_offset().y > px(0.0),
            "short windows retain real scrolling"
        );
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    cx.update(|_, app| assert_eq!(view.read(app).axis_menu_scroll.max_offset().y, px(0.0)));
}

#[test]
fn model_projections_survive_redraws_and_preview_changes() {
    let mut state = state_with_fixture_catalog();
    let groups = state.model_groups();
    for model in groups.iter().flat_map(|group| &group.models) {
        state.preview_model(&model.id);
        assert!(Rc::ptr_eq(&groups, &state.model_groups()));
    }
    state.set_active_engine("claude".to_owned());
    let switched = state.model_groups();
    assert!(!Rc::ptr_eq(&groups, &switched));
    assert!(
        switched
            .iter()
            .flat_map(|group| &group.models)
            .all(|model| model.engine_id == "claude")
    );
    assert!(Rc::ptr_eq(&switched, &state.model_groups()));
    state.set_query("no matching model expected");
    assert!(state.model_groups().is_empty());
    state.set_query("");
    assert!(!state.model_groups().is_empty());
}

#[test]
fn model_projections_refresh_for_selection_and_authoritative_catalog_changes() {
    let mut state = state_with_fixture_catalog();
    let before = state.model_groups();
    let model_id = before
        .iter()
        .flat_map(|group| &group.models)
        .find(|model| !model.selected && !state.model_definition_disabled(&model.id))
        .expect("another selectable model")
        .id
        .clone();
    let policy = state
        .snapshot()
        .selection_policy_for_model(&model_id)
        .unwrap();
    assert!(state.set_policy(Some(policy)));
    let selected = state.model_groups();
    assert!(!Rc::ptr_eq(&before, &selected));
    assert!(
        selected
            .iter()
            .flat_map(|group| &group.models)
            .any(|model| model.id == model_id && model.selected)
    );
    let mut snapshot = state.snapshot().clone();
    snapshot.favorite_ids = vec![model_id.clone()];
    snapshot.runnable_harness_ids = vec![state.active_engine().to_owned()];
    state.set_snapshot(snapshot);
    let refreshed = state.model_groups();
    assert!(!Rc::ptr_eq(&selected, &refreshed));
    let row = refreshed
        .iter()
        .flat_map(|group| &group.models)
        .find(|model| model.id == model_id)
        .unwrap();
    assert!(row.favorite);
    assert!(row.available);
}

#[gpui::test]
fn harness_switch_animation_settles_without_rebuilding_catalog(cx: &mut gpui::TestAppContext) {
    let (view, cx) = cx.add_window_view(|_, cx| {
        NativeModelSelector::new(
            NativeModelCatalog::from_manifest_json(include_str!(
                "../../../../tests/fixtures/model_catalog.json"
            ))
            .unwrap(),
            None,
            ThemeMode::Dark,
            cx,
        )
    });
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    for selector in [
        "artisan-native-model-selector-engine-claude",
        "artisan-native-model-selector-engine-cursor",
        "artisan-native-model-selector-engine-codex",
    ] {
        let tab = cx.debug_bounds(selector).unwrap();
        cx.simulate_click(tab.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let groups = cx.update(|_, app| view.read(app).state.model_groups());
        for _ in 0..60 {
            cx.executor().advance_clock(Duration::from_millis(16));
            cx.update(|window, app| {
                window.simulate_next_frame(app);
            });
            cx.run_until_parked();
            cx.update(|_, app| {
                assert!(Rc::ptr_eq(&groups, &view.read(app).state.model_groups()));
            });
        }
        cx.update(|window, app| {
            assert_eq!(
                window.simulate_next_frame(app),
                0,
                "settled tabs must stop requesting frames: {selector}"
            );
        });
    }
}

#[gpui::test]
fn large_catalog_groups_variants_virtualizes_rows_and_collapses(cx: &mut gpui::TestAppContext) {
    cx.update(|app| app.set_reduce_motion(true));
    let mut catalog = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .unwrap();
    let template = catalog.manifest.models[0].clone();
    catalog.manifest.models = vec![template.clone()];
    for index in 0..300 {
        for variant in [None, Some("low"), Some("high")] {
            let mut model = template.clone();
            model.id = format!("fixture-{index}-{}", variant.unwrap_or("default"));
            model.name = format!("Model {index}");
            model.native_model_id = format!("model-{index}");
            model.native_selection = Some(artisan_catalog::NativeModelSelection {
                model_id: model.native_model_id.clone(),
                provider_route_id: "go".to_owned(),
                variant_id: variant.map(str::to_owned),
            });
            catalog.manifest.models.push(model);
        }
    }
    let (view, cx) =
        cx.add_window_view(|_, cx| NativeModelSelector::new(catalog, None, ThemeMode::Dark, cx));
    cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
    cx.run_until_parked();
    let trigger = cx
        .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
        .unwrap();
    cx.simulate_click(trigger.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| {
        let picker = view.read(app);
        let groups = picker.state.model_groups();
        assert_eq!(
            groups
                .iter()
                .find(|group| group.id == "go")
                .unwrap()
                .models
                .len(),
            300
        );
    });
    assert!(
        cx.debug_bounds("artisan-native-model-selector-row-fixture-299-default")
            .is_none(),
        "offscreen rows must not be rendered"
    );
    let engine_tab = cx
        .debug_bounds("artisan-native-model-selector-engine-codex")
        .unwrap();
    let before_scroll = cx.update(|_, app| view.read(app).menu_scroll.scroll_px_offset_for_scrollbar());
    cx.simulate_event(ScrollWheelEvent {
        position: engine_tab.center(),
        delta: gpui::ScrollDelta::Pixels(point(px(-120.0), px(-120.0))),
        modifiers: gpui::Modifiers::none(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    cx.run_until_parked();
    assert_eq!(
        cx.debug_bounds("artisan-native-model-selector-engine-codex")
            .unwrap(),
        engine_tab
    );
    cx.update(|_, app| assert_eq!(view.read(app).menu_scroll.scroll_px_offset_for_scrollbar(), before_scroll));
    let header = cx.debug_bounds("model-group-go").unwrap();
    let first_model = cx.debug_bounds("artisan-native-model-selector-row-fixture-0-default").unwrap();
    assert_eq!(f32::from(header.size.height), 28.0);
    assert_eq!(f32::from(first_model.top() - header.bottom()), 3.0);
    cx.simulate_click(header.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    cx.update(|_, app| assert!(view.read(app).state.group_collapsed("go")));
    assert!(
        cx.debug_bounds("artisan-native-model-selector-row-fixture-0-default")
            .is_none()
    );
    let header = cx.debug_bounds("model-group-go").unwrap();
    cx.simulate_click(header.center(), gpui::Modifiers::none());
    cx.run_until_parked();
    assert!(
        cx.debug_bounds("artisan-native-model-selector-row-fixture-0-default")
            .is_some()
    );
}
