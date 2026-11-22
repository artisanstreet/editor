use super::*;
#[cfg(unix)]
use std::sync::Arc;

const NOW: Timestamp = 1_791_709_200_000;

#[test]
fn claude_usage_text_becomes_windows() {
    let result = "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 3% used · resets Oct 11, 11:49am (UTC)\nCurrent week (all models): 34% used · resets Oct 13, 9:59pm (UTC)\nCurrent week (Fable): 0% used · resets Oct 13, 10pm (UTC)\n\nWhat's contributing to your limits usage?\nLast 24h · 3375 requests · 21 sessions\n  62% of your usage came from subagent-heavy sessions\n";
    insta::assert_debug_snapshot!(parse_claude_usage(result, NOW));
}

#[test]
fn codex_rate_limits_become_windows() {
    let recorded = json!({
        "rateLimits": { "limitId": "codex", "limitName": null, "primary": { "usedPercent": 2, "windowDurationMins": 10080, "resetsAt": 1_792_296_124 }, "secondary": null },
        "rateLimitsByLimitId": { "codex": { "limitId": "codex", "limitName": null, "primary": { "usedPercent": 2, "windowDurationMins": 10080, "resetsAt": 1_792_296_124 }, "secondary": null } }
    });
    let two_slots = json!({
        "rateLimitsByLimitId": { "codex": { "limitName": "GPT-5", "primary": { "usedPercent": 42.5, "windowDurationMins": 300, "resetsAt": 1_791_720_000 }, "secondary": { "usedPercent": 150.0, "windowDurationMins": 10080 } } }
    });
    insta::assert_debug_snapshot!((
        parse_codex_limits(&recorded),
        parse_codex_limits(&two_slots)
    ));
}

#[test]
fn opencode_go_usage_becomes_three_windows() {
    let recorded = json!({"usage":{"rolling":{"status":"ok","percent":0,"resetsAt":"2026-10-21T12:51:10.000Z"},"weekly":{"status":"ok","percent":0,"resetsAt":"2026-10-26T00:00:00.000Z"},"monthly":{"status":"ok","percent":25,"resetsAt":"2026-11-02T21:57:01.000Z"}}});
    let windows = parse_go_usage(&recorded);
    assert_eq!(
        windows
            .iter()
            .map(|window| (window.cadence, window.used_percent, window.resets_at))
            .collect::<Vec<_>>(),
        [
            (UsageCadence::Session, 0.0, Some(1_792_587_070_000)),
            (UsageCadence::Week, 0.0, Some(1_792_972_800_000)),
            (UsageCadence::Month, 25.0, Some(1_793_656_621_000)),
        ]
    );
    let limited =
        json!({"usage":{"rolling":{"status":"rate-limited","percent":97,"resetsAt":null}}});
    assert!(
        (parse_go_usage(&limited)[0].used_percent - 100.0).abs() < f64::EPSILON,
        "rate-limited usage reads as 100%"
    );
}

#[test]
fn a_go_key_wins_and_an_expired_console_sign_in_reads_nothing() {
    let console = |expires: i64| json!({"integrationID":"opencode","active":true,"value":{"type":"oauth","access":"console","expires":expires,"metadata":{"orgID":"org"}}});
    let go = json!({"integrationID":"opencode-go","active":true,"value":{"type":"key","key":"go"}});
    assert_eq!(
        go_credential(&json!({"data":[console(NOW + 1), go]}), NOW),
        Some(("go".to_owned(), None))
    );
    assert_eq!(
        go_credential(&json!([console(NOW + 1)]), NOW),
        Some(("console".to_owned(), Some("org".to_owned())))
    );
    assert_eq!(go_credential(&json!([console(NOW - 1)]), NOW), None);
}

#[test]
fn opencode_spend_sums_variants_costliest_first() {
    let models = json!([
        {"model":{"id":"minimax-m3.1-flash-free","providerID":"opencode-go","variant":"high"},"tokens":{"input":1_623_901,"output":54_112,"reasoning":78_255,"cache":{"read":71_616_226,"write":0}},"cost":0},
        {"model":{"id":"glm-5.2","providerID":"opencode-go"},"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"cost":0.42},
        {"model":{"id":"minimax-m3.1-flash-free","providerID":"opencode-go","variant":"max"},"tokens":{"input":374_992,"output":54_446,"reasoning":109_314,"cache":{"read":54_992_977,"write":0}},"cost":0}
    ]);
    let lines = spend_by_model(models.as_array().unwrap());
    assert_eq!(
        lines
            .iter()
            .map(|line| (line.label.as_str(), line.cost_usd, line.tokens))
            .collect::<Vec<_>>(),
        [
            ("glm-5.2", 0.42, 15),
            ("minimax-m3.1-flash-free", 0.0, 2_295_020),
        ]
    );
}

#[cfg(unix)]
fn gated_usage_cli(home: &Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let binary = home.join("fake-usage");
    std::fs::write(&binary, r#"#!/bin/sh
percentage=$(cat "$PWD/percentage")
printf 'probe\n' >> "$PWD/calls"
touch "$PWD/started"
while [ ! -e "$PWD/release" ]; do sleep 0.01; done
printf '{"result":"Current week (all models): %s%% used · resets Oct 13, 9:59pm (UTC)"}\n' "$percentage"
"#).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(home.join("percentage"), "34").unwrap();
    binary
}

#[cfg(unix)]
async fn wait_for_usage_probe(home: &Path) {
    let started = home.join("started");
    test_support::wait::eventually(
        "the usage probe to start",
        test_support::wait::LIVENESS,
        || std::future::ready(started.exists()),
    )
    .await;
}

#[cfg(unix)]
#[tokio::test]
#[expect(
    clippy::float_cmp,
    reason = "the percentage is parsed from an exact integer"
)]
async fn an_invalidated_old_account_read_cannot_repopulate_usage() {
    let directory = tempfile::tempdir().unwrap();
    let binary = gated_usage_cli(directory.path());
    let cache = Arc::new(UsageCache::new(Duration::from_secs(300)));
    let owner = Arc::clone(&cache);
    let home = directory.path().to_owned();
    let old_binary = binary.clone();
    let old_read =
        tokio::spawn(async move { owner.read(EngineId::Claude, &old_binary, &home, true).await });
    wait_for_usage_probe(directory.path()).await;
    cache.invalidate(EngineId::Claude);
    std::fs::write(directory.path().join("percentage"), "15").unwrap();
    std::fs::write(directory.path().join("release"), "").unwrap();
    assert!(old_read.await.unwrap().is_none());
    assert!(cache.cached(EngineId::Claude).is_none());
    let current = cache
        .read(EngineId::Claude, &binary, directory.path(), true)
        .await
        .unwrap();
    assert_eq!(current.windows[0].used_percent, 15.0);
}

#[cfg(unix)]
#[tokio::test]
async fn overlapping_forced_usage_reads_share_the_same_probe() {
    let directory = tempfile::tempdir().unwrap();
    let binary = gated_usage_cli(directory.path());
    let cache = Arc::new(UsageCache::new(Duration::from_secs(300)));
    let owner = Arc::clone(&cache);
    let home = directory.path().to_owned();
    let first_binary = binary.clone();
    let first = tokio::spawn(async move {
        owner
            .read(EngineId::Claude, &first_binary, &home, true)
            .await
    });
    wait_for_usage_probe(directory.path()).await;
    let owner = Arc::clone(&cache);
    let home = directory.path().to_owned();
    let (started, waiting) = tokio::sync::oneshot::channel();
    let second = tokio::spawn(async move {
        let _peer_gone = started.send(());
        owner.read(EngineId::Claude, &binary, &home, true).await
    });
    waiting.await.unwrap();
    std::fs::write(directory.path().join("release"), "").unwrap();
    assert_eq!(first.await.unwrap(), second.await.unwrap());
    assert_eq!(
        std::fs::read_to_string(directory.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_read_keeps_the_last_good_one() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let fail = dir.path().join("fail");
    let binary = dir.path().join("claude");
    let result = "Current week (all models): 34% used · resets Oct 13, 9:59pm (UTC)";
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\n[ -e {fail} ] && exit 1\necho '{{\"result\":\"{result}\"}}'\n",
            fail = fail.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cache = UsageCache::new(Duration::from_secs(300));
    let read = |force| cache.read(EngineId::Claude, &binary, dir.path(), force);
    let good = read(true).await.expect("the first read");
    std::fs::write(&fail, "").unwrap();
    let after = read(true).await;
    assert_eq!(
        after.as_ref(),
        Some(&good),
        "a failed read dropped the last good one"
    );
}
