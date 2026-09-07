//! External coverage for the shared text-input state/validation seam in
//! `artisan_ui`, exercising only the public API without a GPUI runtime:
//! canonical intake normalization, whole-value replacement with change
//! reporting, the audited empty-versus-blank disagreement, the typed
//! non-blank gate, and search-key derivation. No caret/selection/IME/
//! rendering behavior is claimed or tested here.
//!
//! Trim and blank checks follow Rust `str::trim`, not JavaScript
//! `String.prototype.trim`. The suite explicitly asserts the intentional
//! divergence on `U+FEFF`/`U+0085` and the shared `U+200B` handling; see
//! `artisan_ui::input_state` module docs. This aligns the seam with the
//! current native `MessageBody`/`ComposerState` stack.

use std::borrow::Cow;

use artisan_ui::input_state::{BlankTextError, NonBlankText, TextInputState, normalize_input};

#[test]
fn normalize_input_canonicalizes_intake_without_touching_visible_content() {
    assert_eq!(
        normalize_input("a\u{200B}b\r\nc\r"),
        "ab\nc\n",
        "U+200B is marker chrome; CR/CRLF fold to the canonical LF break"
    );
    assert_eq!(
        normalize_input("  padded \t text  "),
        "  padded \t text  ",
        "no trimming or collapsing in the stored value"
    );
    assert!(
        matches!(normalize_input("plain\ntext"), Cow::Borrowed(_)),
        "canonical input must not allocate"
    );
}

#[test]
fn controlled_replacement_reports_whether_the_canonical_value_changed() {
    let mut state = TextInputState::default();

    assert!(
        state.set_value("first\nsecond"),
        "a real change reports true"
    );
    assert_eq!(state.value(), "first\nsecond");

    assert!(
        !state.set_value("first\u{200B}\nsecond"),
        "replacement equal after normalization mutates nothing"
    );
    assert_eq!(state.value(), "first\nsecond");

    assert!(state.set_value("windows\r\ndraft"));
    assert_eq!(state.value(), "windows\ndraft");

    // The audited post-send clear, and its reported no-op repeat.
    assert!(state.set_value(""));
    assert!(state.is_empty() && state.is_blank());
    assert!(!state.set_value(""));
}

#[test]
fn placeholder_visibility_and_sendability_disagree_on_whitespace_only_drafts() {
    let mut state = TextInputState::default();
    assert!(state.is_empty() && state.is_blank());

    state.set_value("   ");
    assert!(
        !state.is_empty(),
        "`visible = value.length === 0`: the placeholder stays hidden"
    );
    assert!(
        state.is_blank(),
        "`draft.trim().length > 0`: still unsubmittable"
    );

    state.set_value(" hi ");
    assert!(!state.is_empty() && !state.is_blank());
}

#[test]
fn non_blank_handoff_preserves_the_payload_and_rejects_blanks() {
    let accepted = NonBlankText::new("  padded submission  ").expect("visible characters exist");
    assert_eq!(
        accepted.as_str(),
        "  padded submission  ",
        "accepted text is retained untrimmed"
    );

    for rejected in ["", " ", "\t\n "] {
        assert_eq!(NonBlankText::new(rejected), Err(BlankTextError));
    }

    // Boundary parity: U+200B is not whitespace to `str::trim` or the legacy
    // JavaScript trim; intake normalization removes it before this gate.
    assert!(NonBlankText::new("\u{200B}").is_ok());

    let mut state = TextInputState::default();
    state.set_value("hi");
    assert_eq!(state.non_blank().expect("non-blank").as_str(), "hi");
    state.set_value("   ");
    assert_eq!(state.non_blank(), Err(BlankTextError));
}

#[test]
fn search_keys_trim_and_lowercase_while_blank_queries_bypass_filtering() {
    let mut state = TextInputState::default();
    assert_eq!(state.search_key(), None);

    state.set_value("  Foo BAR  ");
    assert_eq!(state.search_key(), Some("foo bar".to_string()));

    state.set_value("thread  title");
    assert_eq!(
        state.search_key(),
        Some("thread  title".to_string()),
        "only edges trim; interior spacing stays for match parity"
    );

    state.set_value("\u{C4}nfrage");
    assert_eq!(
        state.search_key(),
        Some("\u{E4}nfrage".to_string()),
        "lowercasing is Unicode-aware like the legacy call sites"
    );

    state.set_value("\t");
    assert_eq!(state.search_key(), None);
}

#[test]
fn rust_trim_unicode_parity_differs_from_javascript_trim_on_feff_and_nel() {
    // U+200B is whitespace in neither Rust nor JavaScript trim; it is
    // removed only by normalize_input intake, not by trimming.
    assert!(
        NonBlankText::new("\u{200B}").is_ok(),
        "U+200B alone is non-blank in both trims"
    );
    let mut state = TextInputState::default();
    state.set_value("\u{200B}");
    // normalize_input strips U+200B on entry, leaving empty storage.
    assert!(state.is_empty());
    assert!(state.is_blank());
    assert_eq!(state.non_blank(), Err(BlankTextError));
    assert_eq!(state.search_key(), None);
    // Direct normalize_input check: U+200B is always stripped.
    assert_eq!(normalize_input("\u{200B}"), "");
    assert_eq!(normalize_input("a\u{200B}b"), "ab");

    // U+FEFF (BOM) is blank to JavaScript String.prototype.trim but
    // non-blank to Rust str::trim. The native seam follows Rust, aligning
    // with MessageBody/ComposerState, so it is accepted and yields a search
    // key.
    let feff = NonBlankText::new("\u{FEFF}").expect("U+FEFF is non-blank under Rust trim");
    assert_eq!(feff.as_str(), "\u{FEFF}");
    let mut feff_state = TextInputState::default();
    feff_state.set_value("\u{FEFF}");
    assert!(!feff_state.is_empty());
    assert!(
        !feff_state.is_blank(),
        "U+FEFF must be non-blank under Rust trim"
    );
    assert!(feff_state.non_blank().is_ok());
    assert_eq!(
        feff_state.search_key(),
        Some("\u{FEFF}".to_string()),
        "U+FEFF is retained by Rust trim and lowercasing is a no-op"
    );
    // Do not broaden normalize_input to strip U+FEFF.
    assert_eq!(normalize_input("\u{FEFF}"), "\u{FEFF}");

    // U+0085 (NEL) is non-blank to JavaScript trim but blank to Rust
    // str::trim. The native seam follows Rust, so it is rejected and yields
    // no search key.
    assert_eq!(NonBlankText::new("\u{0085}"), Err(BlankTextError));
    let mut nel_state = TextInputState::default();
    nel_state.set_value("\u{0085}");
    assert!(!nel_state.is_empty());
    assert!(nel_state.is_blank(), "U+0085 must be blank under Rust trim");
    assert_eq!(nel_state.non_blank(), Err(BlankTextError));
    assert_eq!(nel_state.search_key(), None);
    // Do not broaden normalize_input to strip U+0085.
    assert_eq!(normalize_input("\u{0085}"), "\u{0085}");
    // Mixed: FEFF + spaces still non-blank; NEL + spaces still blank.
    assert!(NonBlankText::new("\u{FEFF}  ").is_ok());
    assert_eq!(NonBlankText::new("  \u{0085}  "), Err(BlankTextError));
}
