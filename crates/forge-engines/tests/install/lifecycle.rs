use super::{Event, State, machine};

#[test]
fn a_failed_replacement_keeps_the_serving_version_and_retry_replaces_it() {
    let mut machine = machine();
    machine.handle(&Event::Loaded {
        version: Some("1".into()),
    });
    machine.handle(&Event::Start {
        version: "2".into(),
    });
    machine.handle(&Event::Loaded { version: None });
    machine.handle(&Event::Start {
        version: "3".into(),
    });
    machine.handle(&Event::Progress {
        received: 100,
        total: Some(200),
    });
    assert_eq!(
        machine.state(),
        &State::Downloading {
            version: "2".into(),
            received: 100,
            total: Some(200),
            serving: Some("1".into()),
        }
    );
    machine.handle(&Event::Downloaded);
    machine.handle(&Event::Fail {
        message: "checksum mismatch".into(),
    });
    assert_eq!(
        machine.state(),
        &State::Failed {
            message: "checksum mismatch".into(),
            serving: Some("1".into()),
        }
    );
    machine.handle(&Event::Start {
        version: "2".into(),
    });
    machine.handle(&Event::Done {
        version: "2".into(),
    });
    assert_eq!(
        machine.state(),
        &State::Installed {
            version: "2".into()
        }
    );
}

