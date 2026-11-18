use std::path::Path;

use serde_json::{Value, json};

use super::super::{Grammar, Rules, Verdict};
use crate::claudeai::tools::Call;

fn rules(project: &Path, grammar: Grammar, allow: &Value, deny: &Value) -> Rules {
    let settings = project.join(".claude");
    std::fs::create_dir_all(&settings).expect("the settings folder");
    std::fs::write(
        settings.join("settings.json"),
        json!({ "permissions": { "allow": allow, "deny": deny } }).to_string(),
    )
    .expect("the settings");
    Rules::load_for(&project.join("home"), project, grammar)
}

fn verdict(rules: &Rules, command: &str) -> Option<Verdict> {
    rules.verdict(&Call {
        id: "b1".to_owned(),
        name: "Bash".to_owned(),
        input: json!({ "command": command }),
    })
}

#[test]
fn apostrophes_do_not_hide_commands_from_an_allow_rule_on_windows() {
    let project = tempfile::tempdir().expect("a dir");
    let command = "echo 'ok & whoami & echo '";
    let posix = rules(
        project.path(),
        Grammar::Posix,
        &json!(["Bash(echo:*)"]),
        &json!([]),
    );
    assert_eq!(verdict(&posix, command), Some(Verdict::Allow));
    let cmd = rules(
        project.path(),
        Grammar::Cmd,
        &json!(["Bash(echo:*)"]),
        &json!([]),
    );
    assert_eq!(verdict(&cmd, command), None, "cmd runs whoami too");
}

#[test]
fn an_allow_vouches_for_a_cmd_line_only_when_it_is_provably_allowed_commands() {
    let project = tempfile::tempdir().expect("a dir");
    let rules = rules(
        project.path(),
        Grammar::Cmd,
        &json!([
            "Bash(echo:*)",
            "Bash(git status)",
            "Bash(type C:\\notes\\a.txt)"
        ]),
        &json!([]),
    );
    for command in [
        "echo hi",
        "echo \"a & b | c\"",
        "echo a ^& b",
        "echo hi > out.txt",
        "echo hi 2>&1",
        "git status && echo done",
        "@echo hi",
        "type C:\\notes\\a.txt",
    ] {
        assert_eq!(
            verdict(&rules, command),
            Some(Verdict::Allow),
            "{command:?} was not allowed"
        );
    }
    for command in [
        "echo ok && whoami",
        "echo ok & whoami",
        "echo ok | whoami",
        "echo ok || whoami",
        "echo ok&whoami",
        "echo %COMSPEC%",
        "%COMSPEC% /c whoami",
        "echo \"unclosed & whoami",
        "(echo a) & whoami",
        "(echo a)",
        "echo a\nwhoami",
        "echo a\r\nwhoami",
        "echo \"a\\\"&whoami\"",
        "echo \"a\"\"&whoami\"",
        "echo ^\"a & whoami",
        "echo a ^& b | echo c",
        "echo a ^",
        "echo hi >",
        "git status & del x",
        "@echo a & whoami",
        "cmd /c echo hi",
        "echo/x",
    ] {
        assert_eq!(verdict(&rules, command), None, "{command:?} was allowed");
    }
}

#[test]
fn a_deny_holds_however_a_cmd_line_is_dressed() {
    let project = tempfile::tempdir().expect("a dir");
    let rules = rules(
        project.path(),
        Grammar::Cmd,
        &json!(["Bash"]),
        &json!(["Bash(del:*)"]),
    );
    for command in [
        "del x",
        "  del x",
        "echo a & del x",
        "echo a&&del x",
        "echo a || del x",
        "echo a|del x",
        "echo 'a & del x'",
        "@del x",
        ";del x",
        "DEL x",
        "Del x",
        "del/q x",
        "del,x",
        "del.x",
        "C:\\bin\\del.exe x",
        "\"del\" x",
        "d^el x",
        "if exist x del x",
        "if not exist x (del x)",
        "for %f in (*.o) do del %f",
        "cmd /c \"del x\"",
        "cmd /c del x",
        "cmd /s /cdel x",
        "start \"\" del x",
        "start /b del x",
        "call del x",
        "(del x)",
        "echo a | echo ^& del x",
        "echo ok\ndel x",
        "echo hi > out.txt & del x",
    ] {
        assert!(
            matches!(verdict(&rules, command), Some(Verdict::Deny(_))),
            "{command:?} slipped past the deny"
        );
    }
    for command in [
        "echo del",
        "type del.txt",
        "git commit -m \"del x\"",
        "echo a ^& del x",
    ] {
        assert_eq!(
            verdict(&rules, command),
            Some(Verdict::Allow),
            "{command:?} was denied"
        );
    }
}
