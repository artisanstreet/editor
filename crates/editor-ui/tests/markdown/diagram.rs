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
            .as_chunks::<4>().0.iter()
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
        for svg in [&diagram.source, &diagram.light_source] {
            assert!(!svg.contains("<text"), "{source}");
            assert!(svg.contains("<path"), "{source}");
        }
    }
}
