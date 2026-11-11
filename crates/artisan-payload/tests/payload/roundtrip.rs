use super::*;

#[test]
fn a_packed_program_is_written_back_byte_for_byte_and_runnable() {
    let program: Vec<u8> = (0..200_000u32).flat_map(u32::to_le_bytes).collect();
    let packed = pack(&program).expect("pack");
    assert!(packed.len() < program.len(), "the payload is compressed");

    let payload = Payload::new(&packed);
    assert!(payload.present());
    let folder = tempfile::tempdir().expect("temp folder");
    let path = folder.path().join("program");
    payload.write(&path).expect("write");
    assert_eq!(std::fs::read(&path).expect("read back"), program);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "the program can be run");
    }
    assert!(
        !Payload::new(&[]).present(),
        "a build without one carries nothing"
    );
}
