use super::*;

#[test]
fn cubic_band_and_node_hit_testing_follow_the_real_layout() {
    let value = serde_json::json!({"links":[{"from":"a","to":"b","value":10},{"from":"b","to":"c","value":5}]});
    let graph = Arc::new(super::super::prepare(&value).expect("a valid flow chart"));
    let laid = SankeyState::default().sync("chart", &graph).2;
    let width = 640.0;
    for index in 0..3 {
        let place = laid.nodes[index];
        let x = node_x(&laid, place.rank, width) + 5.0;
        let y = f32::midpoint(place.y0, place.y1);
        assert_eq!(hit(&laid, width, x, y), Some(Hover::Node(index)));
    }
    for (index, link) in graph.links.iter().enumerate() {
        let (x0, x1) = band_span(&laid, (link.from, link.to), width);
        let band = laid.bands[index];
        let y = f32::midpoint(
            f32::midpoint(band.source.0, band.source.1),
            f32::midpoint(band.target.0, band.target.1),
        );
        assert_eq!(
            hit(&laid, width, f32::midpoint(x0, x1), y),
            Some(Hover::Link(index))
        );
    }
    assert_eq!(hit(&laid, width, -100.0, -100.0), None);
}
