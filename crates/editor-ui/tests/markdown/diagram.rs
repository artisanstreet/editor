use super::*;
use aqua_core::vector::{SvgSource, VectorSource};
use aqua_media::vector::{VectorBudget, rasterize};

#[test]
fn outlined_labels_render_with_bundled_fonts_and_keep_span_colors() {
    let source = outline_text(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="50"><text x="4" y="30" font-family="Spline Sans" font-size="20" fill="white">Start<tspan x="100" fill="red">End</tspan></text></svg>"#,
    )
    .unwrap();
    assert!(!source.contains("<text"));
    let raster = rasterize(
        &VectorSource::Svg(Arc::new(SvgSource {
            source: source.into(),
            tint: None,
        })),
        200,
        50,
        VectorBudget::default(),
    )
    .unwrap();
    let count = |start: usize, end: usize, red: bool| {
        raster
            .pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(index, pixel)| {
                (start..end).contains(&(index % 200))
                    && pixel[3] > 128
                    && pixel[0] > 128
                    && if red { pixel[1] == 0 } else { pixel[1] > 128 }
            })
            .count()
    };
    assert!(count(0, 100, false) > 100);
    assert!(count(100, 200, true) > 100);
}

#[test]
fn mermaid_diagrams_outline_labels_in_both_themes() {
    for source in [
        "flowchart LR\nA[Start] -->|next| B[End]",
        "sequenceDiagram\nAlice->>Bob: Hello",
        "classDiagram\nAnimal <|-- Duck",
    ] {
        let diagram = render(source).unwrap();
        for svg in diagram.inks.all() {
            assert!(!svg.contains("<text"), "{source}");
            assert!(svg.contains("<path"), "{source}");
        }
    }
}

#[cfg(windows)]
fn laid_out_glyphs(group: &usvg::Group, glyphs: &mut Vec<(String, u32)>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(group) => laid_out_glyphs(group, glyphs),
            usvg::Node::Text(text) => glyphs.extend(
                text.layouted()
                    .iter()
                    .flat_map(|span| &span.positioned_glyphs)
                    .map(|glyph| (glyph.text.clone(), glyph.id.0)),
            ),
            usvg::Node::Path(_) | usvg::Node::Image(_) => {}
        }
    }
}

#[cfg(windows)]
#[test]
fn non_latin_mermaid_labels_draw_real_glyphs_from_system_fallbacks() {
    let labels = ["日本語", "한국어", "العربية", "עברית", "Кириллица", "हिन्दी"];
    let source = format!(
        "flowchart LR\nA[{}] --> B[{}]\nB --> C[{}]\nC --> D[{}]\nD --> E[{}]\nE --> F[{}]",
        labels[0], labels[1], labels[2], labels[3], labels[4], labels[5]
    );
    let theme = diagram_theme(ArtisanTheme::for_mode(ThemeMode::Dark));
    let config = LayoutConfig {
        fast_text_metrics: true,
        ..LayoutConfig::default()
    };
    let parsed = parse_mermaid_strict(&source).unwrap();
    let svg = render_svg(
        &compute_layout(&parsed.graph, &theme, &config),
        &theme,
        &config,
    );
    let tree = usvg::Tree::from_str(&svg, &diagram_options()).unwrap();
    let mut glyphs = Vec::new();
    laid_out_glyphs(tree.root(), &mut glyphs);
    let drawn: String = glyphs.iter().map(|(text, _)| text.as_str()).collect();
    for label in labels {
        assert!(
            label.chars().all(|character| drawn.contains(character)),
            "{label} was not laid out: {drawn}"
        );
    }
    let missing: Vec<&str> = glyphs
        .iter()
        .filter(|(text, id)| *id == 0 && !text.trim().is_empty())
        .map(|(text, _)| text.as_str())
        .collect();
    assert!(missing.is_empty(), "drawn as .notdef: {missing:?}");
}

const LEAK_DATA: &str = "EDITOR_UI_DIAGRAM_LEAK_DATA";
const SENTINEL: &str = "PRIVATESENTINEL";

fn leaking_sources() -> [String; 3] {
    [
        format!("sequenceDiagram\nparticipant A\n{SENTINEL}->>A: hello"),
        format!("flowchart LR\nA --> B\nsubgraph {SENTINEL}\nB --> C"),
        format!("%%{{init: {{\"theme\": \"{SENTINEL}\" }}%%\nflowchart LR\nA --> B"),
    ]
}

#[expect(
    clippy::panic,
    reason = "the child crashes on purpose to write its recent log"
)]
fn render_and_crash(data: &std::path::Path) -> ! {
    let log = std::fs::File::create(data.join("editor.log")).unwrap();
    let recent = diagnostics::subscribe("debug", std::sync::Mutex::new(log)).unwrap();
    diagnostics::CrashReporter {
        app: artisan_paths::App::Editor,
        version: "9.9.9 (test)",
        data: data.to_owned(),
        recent,
    }
    .install();
    for source in leaking_sources() {
        assert!(render(&source).is_none(), "{source}");
    }
    panic!("invariant: the test ends here")
}

#[test]
fn a_diagram_that_fails_logs_none_of_its_source() {
    if let Some(data) = std::env::var_os(LEAK_DATA) {
        render_and_crash(std::path::Path::new(&data));
    }
    let test = format!(
        "{}::a_diagram_that_fails_logs_none_of_its_source",
        module_path!().split_once("::").unwrap().1
    );
    let directory = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([test.as_str(), "--exact", "--nocapture", "--test-threads=1"])
        .env(LEAK_DATA, directory.path())
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    assert!(!output.status.success(), "the child must crash");
    let log = std::fs::read_to_string(directory.path().join("editor.log")).unwrap();
    assert_eq!(
        log.matches("the diagram shows as its source").count(),
        leaking_sources().len(),
        "each failure is logged: {log}"
    );
    assert!(
        !log.contains(SENTINEL),
        "the log holds diagram source: {log}"
    );
    let reports: Vec<_> = std::fs::read_dir(diagnostics::CrashReporter::folder(directory.path()))
        .unwrap()
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(reports.len(), 1);
    assert!(
        reports[0].contains("the diagram shows as its source"),
        "the crash report carries the recent log: {}",
        reports[0]
    );
    assert!(
        !reports[0].contains(SENTINEL),
        "the crash report holds diagram source: {}",
        reports[0]
    );
}
