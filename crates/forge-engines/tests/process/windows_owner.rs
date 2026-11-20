use super::*;
#[tokio::test]
async fn native_job_drop_closes_pipes_held_by_its_own_descendants() {
    let mut command = Command::new("cmd.exe");
    command
        .raw_arg("/d /s /c \"echo ready& ping -n 30 127.0.0.1 >NUL\"")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = owned_command(command, forge_isolation::Containment::Shared).unwrap();
    let mut pipe = BufReader::new(child.stdout().take().unwrap());
    let mut ready = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        pipe.read_line(&mut ready),
    )
    .await
    .expect("owned native parent must start its descendant")
    .unwrap();
    assert_eq!(ready.trim(), "ready");
    drop(child);
    let mut rest = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        pipe.read_to_end(&mut rest),
    )
    .await
    .expect("native JobObject drop must close descendant stdout")
    .unwrap();
}
