use forge_store::{DataLock, LockDataError};

#[test]
fn taking_the_data_back_waits_for_a_stopping_forge_to_let_go_of_it() {
    let dir = tempfile::tempdir().expect("create the data folder");
    let stopping = DataLock::acquire(dir.path()).expect("the stopping Forge holds its data");
    assert!(matches!(
        DataLock::acquire(dir.path()),
        Err(LockDataError::Held { .. })
    ));
    let (told, waiting) = std::sync::mpsc::channel();
    let folder = dir.path().to_owned();
    let taking = std::thread::spawn(move || {
        DataLock::acquire_waiting(&folder, move || {
            told.send(()).expect("the test waits to be told");
        })
    });

    waiting
        .recv_timeout(test_support::wait::LIVENESS)
        .expect("the installer says it is waiting for the data");
    drop(stopping);

    taking
        .join()
        .expect("taking the data doesn't panic")
        .expect("the data is taken once the stopping Forge lets go");
}
