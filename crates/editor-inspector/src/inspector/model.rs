use artisan_api::{Collection, InspectorSummary};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Fact(&'static str),
    Heading(&'static str),
    Agent(usize),
    Plan(usize),
    PlanCollection,
    Terminal(usize),
    Workflow(usize),
}

pub fn entries(summary: &InspectorSummary) -> Vec<Entry> {
    let mut entries = ["Project", "Forge", "Branch", "Worktree"]
        .into_iter()
        .map(Entry::Fact)
        .collect::<Vec<_>>();
    if summary.edits.is_some() {
        entries.push(Entry::Fact("Edit operations"));
    }
    if !summary.agents.is_empty() {
        entries.push(Entry::Heading("Agents"));
        entries.extend((0..summary.agents.len()).map(Entry::Agent));
    }
    if !summary.plan.is_empty() {
        entries.push(Entry::Heading("Checklist"));
        match &summary.plan {
            Collection::Inline(steps) => entries.extend((0..steps.len()).map(Entry::Plan)),
            Collection::Stored(_) => entries.push(Entry::PlanCollection),
        }
    }
    let terminals = |background| {
        summary
            .terminals
            .iter()
            .enumerate()
            .filter_map(move |(index, terminal)| {
                (terminal.background == background).then_some(Entry::Terminal(index))
            })
    };
    let foreground = terminals(false).collect::<Vec<_>>();
    if !foreground.is_empty() {
        entries.push(Entry::Heading("Terminals"));
        entries.extend(foreground);
    }
    let background = (0..summary.workflows.len())
        .map(Entry::Workflow)
        .chain(terminals(true))
        .collect::<Vec<_>>();
    if !background.is_empty() {
        entries.push(Entry::Heading("In the background"));
        entries.extend(background);
    }
    entries
}
