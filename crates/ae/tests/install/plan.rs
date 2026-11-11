use super::*;

#[test]
fn every_place_survives_being_handed_to_another_installer() {
    for place in [
        Place::Here,
        Place::Windows,
        Place::Wsl("Ubuntu-24.04".to_owned()),
        Place::Keep,
        Place::Skip,
    ] {
        assert_eq!(Place::parse(&place.argument()), Ok(place));
    }
    assert!(Place::parse("wsl:").is_err(), "a distro needs a name");
}
