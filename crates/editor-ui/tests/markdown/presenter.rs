use super::{HeatmapServices, MarkdownPresenter};
use crate::ui::markdown::MarkdownEngine;
use crate::ui::markdown::visual::BodyTone;
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
            presenter
                .render_projected_with_services(
                    row,
                    "item",
                    theme,
                    BodyTone::Foreground,
                    SelectionDocumentId(1),
                    &[1],
                    |index| SelectionSourceId(index as u64),
                    Rc::new(|_, _, _| aqua_ui::element::ui::Box::new().build()),
                    Rc::new(|| {}),
                    None,
                    None,
                    None,
                    Some(&services),
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
