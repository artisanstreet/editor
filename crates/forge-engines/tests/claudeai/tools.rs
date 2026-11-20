use super::*;
use crate::claudeai::rules::Grammar;

fn call(name: &str, input: Value) -> Call {
    Call {
        id: "t1".to_owned(),
        name: name.to_owned(),
        input,
    }
}

fn plan(call: &Call, permissions: Permissions, allowed: &Allowed, rules: &Rules) -> Plan {
    super::plan(call, permissions, allowed, rules, &mcp::Servers::default())
}

fn preview(call: &Call, cwd: &Path) -> ItemKind {
    super::preview(call, cwd, &mcp::Servers::default())
}

async fn web_fetch(call: &Call, rules: &Rules, local_ok: bool) -> Answer {
    super::web_fetch(call, rules, local_ok, &|_| None).await
}

fn runtime() -> Runtime {
    Runtime {
        skills: Vec::new(),
        mcp: mcp::Servers::default(),
        shells: shells::Shells::default(),
        rules: Rules::default(),
        containment: Containment::Shared,
    }
}

#[test]
fn a_read_runs_and_a_change_asks_unless_full_or_always_allowed() {
    let allowed = Allowed::default();
    let rules = Rules::default();
    let read = call("Read", json!({ "file_path": "/x" }));
    assert_eq!(plan(&read, Permissions::Auto, &allowed, &rules), Plan::Run);
    let write = call("Write", json!({ "file_path": "/x", "content": "y" }));
    assert_eq!(
        plan(&write, Permissions::Auto, &allowed, &rules),
        Plan::Ask(ApprovalRequest::FileChange)
    );
    assert_eq!(plan(&write, Permissions::Full, &allowed, &rules), Plan::Run);
    let mut allowed = Allowed::default();
    allowed.remember(&write, Decision::AllowAlways);
    assert_eq!(plan(&write, Permissions::Auto, &allowed, &rules), Plan::Run);
    let bash = call("Bash", json!({ "command": "ls" }));
    assert!(matches!(
        plan(&bash, Permissions::Auto, &allowed, &rules),
        Plan::Ask(_)
    ));
    allowed.remember(&bash, Decision::AllowOnce);
    assert!(matches!(
        plan(&bash, Permissions::Auto, &allowed, &rules),
        Plan::Ask(_)
    ));
    allowed.remember(&bash, Decision::AllowAlways);
    assert_eq!(plan(&bash, Permissions::Auto, &allowed, &rules), Plan::Run);
}

#[test]
fn mcp_calls_ask_under_auto_and_run_under_full_or_once_allowed() {
    let rules = Rules::default();
    let tool = call("mcp__docs__search", json!({ "query": "x" }));
    assert!(matches!(
        plan(&tool, Permissions::Auto, &Allowed::default(), &rules),
        Plan::Ask(_)
    ));
    assert_eq!(
        plan(&tool, Permissions::Full, &Allowed::default(), &rules),
        Plan::Run
    );
    let mut allowed = Allowed::default();
    allowed.remember(&tool, Decision::AllowAlways);
    assert_eq!(plan(&tool, Permissions::Auto, &allowed, &rules), Plan::Run);
}

#[test]
fn todo_and_task_run_without_asking_and_keep_their_items() {
    let rules = Rules::default();
    let allowed = Allowed::default();
    let todo = call(
        "TodoWrite",
        json!({ "todos": [
            { "content": "Read the code", "status": "in_progress" },
            { "content": "Write the fix", "status": "pending" }
        ] }),
    );
    assert_eq!(plan(&todo, Permissions::Auto, &allowed, &rules), Plan::Run);
    let ItemKind::Plan { steps, .. } = preview(&todo, Path::new("/")) else {
        panic!("a plan item");
    };
    let steps = steps.inline().expect("inline steps");
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].status, artisan_api::StepStatus::InProgress);
    let task = call(
        "Task",
        json!({ "description": "Find things", "prompt": "Find the things." }),
    );
    assert_eq!(plan(&task, Permissions::Auto, &allowed, &rules), Plan::Run);
    let ItemKind::Agent {
        description,
        prompt,
        ..
    } = preview(&task, Path::new("/"))
    else {
        panic!("an agent item");
    };
    assert_eq!(description.inline(), Some("Find things"));
    assert_eq!(
        prompt.and_then(|prompt| prompt.inline().map(str::to_owned)),
        Some("Find the things.".to_owned())
    );
}

#[test]
fn a_malformed_todo_preview_stays_a_tool_call() {
    let item = preview(
        &call("TodoWrite", json!({ "todos": [{ "status": "pending" }] })),
        Path::new("/"),
    );
    assert!(
        matches!(&item, ItemKind::Tool { tool: ToolKind::Other { name, .. }, .. } if name == "TodoWrite"),
        "{item:?}"
    );
}

#[test]
fn accept_edits_runs_writes_without_asking() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let dir = project.path().join(".claude");
    std::fs::create_dir_all(&dir).expect("the settings dir");
    std::fs::write(
        dir.join("settings.json"),
        r#"{"permissions": {"defaultMode": "acceptEdits"}}"#,
    )
    .expect("settings");
    let rules = Rules::load(home.path(), project.path());
    let write = call("Write", json!({ "file_path": "x", "content": "y" }));
    assert_eq!(
        plan(&write, Permissions::Auto, &Allowed::default(), &rules),
        Plan::Run
    );
}

#[test]
fn a_tool_forge_does_not_offer_is_refused_with_the_ones_it_does() {
    let Plan::Refuse(reason) = plan(
        &call("bash_tool", json!({})),
        Permissions::Full,
        &Allowed::default(),
        &Rules::default(),
    ) else {
        panic!("refused");
    };
    assert!(reason.contains("Bash"));
}

#[test]
fn html_becomes_readable_text() {
    let html = "<html><head><title>T</title><style>p { color: red }</style></head>\
                <body><h1>Hello &amp; welcome</h1><p>One <b>two</b> three.</p>\
                <script>alert('no')</script><ul><li>a</li><li>b</li></ul></body></html>";
    let text = html_to_text(html);
    assert!(text.contains("Hello & welcome"), "{text}");
    assert!(text.contains("One two three."), "{text}");
    assert!(text.contains("a\n\nb"), "{text}");
    assert!(!text.contains("alert"), "{text}");
    assert!(!text.contains("color:red"), "{text}");
}

#[test]
fn a_preview_shows_the_change_before_anything_runs() {
    let work = tempfile::tempdir().expect("a dir");
    let path = work.path().join("a.txt");
    let edit = call(
        "Edit",
        json!({ "file_path": path.display().to_string(), "old_string": "one", "new_string": "two" }),
    );
    let ItemKind::Tool {
        tool: ToolKind::Edit { changes },
        ..
    } = preview(&edit, work.path())
    else {
        panic!("an edit item");
    };
    assert!(
        changes
            .inline()
            .is_some_and(|changes| changes[0].diff.is_some())
    );
    assert!(!path.exists(), "a preview touches nothing");
}

#[tokio::test]
async fn read_numbers_its_lines_and_edit_changes_exactly_once() {
    let work = tempfile::tempdir().expect("a dir");
    let path = work.path().join("a.txt");
    std::fs::write(&path, "one\ntwo\n").expect("a file");
    let cancel = CancellationToken::new();
    let (answer, _) = execute(
        &call("Read", json!({ "file_path": path.display().to_string() })),
        work.path(),
        &cancel,
        &runtime(),
    )
    .await;
    assert_eq!(answer.text, "     1\tone\n     2\ttwo\n");
    let edit = call(
        "Edit",
        json!({ "file_path": "a.txt", "old_string": "two", "new_string": "three" }),
    );
    let (answer, _) = execute(&edit, work.path(), &cancel, &runtime()).await;
    assert!(!answer.is_error, "{}", answer.text);
    assert_eq!(
        std::fs::read_to_string(&path).expect("the file"),
        "one\nthree\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn read_refuses_a_device_that_never_ends() {
    let work = tempfile::tempdir().expect("a dir");
    let (answer, _) = tokio::time::timeout(
        Duration::from_secs(10),
        execute(
            &call("Read", json!({ "file_path": "/dev/zero" })),
            work.path(),
            &CancellationToken::new(),
            &runtime(),
        ),
    )
    .await
    .expect("Read answered instead of reading forever");
    assert!(answer.is_error);
    assert!(
        answer.text.contains("not a regular file"),
        "{}",
        answer.text
    );
}

#[tokio::test]
async fn a_capped_glob_lists_the_first_files_by_name() {
    let work = tempfile::tempdir().expect("a dir");
    for index in (0..MAX_MATCHES + 100).rev() {
        std::fs::write(work.path().join(format!("f{index:05}.txt")), "").expect("file");
    }
    let cancel = CancellationToken::new();
    let (found, _) = execute(
        &call("Glob", json!({ "pattern": "*.txt" })),
        work.path(),
        &cancel,
        &runtime(),
    )
    .await;
    let listed = found.text.lines().take(MAX_MATCHES).collect::<Vec<_>>();
    let first = (0..MAX_MATCHES)
        .map(|index| format!("f{index:05}.txt"))
        .collect::<Vec<_>>();
    assert_eq!(listed, first);
    assert!(
        found.text.ends_with("narrow the pattern]"),
        "{}",
        found.text
    );
}

#[test]
fn entities_decode_between_bare_ampersands() {
    let text = html_to_text("<p>a=1&b=2&c=3 &amp; d &#x41;&#66;&hellip;</p>");
    assert_eq!(text, "a=1&b=2&c=3 & d AB…");
}

#[tokio::test]
async fn glob_and_grep_honour_gitignore() {
    let work = tempfile::tempdir().expect("a dir");
    std::fs::write(work.path().join(".gitignore"), "build/\n").expect("ignore");
    std::fs::create_dir_all(work.path().join("src")).expect("src");
    std::fs::create_dir_all(work.path().join("build")).expect("build");
    std::fs::write(work.path().join("src/lib.rs"), "fn needle() {}\n").expect("lib");
    std::fs::write(work.path().join("build/out.rs"), "fn needle() {}\n").expect("out");
    let cancel = CancellationToken::new();
    let (found, _) = execute(
        &call("Glob", json!({ "pattern": "**/*.rs" })),
        work.path(),
        &cancel,
        &runtime(),
    )
    .await;
    assert_eq!(found.text, "src/lib.rs");
    let (hits, _) = execute(
        &call("Grep", json!({ "pattern": "needle" })),
        work.path(),
        &cancel,
        &runtime(),
    )
    .await;
    assert_eq!(hits.text, "src/lib.rs:1:fn needle() {}");
}

#[cfg(unix)]
#[tokio::test]
async fn a_failing_command_tells_the_model_its_exit_code() {
    let work = tempfile::tempdir().expect("a dir");
    let cancel = CancellationToken::new();
    let (answer, item) = execute(
        &call(
            "Bash",
            json!({ "command": "echo out; echo err >&2; exit 3" }),
        ),
        work.path(),
        &cancel,
        &runtime(),
    )
    .await;
    assert!(answer.is_error);
    assert_eq!(answer.text, "out\nerr\n[exit code 3]");
    assert!(matches!(
        item,
        ItemKind::Tool {
            exit_code: Some(3),
            ..
        }
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn a_stop_ends_the_command_and_what_it_started() {
    let work = tempfile::tempdir().expect("a dir");
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    let started = work.path().join("started");
    tokio::spawn(async move {
        test_support::wait::eventually(
            "the command to start",
            test_support::wait::LIVENESS,
            || std::future::ready(started.exists()),
        )
        .await;
        stopper.cancel();
    });
    let (answer, _) = test_support::wait::within(
        "the stopped command and what it started to end",
        test_support::wait::LIVENESS,
        execute(
            &call(
                "Bash",
                json!({ "command": "sleep 600 & touch started; sleep 600" }),
            ),
            work.path(),
            &cancel,
            &runtime(),
        ),
    )
    .await;
    assert_eq!(answer.text, "The command was stopped.");
}

fn project_rules(home: &Path, project: &Path, permissions: &Value) -> Rules {
    project_rules_in(home, project, permissions, Grammar::default())
}

fn project_rules_in(home: &Path, project: &Path, permissions: &Value, grammar: Grammar) -> Rules {
    let dir = project.join(".claude");
    std::fs::create_dir_all(&dir).expect("the settings dir");
    std::fs::write(
        dir.join("settings.json"),
        json!({ "permissions": permissions }).to_string(),
    )
    .expect("settings");
    Rules::load_for(home, project, grammar)
}

#[test]
fn a_compound_command_is_not_allowed_by_a_rule_for_its_first_part() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let rules = project_rules(
        home.path(),
        project.path(),
        &json!({ "allow": ["Bash(npm run test:*)"] }),
    );
    let allowed = Allowed::default();
    let fine = call("Bash", json!({ "command": "npm run test -- --ci" }));
    assert_eq!(plan(&fine, Permissions::Auto, &allowed, &rules), Plan::Run);
    let sneaky = call("Bash", json!({ "command": "npm run test && rm -rf ~" }));
    assert_eq!(
        plan(&sneaky, Permissions::Auto, &allowed, &rules),
        Plan::Ask(ApprovalRequest::Command {
            command: TextContent::from("npm run test && rm -rf ~".to_owned()),
            cwd: None,
        })
    );
}

#[test]
fn a_denied_command_is_refused_even_under_full_access_however_it_is_dressed() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let rules = project_rules_in(
        home.path(),
        project.path(),
        &json!({ "deny": ["Bash(rm:*)"] }),
        Grammar::Posix,
    );
    for command in [
        " rm -rf /",
        "FOO=1 rm -rf /",
        "true && rm -rf /",
        "cd /; rm -rf /",
        "(rm -rf /)",
        "bash -c 'rm -rf /'",
        "/bin/rm -rf /",
    ] {
        let bash = call("Bash", json!({ "command": command }));
        assert!(
            matches!(
                plan(&bash, Permissions::Full, &Allowed::default(), &rules),
                Plan::Refuse(_)
            ),
            "{command:?} would have run"
        );
    }
}

#[test]
fn always_allowing_a_command_does_not_skip_an_ask_rule() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let rules = project_rules(
        home.path(),
        project.path(),
        &json!({ "ask": ["Bash(git push:*)"] }),
    );
    let push = call("Bash", json!({ "command": "git push --force" }));
    let mut allowed = Allowed::default();
    allowed.remember(&push, Decision::AllowAlways);
    assert!(matches!(
        plan(&push, Permissions::Auto, &allowed, &rules),
        Plan::Ask(_)
    ));
    assert_eq!(plan(&push, Permissions::Full, &allowed, &rules), Plan::Run);
}

#[tokio::test]
async fn a_search_never_shows_a_denied_file() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let rules = project_rules(
        home.path(),
        project.path(),
        &json!({ "deny": ["Read(.env)", "Read(secrets/**)"] }),
    );
    std::fs::create_dir_all(project.path().join("secrets")).expect("secrets");
    std::fs::write(project.path().join(".env"), "TOKEN=hunter2\n").expect("env");
    std::fs::write(project.path().join("secrets/key"), "TOKEN=hunter3\n").expect("key");
    std::fs::write(project.path().join("app.rs"), "// TOKEN=placeholder\n").expect("app");
    let grep = call("Grep", json!({ "pattern": "TOKEN", "path": ".env" }));
    assert!(matches!(
        plan(&grep, Permissions::Full, &Allowed::default(), &rules),
        Plan::Refuse(_)
    ));
    let mut runtime = runtime();
    runtime.rules = rules;
    let cancel = CancellationToken::new();
    let (hits, _) = execute(
        &call("Grep", json!({ "pattern": "TOKEN" })),
        project.path(),
        &cancel,
        &runtime,
    )
    .await;
    assert_eq!(hits.text, "app.rs:1:// TOKEN=placeholder");
    let (found, _) = execute(
        &call("Glob", json!({ "pattern": "**/*" })),
        project.path(),
        &cancel,
        &runtime,
    )
    .await;
    assert!(!found.text.contains("secrets/key"), "{}", found.text);
}

#[test]
fn accept_edits_asks_for_a_change_outside_the_project() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let elsewhere = tempfile::tempdir().expect("elsewhere");
    let rules = project_rules(
        home.path(),
        project.path(),
        &json!({ "defaultMode": "acceptEdits" }),
    );
    let allowed = Allowed::default();
    let inside = call(
        "Edit",
        json!({ "file_path": "src/../a.txt", "old_string": "a", "new_string": "b" }),
    );
    assert_eq!(
        plan(&inside, Permissions::Auto, &allowed, &rules),
        Plan::Run
    );
    for path in [
        elsewhere.path().join("a.txt").display().to_string(),
        "../escape.txt".to_owned(),
    ] {
        let outside = call("Write", json!({ "file_path": path, "content": "x" }));
        assert_eq!(
            plan(&outside, Permissions::Auto, &allowed, &rules),
            Plan::Ask(ApprovalRequest::FileChange),
            "{path}"
        );
    }
}

#[test]
fn a_change_to_permission_settings_always_asks_short_of_full_access() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let rules = project_rules(
        home.path(),
        project.path(),
        &json!({ "defaultMode": "acceptEdits", "allow": ["Edit", "Write"] }),
    );
    let mut allowed = Allowed::default();
    allowed.remember(
        &call("Write", json!({ "file_path": "a.txt", "content": "" })),
        Decision::AllowAlways,
    );
    for path in [
        ".claude/settings.json".to_owned(),
        ".claude/settings.local.json".to_owned(),
        "sub/.Claude/settings.json".to_owned(),
        ".mcp.json".to_owned(),
        home.path()
            .join(".claude/settings.json")
            .display()
            .to_string(),
        home.path().join("session.json").display().to_string(),
    ] {
        let write = call(
            "Write",
            json!({ "file_path": path, "content": r#"{"permissions":{"allow":["Bash"]}}"# }),
        );
        assert_eq!(
            plan(&write, Permissions::Auto, &allowed, &rules),
            Plan::Ask(ApprovalRequest::FileChange),
            "{path}"
        );
        assert_eq!(plan(&write, Permissions::Full, &allowed, &rules), Plan::Run);
    }
    let ordinary = call("Write", json!({ "file_path": "a.txt", "content": "" }));
    assert_eq!(
        plan(&ordinary, Permissions::Auto, &allowed, &rules),
        Plan::Run
    );
}

#[test]
fn web_fetch_asks_per_site_under_auto() {
    let rules = Rules::default();
    let page = call("WebFetch", json!({ "url": "https://Docs.Example.com/a" }));
    assert_eq!(
        plan(&page, Permissions::Auto, &Allowed::default(), &rules),
        Plan::Ask(ApprovalRequest::Action {
            description: TextContent::from(
                "Fetch a page from docs.example.com: https://Docs.Example.com/a".to_owned()
            ),
        })
    );
    assert_eq!(
        plan(&page, Permissions::Full, &Allowed::default(), &rules),
        Plan::Run
    );
    let mut allowed = Allowed::default();
    allowed.remember(&page, Decision::AllowAlways);
    let same = call("WebFetch", json!({ "url": "https://docs.example.com/b" }));
    assert_eq!(plan(&same, Permissions::Auto, &allowed, &rules), Plan::Run);
    let other = call("WebFetch", json!({ "url": "https://other.example/b" }));
    assert!(matches!(
        plan(&other, Permissions::Auto, &allowed, &rules),
        Plan::Ask(_)
    ));
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let rules = project_rules(
        home.path(),
        project.path(),
        &json!({ "allow": ["WebFetch(domain:other.example)"] }),
    );
    assert_eq!(
        plan(&other, Permissions::Auto, &Allowed::default(), &rules),
        Plan::Run
    );
}

#[test]
fn an_mcp_card_names_the_server_and_tool_as_configured() {
    assert_eq!(
        mcp_request("mcp__my_srv__search_7f3a", Some(("my__srv", "search"))),
        ApprovalRequest::Action {
            description: TextContent::from("Run search on the my__srv MCP server".to_owned()),
        }
    );
    assert_eq!(
        mcp_request("mcp__my__srv__search", None),
        ApprovalRequest::Action {
            description: TextContent::from("Run search on the my__srv MCP server".to_owned()),
        }
    );
}

#[test]
fn the_environment_s_proxy_applies_unless_no_proxy_exempts_the_host() {
    let vars = |set: &'static [(&'static str, &'static str)]| {
        move |name: &str| {
            set.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        }
    };
    let url = |url: &str| reqwest::Url::parse(url).expect("a url");
    let env = vars(&[
        ("HTTPS_PROXY", "http://proxy.corp:3128"),
        ("no_proxy", "internal.corp, .lan,10.0.0.0/8"),
    ]);
    assert_eq!(
        proxy_for(&url("https://docs.rs/x"), &env).as_deref(),
        Some("http://proxy.corp:3128")
    );
    assert_eq!(
        proxy_for(&url("http://docs.rs/x"), &env),
        None,
        "no HTTP_PROXY"
    );
    for exempt in [
        "https://internal.corp/",
        "https://wiki.internal.corp/",
        "https://nas.lan/",
        "https://10.1.2.3/",
    ] {
        assert_eq!(proxy_for(&url(exempt), &env), None, "{exempt}");
    }
    assert!(proxy_for(&url("https://notinternal.corp/"), &env).is_some());
    let all = vars(&[("ALL_PROXY", "http://proxy.corp:3128"), ("NO_PROXY", "*")]);
    assert_eq!(proxy_for(&url("http://docs.rs/"), &all), None);
}

#[tokio::test]
async fn web_fetch_goes_through_the_user_s_proxy() {
    let proxy = serve(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 13\r\nConnection: close\r\n\r\nvia the proxy",
    )
    .await;
    let vars = move |name: &str| (name == "HTTP_PROXY").then(|| proxy.clone());
    let answer = super::web_fetch(
        &call(
            "WebFetch",
            json!({ "url": "http://artisan-proxy-test.invalid/page" }),
        ),
        &Rules::default(),
        false,
        &vars,
    )
    .await;
    assert!(!answer.is_error, "{}", answer.text);
    assert!(answer.text.ends_with("via the proxy"), "{}", answer.text);
    let answer = super::web_fetch(
        &call("WebFetch", json!({ "url": "http://127.0.0.1:9/" })),
        &Rules::default(),
        false,
        &vars,
    )
    .await;
    assert!(
        answer.text.contains("local or private"),
        "a private address is refused through the proxy too: {}",
        answer.text
    );
}

#[test]
fn only_public_addresses_are_public() {
    for address in [
        "127.0.0.1",
        "10.1.2.3",
        "172.16.0.1",
        "192.168.1.1",
        "169.254.169.254",
        "0.0.0.0",
        "100.64.0.1",
        "224.0.0.1",
        "255.255.255.255",
        "::1",
        "::",
        "fe80::1",
        "fd00::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::ffff:10.0.0.1",
        "64:ff9b::a9fe:a9fe",
    ] {
        assert!(
            !is_public(address.parse().expect("an address")),
            "{address}"
        );
    }
    for address in ["93.184.216.34", "1.1.1.1", "2606:4700::1111"] {
        assert!(is_public(address.parse().expect("an address")), "{address}");
    }
}

#[tokio::test]
async fn web_fetch_refuses_local_and_private_hosts() {
    let rules = Rules::default();
    for url in [
        "http://127.0.0.1:9/",
        "http://localhost:9/",
        "http://[::ffff:127.0.0.1]:9/",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
    ] {
        let answer = web_fetch(&call("WebFetch", json!({ "url": url })), &rules, false).await;
        assert!(answer.is_error, "{url}");
        assert!(
            answer.text.contains("local or private"),
            "{url}: {}",
            answer.text
        );
    }
}

async fn serve(response: &'static [u8]) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a listener");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = vec![0u8; 8192];
            drop(socket.read(&mut request).await);
            drop(socket.write_all(response).await);
            drop(socket.shutdown().await);
        }
    });
    format!("http://{address}/")
}

#[tokio::test]
async fn a_redirect_to_a_denied_domain_is_not_followed() {
    let home = tempfile::tempdir().expect("a home");
    let project = tempfile::tempdir().expect("a project");
    let rules = project_rules(
        home.path(),
        project.path(),
        &json!({ "deny": ["WebFetch(domain:denied.example)"] }),
    );
    let url = serve(
        b"HTTP/1.1 302 Found\r\nLocation: http://denied.example/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
    let answer = web_fetch(&call("WebFetch", json!({ "url": url })), &rules, true).await;
    assert!(answer.is_error, "{}", answer.text);
    assert!(answer.text.contains("refused"), "{}", answer.text);
    let answer = web_fetch(
        &call("WebFetch", json!({ "url": url })),
        &Rules::default(),
        true,
    )
    .await;
    assert!(
        answer
            .text
            .contains("redirects to another host: http://denied.example/secret"),
        "a redirect off the site is handed back, not followed: {}",
        answer.text
    );
}

#[tokio::test]
async fn web_fetch_shows_no_binary_and_no_cut_off_page() {
    let rules = Rules::default();
    let pdf = serve(
        b"HTTP/1.1 200 OK\r\nContent-Type: application/pdf\r\nContent-Length: 9\r\nConnection: close\r\n\r\n%PDF-1.7\n",
    )
    .await;
    let answer = web_fetch(&call("WebFetch", json!({ "url": pdf })), &rules, true).await;
    assert!(answer.is_error);
    assert!(
        answer
            .text
            .contains("binary content (application/pdf, 9 bytes)"),
        "{}",
        answer.text
    );
    let unlabelled = serve(
        b"HTTP/1.1 200 OK\r\nContent-Length: 12\r\nConnection: close\r\n\r\n\x89PNG\r\n\x1a\n\0\0\0\0",
    )
    .await;
    let answer = web_fetch(
        &call("WebFetch", json!({ "url": unlabelled })),
        &rules,
        true,
    )
    .await;
    assert!(answer.text.contains("binary content"), "{}", answer.text);
    let broken = serve(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 1000\r\nConnection: close\r\n\r\nthe start of",
    )
    .await;
    let answer = web_fetch(&call("WebFetch", json!({ "url": broken })), &rules, true).await;
    assert!(answer.is_error, "{}", answer.text);
    assert!(!answer.text.contains("the start of"), "{}", answer.text);
}

#[tokio::test]
async fn web_fetch_decodes_the_declared_charset_and_carries_the_prompt() {
    let page = serve(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 70\r\nConnection: close\r\n\r\n<html><head><meta charset=\"iso-8859-1\"></head><body>Caf\xe9</body></html>",
    )
    .await;
    let answer = web_fetch(
        &call(
            "WebFetch",
            json!({ "url": page, "prompt": "What is sold?" }),
        ),
        &Rules::default(),
        true,
    )
    .await;
    assert!(!answer.is_error, "{}", answer.text);
    assert!(
        answer.text.contains("Requested: What is sold?"),
        "{}",
        answer.text
    );
    assert!(answer.text.ends_with("Café"), "{}", answer.text);
}
