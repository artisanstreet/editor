use super::{WslCommandError, running_or_none, wsl_text};

fn utf16le(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

#[test]
fn a_failed_running_list_means_no_distribution_is_running() {
    let failed = Err(WslCommandError::Failed {
        code: Some(-1),
        stderr: "There are no running distributions.".into(),
    });

    assert_eq!(running_or_none(failed).unwrap(), "");
}

#[test]
fn a_running_list_that_could_not_run_is_still_an_error() {
    let timed_out = Err(WslCommandError::TimedOut { seconds: 10 });

    assert!(matches!(
        running_or_none(timed_out),
        Err(WslCommandError::TimedOut { seconds: 10 })
    ));
}

#[test]
fn wsl_messages_read_the_same_in_utf16_and_utf8() {
    let message = "Le système ne trouve pas la distribution «Arch».\r\n";
    let mut marked = vec![0xff, 0xfe];
    marked.extend(utf16le(message));

    assert_eq!(wsl_text(&utf16le(message)), message);
    assert_eq!(wsl_text(&marked), message);
    assert_eq!(
        wsl_text(message.as_bytes()),
        message,
        "WSL_UTF8=1 makes wsl.exe write UTF-8"
    );
}
