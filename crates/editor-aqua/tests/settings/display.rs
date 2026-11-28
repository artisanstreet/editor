use editor_aqua::settings::display::DisplayRate;
use std::time::Duration;

#[test]
fn refresh_intervals_name_the_rate_the_monitor_runs_at() {
    let rates = [
        Some(Duration::from_nanos(4_166_667)),
        Some(Duration::from_nanos(6_060_606)),
        Some(Duration::from_nanos(16_683_350)),
        Some(Duration::ZERO),
        None,
    ]
    .map(DisplayRate::of);
    assert_eq!(
        rates.map(DisplayRate::label),
        ["240 Hz", "165 Hz", "60 Hz", "Native", "Native"]
    );
}
