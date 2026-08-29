//! Exhaustive focused coverage for the dependency-free editor marker mapper.

#[path = "../../modules/frontend/src/editor_diagnostic_mapping.rs"]
mod editor_diagnostic_mapping;

use editor_diagnostic_mapping::{
    EditorDiagnostic, EditorDiagnosticSeverity, MappedMarker, map_editor_diagnostics,
};

fn diagnostic(
    message: &str,
    severity: EditorDiagnosticSeverity,
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
) -> EditorDiagnostic {
    EditorDiagnostic::new(
        message,
        severity,
        start_line,
        start_column,
        end_line,
        end_column,
    )
}

fn marker(
    from: usize,
    to: usize,
    message: &str,
    severity: EditorDiagnosticSeverity,
    source: Option<&str>,
) -> MappedMarker {
    MappedMarker {
        from,
        to,
        message: message.to_owned(),
        severity,
        source: source.map(str::to_owned),
    }
}

#[test]
fn all_severities_and_optional_fields_map_exactly() {
    let diagnostics = [
        diagnostic("error", EditorDiagnosticSeverity::Error, 1, 1, 1, 2),
        diagnostic("warning", EditorDiagnosticSeverity::Warning, 1, 2, 1, 3).with_code("W001"),
        diagnostic("info", EditorDiagnosticSeverity::Info, 1, 3, 1, 4),
        diagnostic("hint", EditorDiagnosticSeverity::Hint, 1, 4, 1, 5).with_code(""),
    ];

    assert_eq!(
        EditorDiagnosticSeverity::ALL.map(EditorDiagnosticSeverity::as_str),
        ["error", "warning", "info", "hint"]
    );
    assert_eq!(
        map_editor_diagnostics("abcd", &diagnostics),
        vec![
            marker(0, 1, "error", EditorDiagnosticSeverity::Error, None),
            marker(
                1,
                2,
                "warning",
                EditorDiagnosticSeverity::Warning,
                Some("W001")
            ),
            marker(2, 3, "info", EditorDiagnosticSeverity::Info, None),
            marker(3, 4, "hint", EditorDiagnosticSeverity::Hint, Some("")),
        ]
    );
}

#[test]
fn ascii_columns_are_one_based_and_end_at_content_not_newline() {
    let diagnostics = [
        diagnostic("first", EditorDiagnosticSeverity::Error, 1, 1, 1, 2),
        diagnostic("middle", EditorDiagnosticSeverity::Warning, 1, 2, 1, 4),
        diagnostic("last", EditorDiagnosticSeverity::Info, 1, 4, 1, 5),
    ];

    assert_eq!(
        map_editor_diagnostics("abcd\nnext", &diagnostics),
        vec![
            marker(0, 1, "first", EditorDiagnosticSeverity::Error, None),
            marker(1, 3, "middle", EditorDiagnosticSeverity::Warning, None),
            marker(3, 4, "last", EditorDiagnosticSeverity::Info, None),
        ]
    );
}

#[test]
fn zero_and_oversized_lines_and_columns_clamp_deterministically() {
    let diagnostics = [
        diagnostic("zero", EditorDiagnosticSeverity::Error, 0, 0, 0, 0),
        diagnostic(
            "line high",
            EditorDiagnosticSeverity::Warning,
            99,
            1,
            99,
            99,
        ),
        diagnostic("column high", EditorDiagnosticSeverity::Info, 2, 99, 2, 100),
        diagnostic(
            "both high",
            EditorDiagnosticSeverity::Hint,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            u32::MAX,
        ),
    ];

    assert_eq!(
        map_editor_diagnostics("abc\nxy", &diagnostics),
        vec![
            marker(0, 0, "zero", EditorDiagnosticSeverity::Error, None),
            marker(4, 6, "line high", EditorDiagnosticSeverity::Warning, None),
            marker(6, 6, "column high", EditorDiagnosticSeverity::Info, None),
            marker(6, 6, "both high", EditorDiagnosticSeverity::Hint, None),
        ]
    );
}

#[test]
fn empty_document_has_one_empty_line_for_every_position() {
    let diagnostics = [
        diagnostic("empty", EditorDiagnosticSeverity::Error, 0, 0, 100, 100),
        diagnostic("unicode", EditorDiagnosticSeverity::Info, 1, 2, 1, 3),
    ];

    assert_eq!(
        map_editor_diagnostics("", &diagnostics),
        vec![
            marker(0, 0, "empty", EditorDiagnosticSeverity::Error, None),
            marker(0, 0, "unicode", EditorDiagnosticSeverity::Info, None),
        ]
    );
    assert!(map_editor_diagnostics("", &[]).is_empty());
}

#[test]
fn trailing_newline_keeps_a_clampable_empty_final_line() {
    let diagnostics = [
        diagnostic("line one", EditorDiagnosticSeverity::Error, 1, 99, 1, 99),
        diagnostic("line two", EditorDiagnosticSeverity::Warning, 2, 1, 2, 99),
        diagnostic("line high", EditorDiagnosticSeverity::Hint, 3, 1, 3, 1),
    ];

    assert_eq!(
        map_editor_diagnostics("abc\n", &diagnostics),
        vec![
            marker(3, 3, "line one", EditorDiagnosticSeverity::Error, None),
            marker(4, 4, "line two", EditorDiagnosticSeverity::Warning, None),
            marker(4, 4, "line high", EditorDiagnosticSeverity::Hint, None),
        ]
    );
}

#[test]
fn crlf_bytes_are_not_line_content_or_marker_targets() {
    let document = "ab\r\ncd\r\nef";
    let diagnostics = [
        diagnostic("first end", EditorDiagnosticSeverity::Error, 1, 99, 1, 99),
        diagnostic(
            "second start",
            EditorDiagnosticSeverity::Warning,
            2,
            1,
            2,
            2,
        ),
        diagnostic("second end", EditorDiagnosticSeverity::Info, 2, 99, 2, 99),
        diagnostic("third", EditorDiagnosticSeverity::Hint, 3, 1, 3, 3),
    ];

    assert_eq!(
        map_editor_diagnostics(document, &diagnostics),
        vec![
            marker(2, 2, "first end", EditorDiagnosticSeverity::Error, None),
            marker(
                4,
                5,
                "second start",
                EditorDiagnosticSeverity::Warning,
                None
            ),
            marker(6, 6, "second end", EditorDiagnosticSeverity::Info, None),
            marker(8, 10, "third", EditorDiagnosticSeverity::Hint, None),
        ]
    );
}

#[test]
fn unicode_columns_count_scalars_and_never_split_utf8() {
    let document = "Aé😀Z";
    let diagnostics = [
        diagnostic("accent", EditorDiagnosticSeverity::Error, 1, 2, 1, 3),
        diagnostic("astral", EditorDiagnosticSeverity::Warning, 1, 3, 1, 4),
        diagnostic("after", EditorDiagnosticSeverity::Info, 1, 4, 1, 5),
        diagnostic("clamped", EditorDiagnosticSeverity::Hint, 1, 99, 1, 99),
    ];

    let mapped = map_editor_diagnostics(document, &diagnostics);
    assert_eq!(
        mapped,
        vec![
            marker(1, 3, "accent", EditorDiagnosticSeverity::Error, None),
            marker(3, 7, "astral", EditorDiagnosticSeverity::Warning, None),
            marker(7, 8, "after", EditorDiagnosticSeverity::Info, None),
            marker(8, 8, "clamped", EditorDiagnosticSeverity::Hint, None),
        ]
    );
    for marker in mapped {
        assert!(document.get(marker.from..marker.from).is_some());
        assert!(document.get(marker.to..marker.to).is_some());
        assert!(document.get(marker.from..marker.to).is_some());
    }
}

#[test]
fn multiline_ranges_include_newlines_and_clamp_each_endpoint_independently() {
    let document = "one\ntwo\nthree";
    let diagnostics = [
        diagnostic("across", EditorDiagnosticSeverity::Error, 1, 2, 3, 3),
        diagnostic("start high", EditorDiagnosticSeverity::Warning, 1, 99, 2, 1),
        diagnostic("end high", EditorDiagnosticSeverity::Info, 2, 2, 99, 99),
    ];

    assert_eq!(
        map_editor_diagnostics(document, &diagnostics),
        vec![
            marker(1, 10, "across", EditorDiagnosticSeverity::Error, None),
            marker(3, 4, "start high", EditorDiagnosticSeverity::Warning, None),
            marker(5, 13, "end high", EditorDiagnosticSeverity::Info, None),
        ]
    );
}

#[test]
fn reversed_ranges_collapse_to_the_mapped_start_without_swapping_endpoints() {
    let diagnostics = [
        diagnostic("same line", EditorDiagnosticSeverity::Error, 1, 4, 1, 2),
        diagnostic(
            "backward lines",
            EditorDiagnosticSeverity::Warning,
            3,
            1,
            1,
            1,
        ),
        diagnostic("zero width", EditorDiagnosticSeverity::Info, 2, 3, 2, 3),
    ];

    assert_eq!(
        map_editor_diagnostics("abcd\nef\nghi", &diagnostics),
        vec![
            marker(3, 3, "same line", EditorDiagnosticSeverity::Error, None),
            marker(
                8,
                8,
                "backward lines",
                EditorDiagnosticSeverity::Warning,
                None
            ),
            marker(7, 7, "zero width", EditorDiagnosticSeverity::Info, None),
        ]
    );
}

#[test]
fn mapping_clones_fields_and_does_not_mutate_input_diagnostics() {
    let diagnostics = vec![
        diagnostic(
            "message — 🚀",
            EditorDiagnosticSeverity::Warning,
            1,
            2,
            1,
            4,
        )
        .with_code("源-1"),
    ];
    let before = diagnostics.clone();

    let mapped = map_editor_diagnostics("x🚀yz", &diagnostics);

    assert_eq!(diagnostics, before);
    assert_eq!(mapped[0].message, diagnostics[0].message);
    assert_eq!(mapped[0].source.as_deref(), diagnostics[0].code.as_deref());
    assert_eq!(mapped[0].severity, diagnostics[0].severity);
    assert_eq!(mapped[0].from, 1);
    assert_eq!(mapped[0].to, 6);
}
