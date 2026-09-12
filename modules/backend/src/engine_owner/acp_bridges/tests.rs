    use super::*;
    use artisan_domain::ApprovalKind;

    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper takes JSON fixtures by value for call-site symmetry with serde_json::json!"
    )]
    fn permission_params(tool_call: Value, options: Value) -> Value {
        serde_json::json!({
            "toolCall": tool_call,
            "options": options,
        })
    }

    fn standard_options() -> Value {
        serde_json::json!([
            { "kind": "allow_once", "optionId": "allow-1", "label": "Allow" },
            { "kind": "reject_once", "optionId": "reject-1", "label": "Deny" },
        ])
    }

    #[test]
    fn command_approval_maps_command_cwd_reason_options() {
        let pending = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-1",
                "kind": "execute",
                "title": "Run tests",
                "rawInput": { "command": "cargo test", "cwd": "C:\\work" },
            }),
            standard_options(),
        ))
        .expect("command approval");
        assert_eq!(pending.provider_id(), "tool-1");
        assert_eq!(pending.description(), "Run tests");
        assert_eq!(pending.request().kind(), ApprovalKind::Command);
        assert_eq!(pending.request().command_text(), Some("cargo test"));
        assert_eq!(pending.request().cwd(), Some("C:\\work"));
        assert_eq!(pending.request().reason(), Some("Run tests"));
        assert_eq!(
            answer_permission(&pending, true),
            PermissionOutcome::Selected {
                option_id: "allow-1".to_owned(),
            }
        );
        assert_eq!(
            answer_permission(&pending, false),
            PermissionOutcome::Selected {
                option_id: "reject-1".to_owned(),
            }
        );
    }

    #[test]
    fn file_change_and_action_kinds_map() {
        for kind in ["edit", "delete", "move"] {
            let pending = normalize_permission_request(&permission_params(
                serde_json::json!({
                    "toolCallId": format!("tool-{kind}"),
                    "kind": kind,
                    "title": "Touch file",
                }),
                standard_options(),
            ))
            .expect("file approval");
            assert_eq!(pending.request().kind(), ApprovalKind::FileChange);
            assert_eq!(pending.request().command_text(), None);
            assert_eq!(pending.request().reason(), Some("Touch file"));
        }
        let other = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-web",
                "kind": "fetch",
                "title": "Fetch page",
            }),
            standard_options(),
        ))
        .expect("action approval");
        assert_eq!(other.request().kind(), ApprovalKind::Action);
        assert_eq!(other.request().reason(), Some("Fetch page"));
    }

    #[test]
    fn execute_without_command_gates_as_action() {
        let pending = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-nocmd",
                "kind": "execute",
                "title": "Mystery run",
                "rawInput": {},
            }),
            standard_options(),
        ))
        .expect("action fallback");
        assert_eq!(pending.request().kind(), ApprovalKind::Action);
        assert_eq!(pending.request().reason(), Some("Mystery run"));
    }

    #[test]
    fn missing_options_answer_cancelled_never_defaults() {
        let pending = normalize_permission_request(&serde_json::json!({
            "toolCall": {
                "toolCallId": "tool-bare",
                "kind": "execute",
                "rawInput": { "command": "ls" },
            },
        }))
        .expect("options stay optional");
        assert_eq!(pending.description(), DEFAULT_APPROVAL_DESCRIPTION);
        assert_eq!(pending.request().reason(), None);
        assert_eq!(
            answer_permission(&pending, true),
            PermissionOutcome::Cancelled
        );
        assert_eq!(
            answer_permission(&pending, false),
            PermissionOutcome::Cancelled
        );
    }

    #[test]
    fn malformed_permission_payloads_rejected() {
        for params in [
            serde_json::json!(null),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({ "toolCall": null }),
            serde_json::json!({ "toolCall": [] }),
            serde_json::json!({ "toolCall": { "kind": "execute" } }),
            serde_json::json!({ "toolCall": { "toolCallId": "", "kind": "execute" } }),
        ] {
            assert_eq!(
                normalize_permission_request(&params).expect_err("malformed"),
                BridgeError::MalformedRequest,
                "params: {params}"
            );
        }
        // Well-shaped but domain-rejected: empty command text is InvalidRequest
        // only when it would build a command; here the fallback applies, so an
        // over-long reason exercises the invalid path instead.
        let long_reason = "r".repeat(4_096);
        assert_eq!(
            normalize_permission_request(&permission_params(
                serde_json::json!({
                    "toolCallId": "tool-long",
                    "kind": "fetch",
                    "title": long_reason,
                }),
                standard_options(),
            ))
            .expect_err("oversized reason"),
            BridgeError::InvalidRequest
        );
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper takes a JSON fixture by value for call-site symmetry with serde_json::json!"
    )]
    fn elicitation_params(properties: Value, message: &str) -> Value {
        serde_json::json!({
            "mode": "form",
            "message": message,
            "requestedSchema": { "properties": properties },
        })
    }

    #[test]
    fn single_select_question_maps_options_and_text() {
        let pending = normalize_elicitation_request(
            "elicit-1",
            &elicitation_params(
                serde_json::json!({
                    "color": {
                        "type": "string",
                        "title": "Pick a color",
                        "description": "Choose wisely",
                        "oneOf": ["red", "green"],
                    },
                }),
                "fallback message",
            ),
        )
        .expect("single select");
        assert_eq!(pending.provider_id(), "elicit-1");
        assert_eq!(pending.questions().len(), 1);
        let question = &pending.questions()[0];
        assert_eq!(question.value_type(), ElicitedType::Text);
        assert_eq!(question.input.text, "Choose wisely");
        assert_eq!(question.input.header.as_deref(), Some("Pick a color"));
        assert!(!question.input.multi_select);
        let options = question.input.options.as_ref().expect("options");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].label(), "red");
        let answers = BTreeMap::from([("color".to_owned(), vec!["green".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &answers).expect("encode"),
            serde_json::json!({ "color": "green" })
        );
    }

    #[test]
    fn multi_select_question_encodes_arrays() {
        let pending = normalize_elicitation_request(
            "elicit-2",
            &elicitation_params(
                serde_json::json!({
                    "tags": {
                        "type": "array",
                        "title": "Tags",
                        "items": { "enum": ["a", "b"] },
                    },
                }),
                "pick tags",
            ),
        )
        .expect("multi select");
        let question = &pending.questions()[0];
        assert_eq!(question.value_type(), ElicitedType::MultiText);
        assert!(question.input.multi_select);
        assert_eq!(question.input.text, "Tags");
        let answers = BTreeMap::from([("tags".to_owned(), vec!["a".to_owned(), "b".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &answers).expect("encode"),
            serde_json::json!({ "tags": ["a", "b"] })
        );
    }

    #[test]
    fn free_form_question_has_no_options() {
        let pending = normalize_elicitation_request(
            "elicit-3",
            &elicitation_params(
                serde_json::json!({
                    "notes": { "type": "string", "title": "Notes" },
                }),
                "tell me",
            ),
        )
        .expect("free form");
        let question = &pending.questions()[0];
        assert_eq!(question.value_type(), ElicitedType::Text);
        assert_eq!(question.input.options, None);
        assert!(!question.input.multi_select);
        let answers = BTreeMap::from([("notes".to_owned(), vec!["hi".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &answers).expect("encode"),
            serde_json::json!({ "notes": "hi" })
        );
        let missing = BTreeMap::new();
        assert_eq!(
            answer_elicitation(&pending, &missing).expect("defaults"),
            serde_json::json!({ "notes": "" })
        );
    }

    #[test]
    fn scalar_types_encode_with_validation() {
        let pending = normalize_elicitation_request(
            "elicit-4",
            &elicitation_params(
                serde_json::json!({
                    "enabled": { "type": "boolean", "title": "On?" },
                    "count": { "type": "integer", "title": "N" },
                    "ratio": { "type": "number", "title": "R" },
                }),
                "scalars",
            ),
        )
        .expect("scalars");
        let answers = BTreeMap::from([
            ("enabled".to_owned(), vec!["TRUE".to_owned()]),
            ("count".to_owned(), vec!["42".to_owned()]),
            ("ratio".to_owned(), vec![String::new()]),
        ]);
        let content = answer_elicitation(&pending, &answers).expect("encode");
        assert_eq!(content.get("enabled"), Some(&Value::Bool(true)));
        assert_eq!(content.get("count"), Some(&serde_json::json!(42.0)));
        assert_eq!(content.get("ratio"), Some(&serde_json::json!(0.0)));
        let bad = BTreeMap::from([("count".to_owned(), vec!["many".to_owned()])]);
        assert_eq!(
            answer_elicitation(&pending, &bad).expect_err("invalid number"),
            BridgeError::InvalidAnswer
        );
    }

    #[test]
    fn object_options_keep_title_const_labels_and_skip_junk() {
        let pending = normalize_elicitation_request(
            "elicit-5",
            &elicitation_params(
                serde_json::json!({
                    "choice": {
                        "type": "string",
                        "title": "Pick",
                        "oneOf": [
                            { "title": "First", "description": "The first" },
                            { "const": "second" },
                            42,
                            { "description": "no label" },
                        ],
                    },
                }),
                "msg",
            ),
        )
        .expect("object options");
        let options = pending.questions()[0]
            .input
            .options
            .as_ref()
            .expect("options");
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].label(), "First");
        assert_eq!(options[0].description(), Some("The first"));
        assert_eq!(options[1].label(), "second");
        assert_eq!(options[1].description(), None);
    }

    #[test]
    fn malformed_elicitation_payloads_rejected() {
        assert_eq!(
            normalize_elicitation_request(
                "e",
                &serde_json::json!({ "mode": "dialog", "requestedSchema": {} }),
            )
            .expect_err("non-form"),
            BridgeError::UnsupportedMode
        );
        for params in [
            serde_json::json!(null),
            serde_json::json!({ "mode": "form" }),
            serde_json::json!({ "mode": "form", "requestedSchema": [] }),
            serde_json::json!({
                "mode": "form",
                "requestedSchema": { "properties": [] },
            }),
            serde_json::json!({
                "mode": "form",
                "requestedSchema": { "properties": { "q": [] } },
            }),
        ] {
            assert_eq!(
                normalize_elicitation_request("e", &params).expect_err("malformed"),
                BridgeError::MalformedRequest,
                "params: {params}"
            );
        }
        assert_eq!(
            normalize_elicitation_request("", &elicitation_params(serde_json::json!({}), "m"),)
                .expect_err("empty provider id"),
            BridgeError::MalformedRequest
        );
        assert_eq!(
            normalize_elicitation_request(
                "e",
                &elicitation_params(serde_json::json!({ "bad id": { "type": "string" } }), "m",),
            )
            .expect_err("bad question id"),
            BridgeError::InvalidRequest
        );
        assert_eq!(
            normalize_elicitation_request(
                "e",
                &elicitation_params(
                    serde_json::json!({
                        "q": {
                            "type": "string",
                            "title": "T",
                            "oneOf": [{ "title": "" }],
                        },
                    }),
                    "m",
                ),
            )
            .expect_err("empty option label"),
            BridgeError::InvalidRequest
        );
    }

    #[test]
    fn approval_answers_are_idempotent_then_conflicting() {
        let mut table = PendingBridgeTable::new();
        assert!(table.is_empty());
        let pending = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "tool-7",
                "kind": "execute",
                "title": "Run",
                "rawInput": { "command": "make" },
            }),
            standard_options(),
        ))
        .expect("normalize");
        table.insert_approval(8, pending).expect("insert");
        assert_eq!(table.len(), 1);
        let first = table.answer_approval("tool-7", true).expect("answer");
        let repeat = table.answer_approval("tool-7", true).expect("idempotent");
        assert_eq!(first, repeat);
        assert_eq!(
            table
                .answer_approval("tool-7", false)
                .expect_err("conflict"),
            BridgeError::AnswerConflict
        );
        assert_eq!(
            table.answer_approval("nope", true).expect_err("unknown"),
            BridgeError::UnknownRequest
        );
        assert!(table.remove("tool-7"));
        assert!(!table.remove("tool-7"));
        assert!(table.is_empty());
    }

    #[test]
    fn elicitation_answers_are_idempotent_then_conflicting() {
        let mut table = PendingBridgeTable::new();
        let pending = normalize_elicitation_request(
            "elicit-9",
            &elicitation_params(
                serde_json::json!({ "color": { "type": "string", "title": "C" } }),
                "msg",
            ),
        )
        .expect("normalize");
        table.insert_elicitation(8, pending).expect("insert");
        let answers = BTreeMap::from([("color".to_owned(), vec!["red".to_owned()])]);
        let first = table
            .answer_elicitation("elicit-9", &answers)
            .expect("answer");
        assert_eq!(first, serde_json::json!({ "color": "red" }));
        let repeat = table
            .answer_elicitation("elicit-9", &answers)
            .expect("idempotent");
        assert_eq!(first, repeat);
        let changed = BTreeMap::from([("color".to_owned(), vec!["blue".to_owned()])]);
        assert_eq!(
            table
                .answer_elicitation("elicit-9", &changed)
                .expect_err("conflict"),
            BridgeError::AnswerConflict
        );
        assert_eq!(
            table
                .answer_approval("elicit-9", true)
                .expect_err("wrong kind"),
            BridgeError::WrongKind
        );
    }

    #[test]
    fn table_bound_rejects_new_identities() {
        let mut table = PendingBridgeTable::new();
        let first = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "a",
                "kind": "action",
                "title": "first",
            }),
            standard_options(),
        ))
        .expect("normalize");
        table.insert_approval(1, first).expect("insert");
        // Replacing the same identity must not count against the bound.
        let again = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "a",
                "kind": "action",
                "title": "again",
            }),
            standard_options(),
        ))
        .expect("normalize");
        table
            .insert_approval(1, again)
            .expect("replace under bound");
        assert_eq!(table.len(), 1);
        assert_eq!(
            table.answer_approval("a", true).expect("answer replaced"),
            PermissionOutcome::Selected {
                option_id: "allow-1".to_owned(),
            }
        );
        let extra = normalize_permission_request(&permission_params(
            serde_json::json!({
                "toolCallId": "overflow",
                "kind": "action",
            }),
            standard_options(),
        ))
        .expect("normalize");
        assert_eq!(
            table.insert_approval(1, extra).expect_err("table full"),
            BridgeError::TableFull
        );
    }

    #[test]
    fn completion_projection_accumulates_and_completes() {
        let mut state = AcpCompletionState::new();
        state
            .push_message_text(1_024, "m2", "hello ")
            .expect("message");
        state
            .push_message_text(1_024, "m1", "first")
            .expect("message");
        state
            .push_message_text(1_024, "m2", "world")
            .expect("append");
        state
            .push_message_text(1_024, "m1", "")
            .expect("empty delta ignored");
        state.push_thought_text(1_024, "t1", "").expect("touch");
        state
            .push_thought_text(1_024, "t1", "hmm")
            .expect("thought");
        assert_eq!(
            state.completed_messages(),
            vec![
                ("m1".to_owned(), "first".to_owned()),
                ("m2".to_owned(), "hello world".to_owned()),
            ]
        );
        assert_eq!(
            state.completed_thoughts(),
            vec![("t1".to_owned(), "hmm".to_owned())]
        );
        assert_eq!(
            state
                .push_message_text(10, "m3", "way too long for ten bytes")
                .expect_err("cap"),
            BridgeError::CompletionTooLarge
        );
        assert_eq!(
            state
                .push_message_text(1_024, "", "x")
                .expect_err("empty id"),
            BridgeError::MalformedRequest
        );
    }

