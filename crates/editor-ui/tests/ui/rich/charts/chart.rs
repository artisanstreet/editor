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
