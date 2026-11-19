use super::RootViews;
use crate::conversation::controller::Conversation;
use crate::conversation::viewport::Jump;
use crate::shell::host::Shell;
use crate::sidebar::list::HitRow;
use aqua::UiApp;
use aqua_core::NodeId;
use aqua_ui::tree::NodeKind;
use icu_casemap::CaseMapper;
use icu_normalizer::DecomposingNormalizerBorrowed;
use icu_normalizer::properties::CanonicalCombiningClassMapBorrowed;
use std::time::{Duration, Instant};

const PATIENCE: Duration = Duration::from_secs(8);
const DEEPEN_AFTER: Duration = Duration::from_millis(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Progress {
    Idle,
    Waiting,
    Acted,
}

pub(super) const POLL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Opening,
    Jumped(Instant),
    Deepened,
}

pub(super) struct Reveal {
    hit: HitRow,
    needle: String,
    stage: Stage,
    until: Instant,
}

impl Reveal {
    pub(super) fn new(hit: HitRow) -> Self {
        let needle = fold(&hit.needle);
        Self {
            hit,
            needle,
            stage: Stage::Opening,
            until: Instant::now() + PATIENCE,
        }
    }
}

fn fold(text: &str) -> String {
    let folded = CaseMapper::new().fold_string(text);
    DecomposingNormalizerBorrowed::new_nfd()
        .normalize(&folded)
        .into_owned()
}

pub fn find_ignoring_case(text: &str, needle: &str) -> Option<std::ops::Range<usize>> {
    find_folded(text, &fold(needle))
}

struct Folded {
    text: String,
    clusters: Vec<(usize, usize)>,
}

impl Folded {
    fn new(text: &str) -> Self {
        let classes = CanonicalCombiningClassMapBorrowed::new();
        let mut starts: Vec<usize> = text
            .char_indices()
            .filter(|(offset, character)| *offset == 0 || classes.get_u8(*character) == 0)
            .map(|(offset, _)| offset)
            .collect();
        starts.push(text.len());
        let mut folded = String::with_capacity(text.len());
        let mut clusters = Vec::with_capacity(starts.len());
        for pair in starts.windows(2) {
            let [start, end] = *pair else {
                unreachable!("invariant: windows(2) yields pairs");
            };
            clusters.push((folded.len(), start));
            folded.push_str(&fold(
                text.get(start..end)
                    .expect("invariant: cluster starts fall on char boundaries"),
            ));
        }
        clusters.push((folded.len(), text.len()));
        Self {
            text: folded,
            clusters,
        }
    }

    fn original(&self, folded: usize) -> Option<usize> {
        match self.clusters.binary_search_by_key(&folded, |(at, _)| *at) {
            Ok(index) => self.clusters.get(index).map(|(_, original)| *original),
            Err(_inside_a_cluster) => None,
        }
    }
}

fn find_folded(text: &str, needle: &str) -> Option<std::ops::Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let folded = Folded::new(text);
    folded.clusters.iter().find_map(|(at, start)| {
        folded
            .text
            .get(*at..)
            .filter(|rest| rest.starts_with(needle))
            .and_then(|_| folded.original(at + needle.len()))
            .map(|end| *start..end)
    })
}

pub fn estimated_row(rows: &[usize], depth: (u32, u32)) -> Option<usize> {
    let last = rows.len().checked_sub(1)?;
    let share = f64::from(depth.0) / f64::from(depth.1.max(1));
    let index =
        crate::measure::saturate_row((share.clamp(0.0, 1.0) * crate::measure::index(last)).round());
    rows.get(index.min(last)).copied()
}

fn item_rows(conversation: &Conversation, hit: &HitRow) -> Vec<usize> {
    conversation
        .rows
        .iter()
        .enumerate()
        .filter(|(_, (row, _))| row.anchor() == Some(&hit.item))
        .map(|(index, _)| index)
        .collect()
}

fn shown_match(
    ui: &UiApp,
    conversation: &Conversation,
    hit: &HitRow,
    needle: &str,
) -> Option<(NodeId, std::ops::Range<usize>)> {
    item_rows(conversation, hit).into_iter().find_map(|index| {
        let key = crate::conversation::view::transcript::row_key_hash(
            &conversation.rows.get(index)?.0.key(),
        );
        let mut pending = vec![ui.tree().sole_node_with_key(key)?];
        while let Some(node) = pending.pop() {
            let record = ui.tree().get(node)?;
            pending.extend(record.children.iter().rev().copied());
            if let NodeKind::Text(text) = &record.kind
                && let Some(found) = find_folded(text.text(), needle)
            {
                return Some((node, found));
            }
        }
        None
    })
}

impl RootViews {
    pub(super) fn advance_reveal(&mut self, shell: &mut Shell, ui: &mut UiApp) -> Progress {
        let Some(reveal) = self.reveal.as_mut() else {
            return Progress::Idle;
        };
        let now = Instant::now();
        if now >= reveal.until {
            tracing::info!(
                thread = %reveal.hit.thread,
                item = %reveal.hit.item,
                "gave up selecting a search hit in the transcript"
            );
            self.reveal = None;
            return Progress::Idle;
        }
        let core = &shell.core;
        let Some(workspace) = shell.entry.workspace_mut() else {
            return Progress::Waiting;
        };
        let conversation = &mut workspace.conversation;
        let ready = conversation.agent.is_none()
            && !conversation.is_landing()
            && conversation
                .open
                .as_ref()
                .is_some_and(|(_, thread)| thread == &reveal.hit.thread)
            && conversation
                .view
                .as_ref()
                .is_some_and(|view| view.viewport.loaded());
        if !ready {
            return Progress::Waiting;
        }
        match reveal.stage {
            Stage::Opening => {
                conversation.jump(core, Jump::To(reveal.hit.position));
                reveal.stage = Stage::Jumped(now);
                return Progress::Acted;
            }
            Stage::Jumped(_) | Stage::Deepened => {}
        }
        if let Some((node, range)) = shown_match(ui, conversation, &reveal.hit, &reveal.needle) {
            return if ui.select_text(node, range) {
                self.reveal = None;
                Progress::Acted
            } else {
                Progress::Waiting
            };
        }
        if let Stage::Jumped(at) = reveal.stage
            && now.saturating_duration_since(at) >= DEEPEN_AFTER
        {
            let rows = item_rows(conversation, &reveal.hit);
            reveal.stage = Stage::Deepened;
            if let Some(row) = estimated_row(&rows, reveal.hit.depth).filter(|_| rows.len() > 1) {
                conversation.reveal_row(row);
                return Progress::Acted;
            }
        }
        Progress::Waiting
    }
}
