use super::*;

#[test]
fn names_are_unique_among_held_names_then_numbered() {
    let mut held: Vec<String> = Vec::new();
    for agent in 0..AGENT_NAMES.len() + 3 {
        let name =
            HeldAgentNames::new(held.iter().map(String::as_str)).choose(&format!("agent-{agent}"));
        assert!(!held.contains(&name), "{name} reused");
        held.push(name);
    }
    assert!(
        held[..AGENT_NAMES.len()]
            .iter()
            .all(|name| !name.contains(' '))
    );
    assert!(
        held[AGENT_NAMES.len()..]
            .iter()
            .all(|name| name.ends_with(" 2"))
    );
}

#[test]
fn a_waking_agent_keeps_its_name_unless_another_holds_it() {
    let held = HeldAgentNames::new(["Thea"]);
    assert_eq!(held.keep_or_choose(Some("Ada".into()), "agent-1"), "Ada");
    let renamed = held.keep_or_choose(Some("thea".into()), "agent-1");
    assert!(!held.holds(&renamed), "{renamed} is held");
}
