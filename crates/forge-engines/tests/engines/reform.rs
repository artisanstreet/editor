use super::*;
#[test]
fn published_version_precedence_matches_provider_formats() {
    assert_eq!(compare_versions("1.0.0", "1.0.0-beta.2"), Ordering::Greater);
    assert_eq!(compare_versions("1.0.0+one", "1.0.0+two"), Ordering::Equal);
    assert_eq!(
        compare_versions("0.0.0-beta-19271", "0.0.0-beta-19270"),
        Ordering::Greater
    );
    assert_eq!(
        compare_versions("2026.10.01-e373342", "2026.09.30-acde"),
        Ordering::Greater
    );
}

fn variables(env: &[(String, String)]) -> std::collections::HashMap<&str, &str> {
    env.iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

#[test]
fn engines_keep_their_state_in_the_managed_home_and_tools_see_the_users_home() {
    let home = std::path::Path::new("/data/engines/x/home");
    for engine in EngineId::ALL {
        let env = environment(engine, None, home);
        let env = variables(&env);
        let mut user = vec!["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA"];
        if engine != EngineId::OpenCode {
            user.extend(["XDG_CONFIG_HOME", "XDG_DATA_HOME"]);
        }
        for variable in user {
            assert!(
                !env.contains_key(variable),
                "{engine:?} overrides {variable}"
            );
        }
    }
    let claude = environment(EngineId::Claude, None, home);
    assert_eq!(
        variables(&claude).get("CLAUDE_CONFIG_DIR").copied(),
        Some(home.join(".claude").to_str().unwrap())
    );
    let codex = environment(EngineId::Codex, None, home);
    assert_eq!(
        variables(&codex).get("CODEX_HOME").copied(),
        Some(home.join(".codex").to_str().unwrap())
    );
}

#[test]
fn opencode_uses_its_own_home_variable_only_when_the_build_understands_it() {
    let Some(artifact) = host_spec(EngineId::OpenCode).artifact else {
        return;
    };
    let home = tempfile::tempdir().unwrap();
    let install = tempfile::tempdir().unwrap();
    let binary = install.path().join(artifact.entry);

    let upstream = environment(EngineId::OpenCode, Some(&binary), home.path());
    let upstream = variables(&upstream);
    assert!(!upstream.contains_key("OPENCODE_HOME"));
    assert_eq!(
        upstream.get("XDG_DATA_HOME").copied(),
        Some(home.path().join("data").to_str().unwrap())
    );

    std::fs::write(
        install.path().join("artisan.json"),
        r#"{"upstream":"v2.1.2","features":["OPENCODE_HOME"]}"#,
    )
    .unwrap();
    let artisan = environment(EngineId::OpenCode, Some(&binary), home.path());
    let artisan = variables(&artisan);
    assert_eq!(artisan.get("OPENCODE_HOME").copied(), home.path().to_str());
    assert!(
        !artisan.keys().any(|key| key.starts_with("XDG_")),
        "{artisan:?}"
    );
}
