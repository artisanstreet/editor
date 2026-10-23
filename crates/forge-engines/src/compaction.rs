//! Portable compaction prompt, adapted from OpenCode's session/compaction.ts.
//! Keep it independent of provider setup and tool schemas so forks can reuse their prefix.

/// OpenCode's summary template and preservation rules.
#[must_use]
pub fn compaction_prompt() -> String {
    [
        "You MUST summarize the conversation above into a structured summary that will be given to another agent to resume the work.",
        "If the conversation contains an existing checkpoint, update it into one consolidated summary. Newer history takes precedence: remove resolved blockers and answered questions, move completed work out of Active, and preserve pending work. Do not reproduce checkpoint wrapper tags.",
        "Summarize only what the user and the assistant said and did. Leave out instructions and setup the assistant was given rather than told by the user: repository conventions, instruction files such as AGENTS.md, and environment details like the session ID. The next agent receives current versions of all of these separately.",
        TEMPLATE,
        RULES,
        "Do not continue the task or call tools.",
        "Return only the structured summary in the requested format. Do not include a preamble, explanation, or other commentary.",
    ].join("\n\n")
}

const TEMPLATE: &str = r#"You MUST use this format for your response (you may omit sections that aren't applicable). Do not include the <template> tags in your response.
<template>
## Objective
- [one or two brief sentences describing what the user is trying to accomplish]

## Requirements
- [constraints, preferences, requirements, and scope boundaries stated by the user, or "(none)"]

## Decisions
- [decisions already made and why, or "(none)"]

## Work State
Break the objective into smaller goals and report which are completed, which are being worked on, and which are blocked.
### Completed
- [goals that have been completed; otherwise "(none)"]

### Active
- [goals currently being worked on; otherwise "(none)"]

### Blocked
- [anything blocking progress, and why; otherwise "(none)"]

## Next Move
1. [ordered list of next actions, or "(none)"]

## Relevant Files
List the files and directories, other than the current working directory, that another agent would need to open to continue this work. Include at most 15, most important first. Do not list every file that was read or changed. Include paths outside the current working directory when relevant. If none, write "(none)".
- `[file or directory path]`: [brief reason it matters]

## Important Context
- [facts the next agent cannot continue without and cannot easily find on its own; or "(none)"]
</template>"#;
const RULES: &str = r"Rules:
- Keep each section concise. Use terse, single-line bullets, not prose paragraphs or nested lists.
- Prefer short references over detailed restatement. It is fine to leave out information the next agent can recover from the code or the files listed above.
- Preserve exact file paths, symbols, commands, error strings, URLs, and identifiers.
- Carry forward only user questions or requests that remain unanswered or require further action. Do not repeat ones that newer history has answered or resolved. Preserve exact wording when carrying one forward.
- Preserve consequential workflow state, including whether changes are uncommitted, committed, pushed, under review, or merged.
- Do not mention the summary process or that context was compacted.";

pub const COMPACTION_NUDGE: &str = "The previous response did not fill in the required summary template. Do not call tools. Return the summary as text using the exact section headings from the template.";

/// OpenCode permits inapplicable sections to be omitted. At least one
/// exact template heading must be present; empty prose or tool calls fail.
#[must_use]
pub fn is_compaction_summary(text: &str) -> bool {
    text.lines().any(|line| {
        let heading = line.trim();
        heading.starts_with("##") && TEMPLATE.lines().any(|line| line == heading)
    })
}
