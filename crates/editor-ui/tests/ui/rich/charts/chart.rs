use super::prepare;

fn prepared(json: &str) -> Result<usize, String> {
    let value = serde_json::from_str(json).expect("the fixture is JSON");
    prepare(&value).map(|(_, model)| model.series.len())
}

#[test]
fn a_series_with_values_and_points_is_refused_instead_of_dropping_one() {
    let error =
        prepared(r#"{"series":[{"name":"latency","values":[1,2],"points":[["a",3],["b",4]]}]}"#)
            .expect_err("two datasets for one series are ambiguous");
    assert!(error.contains("series `latency` gives both `values` and `points`"));
}

#[test]
fn a_series_without_data_is_refused() {
    let error = prepared(r#"{"series":[{"name":"empty"}]}"#)
        .expect_err("a series with no data draws nothing");
    assert!(error.contains("series `empty` gives neither"));
}

#[test]
fn values_and_points_each_draw_their_series() {
    assert_eq!(
        prepared(r#"{"series":[{"values":[1,2]},{"points":[["c",3]]}]}"#),
        Ok(2)
    );
}

#[test]
fn the_keyboard_explorable_plot_is_a_named_focusable_group_for_assistive_tech() {
    use super::{ChartState, Services, view};
    use crate::ui::theme::ArtisanTheme;
    use aqua_core::Role;
    use aqua_text::{SharedFonts, SharedRuns};
    use aqua_ui::element::ui;
    use std::rc::Rc;
    use std::sync::Arc;

    let value = serde_json::from_str(r#"{"series":[{"name":"latency","values":[1,2,3]}]}"#)
        .expect("the fixture is JSON");
    let (spec, model) = prepare(&value).expect("the fixture is a chart");
    let blank = || ui::Box::new().build();
    let services = Services {
        press: Rc::new(|plot, _| plot),
        frame: Rc::new(|plot, _, _, _, _, _| plot),
        label: Rc::new(move |_| blank()),
        readout: Rc::new(move |_, _, _| blank()),
        legend: Rc::new(move |_, _| blank()),
        reset: Rc::new(move |_| blank()),
        empty: Rc::new(move |_| blank()),
        navigation: Rc::new(|plot, _| plot),
    };
    let chart = view(
        "chart".to_owned(),
        &ArtisanTheme::resolved(),
        Arc::new(spec),
        Arc::new(model),
        ChartState::default(),
        &services,
        Rc::new(|| {}),
    );
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| chart).unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    let group = app
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.role == Role::Group && node.label.starts_with("Chart. Arrows explore"))
        .expect("the plot is announced as a chart group");
    assert!(
        group.focusable,
        "assistive tech can focus the plot the keyboard reaches with Tab"
    );
}
