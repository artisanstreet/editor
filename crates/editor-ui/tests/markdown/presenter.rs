use super::{HeatmapServices, MarkdownPresenter};
use crate::ui::markdown::MarkdownEngine;
use crate::ui::markdown::presenter::BodyServices;
use crate::ui::markdown::presenter::SelectionPlace;
use crate::ui::markdown::visual::BodyContext;
use crate::ui::markdown::visual::BodyTone;
use crate::ui::text::slice::slice;
use crate::ui::theme::{ArtisanTheme, ThemeMode};
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use std::rc::Rc;

fn rendered_rows(source: &str) -> Vec<Result<(), String>> {
    let engine = MarkdownEngine::new().expect("engine");
    let mut presenter = MarkdownPresenter::default();
    presenter.prepare(&engine, source).expect("prepared");
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let services = HeatmapServices::new(*theme);
    (0..presenter.projected_count().expect("projected"))
        .map(|row| {
            let glyph: crate::ui::rich::narrative::callout::GlyphFactory =
                Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build());
            let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
            presenter
                .render_projected_with_code(
                    row,
                    "item",
                    BodyContext {
                        theme,
                        tone: BodyTone::Foreground,
                        on_link: None,
                        on_color: None,
                        scope: None,
                    },
                    SelectionPlace {
                        document: SelectionDocumentId(1),
                        order: &[1],
                    },
                    |index| SelectionSourceId(index as u64),
                    BodyServices {
                        glyph: &glyph,
                        invalidate: &invalidate,
                        charts: Some(&services),
                        code: None,
                    },
                )
                .map(drop)
                .map_err(|error| error.to_string())
        })
        .collect()
}

#[test]
fn a_rich_fence_inside_a_list_item_renders_like_one_outside_it() {
    let fence = "```artisan-heatmap\n{\"values\":[[1,2],[3,4]]}\n```";
    let nested = format!(
        "1. First step\n\n   {}\n2. Second step",
        fence.replace('\n', "\n   ")
    );
    let rows = rendered_rows(&nested);
    assert!(rows.len() >= 3, "{rows:?}");
    assert!(rows.iter().all(Result::is_ok), "{rows:?}");
    let twice = format!("{fence}\n\n{nested}");
    let rows = rendered_rows(&twice);
    assert!(rows.iter().all(Result::is_ok), "{rows:?}");
}

fn prepared_rows(presenter: &MarkdownPresenter) -> Vec<u64> {
    let projected = presenter.projected.as_ref().expect("projected");
    (0..projected.len())
        .map(|row| projected.signature(row).expect("signature"))
        .collect()
}

fn rich_kinds(presenter: &MarkdownPresenter) -> Vec<Option<(String, bool)>> {
    presenter
        .rich
        .iter()
        .map(|entry| {
            entry
                .as_ref()
                .map(|entry| (entry.source.source.clone(), entry.source.closed))
        })
        .collect()
}

#[test]
fn a_rewritten_reply_is_prepared_from_scratch() {
    let engine = MarkdownEngine::new().expect("engine");
    let mut rewritten = MarkdownPresenter::default();
    for source in [
        "First.\n\n- one\n- two\n",
        "Other.\n\n- one\n- two\n",
        "Other.\n\n- one\n- two\n\n  three\n",
    ] {
        rewritten.prepare(&engine, source).expect("prepared");
        let mut whole = MarkdownPresenter::default();
        whole.prepare(&engine, source).expect("prepared");
        assert_eq!(
            prepared_rows(&rewritten),
            prepared_rows(&whole),
            "{source:?}"
        );
    }
}

#[test]
fn a_reply_prepared_delta_by_delta_matches_one_prepared_whole() {
    let reply = "# Report\n\nThe heatmap below is **hot**.\n\n```artisan-heatmap\n{\"values\":[[1,2],[3,4]]}\n```\n\n- one\n- two with `code`\n\n```rust\nfn main() {\n\n    paint();\n}\n```\n\nBetween fences.\n\n```artisan-callout\n{\"kind\":\"warning\",\"title\":\"Stale\",\"body\":\"Regenerate it.\"}\n```\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\nDone.\n";
    let engine = MarkdownEngine::new().expect("engine");
    let mut streamed = MarkdownPresenter::default();
    let mut identities = Vec::new();
    for end in (1..=reply.len()).filter(|end| reply.is_char_boundary(*end)) {
        let source = slice(reply, ..end);
        streamed.prepare(&engine, source).expect("streamed prepare");
        let mut whole = MarkdownPresenter::default();
        whole.prepare(&engine, source).expect("whole prepare");
        assert_eq!(
            prepared_rows(&streamed),
            prepared_rows(&whole),
            "rows after {end} bytes"
        );
        assert_eq!(
            rich_kinds(&streamed),
            rich_kinds(&whole),
            "rich fences after {end} bytes"
        );
        let current: Vec<u64> = streamed
            .rich
            .iter()
            .flatten()
            .map(|entry| entry.id)
            .collect();
        if end > reply.find("Between").expect("marker") {
            assert_eq!(
                current.first(),
                identities.first(),
                "a closed rich fence keeps its identity while the reply streams"
            );
        }
        identities = current;
    }
    assert_eq!(identities.len(), 2);
    assert_eq!(
        rendered_rows(reply).len(),
        streamed.projected_count().expect("projected")
    );
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let rich_only = "Intro.\n\n```artisan-heatmap\n{\"values\":[[1,2],[3,4]]}\n```\n\n- a\n\n```artisan-callout\n{\"kind\":\"warning\",\"title\":\"Stale\",\"body\":\"Regenerate it.\"}\n```\n\nDone.\n";
    let mut streamed = MarkdownPresenter::default();
    for end in (1..=rich_only.len()).step_by(7).chain([rich_only.len()]) {
        streamed
            .prepare(&engine, slice(rich_only, ..end))
            .expect("streamed prepare");
    }
    streamed.prepare_document(&engine).expect("whole document");
    let glyph: crate::ui::rich::narrative::callout::GlyphFactory =
        Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build());
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    streamed
        .render(
            "item",
            BodyContext {
                theme,
                tone: BodyTone::Foreground,
                on_link: None,
                on_color: None,
                scope: None,
            },
            SelectionPlace {
                document: SelectionDocumentId(1),
                order: &[1],
            },
            |index| SelectionSourceId(index as u64),
            BodyServices {
                glyph: &glyph,
                invalidate: &invalidate,
                charts: Some(&HeatmapServices::new(*theme)),
                code: None,
            },
        )
        .unwrap();
}
