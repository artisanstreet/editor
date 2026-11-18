use super::*;

#[test]
fn names_are_unique_in_a_thread_then_numbered() {
    let mut taken: Vec<String> = Vec::new();
    for agent in 0..AGENT_NAMES.len() + 3 {
        let name = choose_agent_name(&format!("agent-{agent}"), taken.iter().map(String::as_str));
        assert!(!taken.contains(&name), "{name} reused");
        taken.push(name);
    }
    assert!(
        taken[..AGENT_NAMES.len()]
            .iter()
            .all(|name| !name.contains(' '))
    );
    assert!(
        taken[AGENT_NAMES.len()..]
            .iter()
            .all(|name| name.ends_with(" 2"))
    );
}
