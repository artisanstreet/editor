//! Focused parity coverage for shell-command presentation.
//!
//! The cases mirror the small prefix recognizer in the legacy TypeScript
//! implementation. They intentionally cover paths, wrapper aliases, exact
//! flag families, whitespace, quoting, malformed input, and conservative
//! fallback behavior without treating the body as shell syntax.

use artisan_frontend::shell_command::present_shell_command;

fn assert_cases(cases: &[(&str, &str)]) {
    for (input, expected) in cases {
        assert_eq!(
            present_shell_command(input),
            *expected,
            "presentation mismatch for {input:?}"
        );
    }
}

#[test]
fn empty_plain_and_unknown_invocations_keep_the_collapsed_text() {
    assert_cases(&[
        ("", ""),
        (" \t\r\n\u{feff} ", ""),
        ("  echo\t  hello\nworld  ", "echo hello world"),
        ("nu\t-c\t'echo   hello'", "nu -c 'echo hello'"),
        ("git\u{0085}status", "git\u{0085}status"),
        ("git\u{180e}status", "git\u{180e}status"),
    ]);
}

#[test]
fn javascript_whitespace_collapses_before_wrapper_and_body_parsing() {
    assert_eq!(
        present_shell_command(
            "\u{feff}\t\"C:\\Program   Files\\PowerShell\\pwsh.EXE\"\u{00a0}-c\u{000c}\" echo\t  hi \""
        ),
        " echo hi ",
        "FEFF and JavaScript whitespace collapse, while spaces inside body quotes remain"
    );

    // U+0085 is not JavaScript `\\s`; it remains part of the body.
    assert_eq!(
        present_shell_command("pwsh -c echo\u{0085}value"),
        "echo\u{0085}value"
    );
}

#[test]
fn powershell_wrappers_accept_quoted_windows_and_unix_paths() {
    assert_cases(&[
        (
            r#""C:\Program Files\PowerShell\7\pwsh.exe" -Command "Write-Output hello""#,
            "Write-Output hello",
        ),
        (
            r#"'C:\Program Files\PowerShell\7\PoWeRsHeLl.ExE' -C 'Get-Process'"#,
            "Get-Process",
        ),
        (r#"/opt/bin/POWERSHELL.EXE -c echo ready"#, "echo ready"),
        ("pwsh -command Write-Output\tready", "Write-Output ready"),
    ]);
}

#[test]
fn posix_wrappers_cover_each_exact_flag_and_path_separator() {
    assert_cases(&[
        ("/usr/bin/bash -c echo bash", "echo bash"),
        (
            r#"/usr/local/bin/bash -lc "printf 'hello world'""#,
            "printf 'hello world'",
        ),
        (r#"C:\Tools\SH.ExE -ic 'printf "sh"'"#, "printf \"sh\""),
        (r#"/bin/ZsH.EXE -lic 'echo zsh'"#, "echo zsh"),
        (r#"C:/Tools/dAsH -LIC 'printf "dash"'"#, "printf \"dash\""),
    ]);
}

#[test]
fn cmd_wrapper_accepts_c_and_k_with_case_insensitive_exe_suffix() {
    assert_cases(&[
        (
            r#""C:\Windows\System32\CMD.EXE" /c "dir C:\Temp""#,
            "dir C:\\Temp",
        ),
        (
            r#"C:/Windows/System32/cMd.eXe /K 'echo ready'"#,
            "echo ready",
        ),
    ]);
}

#[test]
fn wrapper_flag_sets_are_exact_and_case_insensitive() {
    // Accepted flags are exercised above; each nearby but unsupported family
    // must retain the entire collapsed invocation.
    assert_cases(&[
        ("pwsh -lc echo", "pwsh -lc echo"),
        ("powershell -ic echo", "powershell -ic echo"),
        ("bash -command echo", "bash -command echo"),
        ("sh /c echo", "sh /c echo"),
        ("zsh /k echo", "zsh /k echo"),
        ("dash -Command echo", "dash -Command echo"),
        ("cmd -c echo", "cmd -c echo"),
        ("cmd -lc echo", "cmd -lc echo"),
        ("PWSh -CoMmAnD echo", "echo"),
        ("BaSh -Lc echo", "echo"),
        ("CmD /K echo", "echo"),
    ]);
}

#[test]
fn leading_argument_quotes_are_required_for_paths_with_spaces() {
    assert_cases(&[
        (r#""C:\Program Files\PowerShell\pwsh.exe" -c echo"#, "echo"),
        (
            r#"'C:\Program Files\PowerShell\pwsh.exe' -c 'echo'"#,
            "echo",
        ),
        (
            r#"C:\Program Files\PowerShell\pwsh.exe -c echo"#,
            r#"C:\Program Files\PowerShell\pwsh.exe -c echo"#,
        ),
        (
            r#""C:\Program Files\PowerShell\pwsh.exe -c echo"#,
            r#""C:\Program Files\PowerShell\pwsh.exe -c echo"#,
        ),
    ]);
}

#[test]
fn basename_matching_uses_both_separators_and_only_a_final_exe_suffix() {
    assert_cases(&[
        (r#"C:/one\two/PoWeRsHeLl.ExE -c echo"#, "echo"),
        (r#"C:\one/two\BASH.eXe -lc echo"#, "echo"),
        (
            r#"/usr/bin/pwsh.exe.bak -c echo"#,
            r#"/usr/bin/pwsh.exe.bak -c echo"#,
        ),
        (r#"/usr/bin/pwsh.ex -c echo"#, r#"/usr/bin/pwsh.ex -c echo"#),
    ]);
}

#[test]
fn one_matching_body_quote_pair_is_removed_without_shell_parsing() {
    assert_cases(&[
        (r#"bash -c 'echo "quoted text"'"#, "echo \"quoted text\""),
        (r#"bash -c '"echo"'"#, "\"echo\""),
        (r#"bash -c '""echo""'"#, "\"\"echo\"\""),
        (
            r#"bash -c "printf \"hello world\"""#,
            r#"printf \"hello world\""#,
        ),
        (r#"bash -c 'echo one; echo two'"#, "echo one; echo two"),
    ]);
}

#[test]
fn malformed_body_quotes_are_preserved_after_a_recognized_prefix() {
    assert_cases(&[
        (r#"bash -c "echo"#, "\"echo"),
        (r#"bash -c 'echo"#, "'echo"),
        (r#"bash -c 'echo' trailing"#, "'echo' trailing"),
    ]);
}

#[test]
fn empty_body_falls_back_to_the_whole_collapsed_invocation() {
    assert_cases(&[
        ("pwsh -c", "pwsh -c"),
        ("pwsh -c   \t\n", "pwsh -c"),
        ("pwsh -c \"\"", "pwsh -c \"\""),
        ("bash -lc ''", "bash -lc ''"),
        ("cmd /c \"\"", "cmd /c \"\""),
        ("dash -lic", "dash -lic"),
    ]);

    // Whitespace inside a matching quote is content, not an empty body. The
    // outer invocation whitespace has already been collapsed to one space.
    assert_eq!(present_shell_command("bash -c \"   \""), " ");
}

#[test]
fn unrecognized_wrappers_and_unrecognized_leading_arguments_fall_back() {
    assert_cases(&[
        ("nu -c echo", "nu -c echo"),
        ("fish -c echo", "fish -c echo"),
        ("runner --shell pwsh -c echo", "runner --shell pwsh -c echo"),
        (
            r#""C:\Program Files\unknown.exe" -c echo"#,
            r#""C:\Program Files\unknown.exe" -c echo"#,
        ),
        ("pwshx -c echo", "pwshx -c echo"),
    ]);
}
