use accesskit_consumer::{NodeRef, TreeState, common_filter};
use aqua_a11y_uia::accesskit::{Rect, Role, Toggled};
use std::fmt::Write as _;

#[derive(Clone, Copy)]
pub struct View<'a> {
    state: &'a TreeState,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub role: Role,
    pub label: Option<String>,
    pub value: Option<String>,
    pub focused: bool,
    pub disabled: bool,
    pub bounds: Option<Rect>,
    pub node: aqua_a11y_uia::accesskit::NodeId,
}

fn found(node: &NodeRef<'_>) -> Found {
    Found {
        role: node.role(),
        label: node.label(),
        value: node.value(),
        focused: node.is_focused(),
        disabled: node.is_disabled(),
        bounds: node.bounding_box(),
        node: node.locate().0,
    }
}

fn states(node: &NodeRef<'_>) -> Vec<String> {
    let toggled = node.toggled().map(|toggled| match toggled {
        Toggled::True => "checked",
        Toggled::False => "unchecked",
        Toggled::Mixed => "mixed",
    });
    let expanded = node
        .data()
        .is_expanded()
        .map(|expanded| if expanded { "expanded" } else { "collapsed" });
    let selected = node
        .is_selected()
        .filter(|selected| *selected)
        .map(|_| "selected");
    let numeric = node.numeric_value().map(|value| {
        match (node.min_numeric_value(), node.max_numeric_value()) {
            (Some(min), Some(max)) => format!("{value} of {min}..{max}"),
            _ => format!("{value}"),
        }
    });
    [toggled, expanded, selected]
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .chain(numeric)
        .chain(node.is_disabled().then(|| "disabled".to_owned()))
        .chain(node.is_focused().then(|| "focused".to_owned()))
        .collect()
}

fn structural(
    node: &NodeRef<'_>,
    label: Option<&str>,
    value: Option<&str>,
    states: &[String],
) -> bool {
    node.role() == Role::Group
        && label.is_none()
        && value.is_none()
        && states.iter().all(|state| state == "disabled")
}

fn write_node(out: &mut String, node: &NodeRef<'_>, depth: usize) {
    let label = node.label().filter(|label| !label.is_empty());
    let value = node
        .value()
        .filter(|value| !value.is_empty() && Some(value) != label.as_ref());
    let states = states(node);
    let depth = if structural(node, label.as_deref(), value.as_deref(), &states) {
        depth
    } else {
        write!(out, "{:indent$}{:?}", "", node.role(), indent = depth * 2)
            .expect("invariant: writing to a String succeeds");
        if let Some(label) = &label {
            write!(out, " {label:?}").expect("invariant: writing to a String succeeds");
        }
        if let Some(value) = &value {
            write!(out, " value={value:?}").expect("invariant: writing to a String succeeds");
        }
        if !states.is_empty() {
            write!(out, " [{}]", states.join(", "))
                .expect("invariant: writing to a String succeeds");
        }
        out.push('\n');
        depth + 1
    };
    for child in node.filtered_children(&common_filter) {
        write_node(out, &child, depth);
    }
}

fn rendered(node: &NodeRef<'_>) -> String {
    let mut out = String::new();
    write_node(&mut out, node, 0);
    out
}

fn visit<'a>(node: NodeRef<'a>, seen: &mut Vec<NodeRef<'a>>) {
    seen.push(node);
    for child in node.filtered_children(&common_filter) {
        visit(child, seen);
    }
}

impl<'a> View<'a> {
    pub(crate) const fn new(state: &'a TreeState) -> Self {
        Self { state }
    }

    #[must_use]
    pub fn render(self) -> String {
        rendered(&self.state.root())
    }

    #[must_use]
    pub fn part(self, role: Role, label: Option<&str>) -> Option<String> {
        self.nodes()
            .into_iter()
            .find(|node| {
                node.role() == role
                    && label.is_none_or(|label| node.label().as_deref() == Some(label))
            })
            .map(|node| rendered(&node))
    }

    #[must_use]
    pub fn part_showing(self, role: Role, text: &str) -> Option<String> {
        self.nodes()
            .into_iter()
            .filter(|node| node.role() == role)
            .map(|node| rendered(&node))
            .find(|part| part.contains(text))
    }

    fn nodes(self) -> Vec<NodeRef<'a>> {
        let mut seen = Vec::new();
        visit(self.state.root(), &mut seen);
        seen
    }

    #[must_use]
    pub fn all(self, role: Role) -> Vec<Found> {
        self.nodes()
            .iter()
            .filter(|node| node.role() == role)
            .map(found)
            .collect()
    }

    #[must_use]
    pub fn find(self, role: Role, label: &str) -> Option<Found> {
        self.nodes()
            .iter()
            .find(|node| node.role() == role && node.label().as_deref() == Some(label))
            .map(found)
    }

    #[must_use]
    pub fn shows(self, text: &str) -> bool {
        self.nodes().iter().any(|node| {
            node.label().is_some_and(|label| label.contains(text))
                || node.value().is_some_and(|value| value.contains(text))
        })
    }

    #[must_use]
    pub fn focused(self) -> Option<Found> {
        self.state.focus().map(|node| found(&node))
    }
}

impl std::fmt::Debug for View<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&View::render(*self))
    }
}
