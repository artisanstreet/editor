//! Dependency-free policy for revealing streamed Markdown words.
//!
//! The browser implementation keeps transport targets, visible text, pacing,
//! and the small AST rewrite in one component. This module owns only the
//! deterministic parts of that behavior. It deliberately has no parser,
//! renderer, timer, queue, or executor dependency; callers supply the event
//! that won a delay/target race and adapt [`StreamingNode`] to their Markdown
//! representation.

/// The latest text payload considered by the reveal policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamingWordsTarget {
    /// Complete text received from the transport.
    pub text: String,
    /// Whether the transport may append more text to this target.
    pub streaming: bool,
}

impl StreamingWordsTarget {
    /// Creates a target from owned or borrowed text.
    #[must_use]
    pub fn new(text: impl Into<String>, streaming: bool) -> Self {
        Self {
            text: text.into(),
            streaming,
        }
    }
}

/// The typed winner of a delay-versus-target race.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamingWordDelayOutcome {
    /// The animation hold elapsed without a newer target winning first.
    Elapsed,
    /// A newer transport target won before the delay elapsed.
    Target {
        /// The losslessly queued target that interrupted the hold.
        target: StreamingWordsTarget,
    },
}

/// An observed event supplied to [`decide_streaming_word_delay_or_target`].
///
/// The native caller owns the timer and queue. Representing the observed
/// winner as a value keeps this module synchronous and makes both branches
/// testable without smuggling a runtime into the Markdown policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamingWordDelayEvent {
    /// The requested delay elapsed first.
    Elapsed,
    /// A newer target arrived first.
    Target {
        /// The target that arrived first.
        target: StreamingWordsTarget,
    },
}

/// Converts an observed delay race winner into the public typed outcome.
#[must_use]
pub fn decide_streaming_word_delay_or_target(
    event: StreamingWordDelayEvent,
) -> StreamingWordDelayOutcome {
    match event {
        StreamingWordDelayEvent::Elapsed => StreamingWordDelayOutcome::Elapsed,
        StreamingWordDelayEvent::Target { target } => StreamingWordDelayOutcome::Target { target },
    }
}

/// One reveal tick's delay and word allowance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamingWordPacing {
    /// Delay before the next reveal tick, in milliseconds.
    pub delay_ms: u32,
    /// Maximum number of visual words committed by the tick.
    pub words: usize,
}

const CALM_PACING: StreamingWordPacing = StreamingWordPacing {
    delay_ms: 40,
    words: 1,
};
const MEDIUM_PACING: StreamingWordPacing = StreamingWordPacing {
    delay_ms: 28,
    words: 1,
};
const FAST_PACING: StreamingWordPacing = StreamingWordPacing {
    delay_ms: 20,
    words: 2,
};
const CATCH_UP_PACING: StreamingWordPacing = StreamingWordPacing {
    delay_ms: 16,
    words: 4,
};
const DRAIN_PACING: StreamingWordPacing = StreamingWordPacing {
    delay_ms: 16,
    words: 8,
};

/// Whether a target can extend the currently visible prefix without
/// replaying or discarding a correction.
#[must_use]
pub fn is_append_only_streaming_target(
    current_prefix: &str,
    target: &StreamingWordsTarget,
) -> bool {
    target.text.starts_with(current_prefix)
}

/// Whether a target remains eligible for word animation.
///
/// A live target always animates. A settled target may finish draining a
/// presentation that was not settled yet, but two already-settled snapshots
/// replace each other immediately at the call site.
#[must_use]
pub fn should_animate_streaming_target(
    presentation_settled: bool,
    target: &StreamingWordsTarget,
) -> bool {
    target.streaming || !presentation_settled
}

/// Returns the next UTF-8 byte boundary that a reveal tick may expose.
///
/// Streaming targets retain a final unterminated non-whitespace word. Settled
/// targets expose that final word too. The returned offset is always a valid
/// Rust `str` boundary for an append-only target; it is the target length for
/// a replacement.
#[must_use]
pub fn find_next_reveal_boundary(current_prefix: &str, target: &StreamingWordsTarget) -> usize {
    if !is_append_only_streaming_target(current_prefix, target) {
        return target.text.len();
    }

    let safe_end = if target.streaming {
        trailing_word_start(&target.text).unwrap_or(target.text.len())
    } else {
        target.text.len()
    };

    if current_prefix.len() >= safe_end {
        return current_prefix.len();
    }

    let current_word_start = current_word_start(current_prefix);
    let Some((next_whitespace, _)) = target.text[current_word_start..]
        .char_indices()
        .find(|(_, character)| is_streaming_whitespace(*character))
    else {
        return safe_end;
    };

    let mut boundary = current_word_start + next_whitespace;
    while boundary < safe_end {
        let Some(character) = target.text[boundary..].chars().next() else {
            break;
        };
        if !is_streaming_whitespace(character) {
            break;
        }
        boundary += character.len_utf8();
    }
    boundary
}

/// Returns the exact target prefix permitted by the next queue tick.
#[must_use]
pub fn reveal_streaming_words(
    current_prefix: &str,
    target: &StreamingWordsTarget,
    words: usize,
) -> String {
    let mut revealed = current_prefix.to_owned();

    for _ in 0..words {
        let boundary = find_next_reveal_boundary(&revealed, target);
        let next = &target.text[..boundary];
        if next == revealed {
            break;
        }
        next.clone_into(&mut revealed);
    }

    revealed
}

/// Counts pending Unicode non-whitespace runs for pacing selection.
#[must_use]
pub fn count_pending_streaming_words(current_prefix: &str, target: &StreamingWordsTarget) -> usize {
    if !is_append_only_streaming_target(current_prefix, target) {
        return 0;
    }

    target.text[current_prefix.len()..]
        .split(is_streaming_whitespace)
        .filter(|word| !word.is_empty())
        .count()
}

/// Selects the exact legacy pacing tier for a pending-word backlog.
#[must_use]
pub fn get_streaming_word_pacing(backlog_words: usize) -> StreamingWordPacing {
    if backlog_words <= 4 {
        CALM_PACING
    } else if backlog_words <= 12 {
        MEDIUM_PACING
    } else if backlog_words <= 32 {
        FAST_PACING
    } else if backlog_words <= 96 {
        CATCH_UP_PACING
    } else {
        DRAIN_PACING
    }
}

/// The smallest owned tree shape needed by the streaming-word rewrite.
///
/// `Word` is the native representation of the generated `stream-word`
/// element. Incoming parser nodes that are already `Word`s are opaque to a
/// subsequent rewrite, just as the legacy `stream-word` subtree is excluded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamingNode {
    /// Raw text that has not been split into visual words yet.
    Text(String),
    /// A parser element whose tag and child structure matter to wrapping.
    Element {
        /// Element tag used for the excluded-subtree policy.
        tag: String,
        /// Child nodes in parser order.
        children: Vec<Self>,
    },
    /// A generated or previously wrapped visual word.
    Word {
        /// Word text without surrounding whitespace.
        text: String,
        /// Whether this word is the one selected for the entrance animation.
        incoming: bool,
    },
}

impl StreamingNode {
    /// Creates a raw text node.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// Creates an element node.
    #[must_use]
    pub fn element(tag: impl Into<String>, children: Vec<Self>) -> Self {
        Self::Element {
            tag: tag.into(),
            children,
        }
    }

    /// Creates a non-incoming generated word.
    #[must_use]
    pub fn word(text: impl Into<String>) -> Self {
        Self::Word {
            text: text.into(),
            incoming: false,
        }
    }

    /// Creates a generated word with an explicit animation marker.
    #[must_use]
    pub fn word_with_incoming(text: impl Into<String>, incoming: bool) -> Self {
        Self::Word {
            text: text.into(),
            incoming,
        }
    }
}

/// Whether a generation should select the latest newly wrapped word.
#[must_use]
pub fn should_animate_latest_word(
    animation_generation: Option<u64>,
    consumed_generation: Option<u64>,
) -> bool {
    animation_generation.is_some() && animation_generation != consumed_generation
}

/// Pure result of selecting a generation's latest-word animation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamingWordAnimationSelection {
    /// Whether the current rewrite should mark its latest new word incoming.
    pub animate_latest_word: bool,
    /// The generation state after consuming the current selection.
    pub consumed_generation: Option<u64>,
}

/// Selects and consumes a single animation generation without owning parser
/// or renderer state.
#[must_use]
pub fn select_latest_word_animation(
    animation_generation: Option<u64>,
    consumed_generation: Option<u64>,
) -> StreamingWordAnimationSelection {
    let animate_latest_word = should_animate_latest_word(animation_generation, consumed_generation);
    StreamingWordAnimationSelection {
        animate_latest_word,
        consumed_generation: if animate_latest_word {
            animation_generation
        } else {
            consumed_generation
        },
    }
}

/// Wraps normal text nodes into words while preserving parser structure.
///
/// Whitespace remains raw [`StreamingNode::Text`] data. The last word created
/// outside an excluded subtree is marked `incoming` only when requested;
/// earlier words and every excluded subtree remain unmarked/unchanged.
#[must_use]
pub fn wrap_streaming_words(
    nodes: &[StreamingNode],
    animate_latest_word: bool,
) -> Vec<StreamingNode> {
    let mut wrapped_nodes = Vec::new();
    let mut last_word_path: Option<Vec<usize>> = None;

    for node in nodes {
        let start_path = vec![wrapped_nodes.len()];
        wrapped_nodes.extend(wrap_streaming_node(node, &start_path, &mut last_word_path));
    }

    if animate_latest_word && let Some(path) = last_word_path {
        mark_word_incoming(&mut wrapped_nodes, &path);
    }

    wrapped_nodes
}

/// Rewrites only the un-reused tail of a parsed node list.
///
/// This mirrors the legacy plugin's `reusableNodes` contract: the prefix is
/// copied structurally without wrapping or replaying its latest word, while
/// the tail gets the ordinary deterministic rewrite.
#[must_use]
pub fn wrap_streaming_words_with_reused_prefix(
    nodes: &[StreamingNode],
    reused_nodes: usize,
    animate_latest_word: bool,
) -> Vec<StreamingNode> {
    let split = reused_nodes.min(nodes.len());
    let mut result = nodes[..split].to_vec();
    result.extend(wrap_streaming_words(&nodes[split..], animate_latest_word));
    result
}

fn wrap_streaming_node(
    node: &StreamingNode,
    start_path: &[usize],
    last_word_path: &mut Option<Vec<usize>>,
) -> Vec<StreamingNode> {
    match node {
        StreamingNode::Text(text) => wrap_streaming_text(text, start_path, last_word_path),
        StreamingNode::Word { .. } => vec![node.clone()],
        StreamingNode::Element { tag, children } => {
            if is_excluded_streaming_subtree(tag) {
                return vec![node.clone()];
            }

            let mut wrapped_children = Vec::new();
            for child in children {
                let mut child_path = start_path.to_vec();
                child_path.push(wrapped_children.len());
                wrapped_children.extend(wrap_streaming_node(child, &child_path, last_word_path));
            }
            vec![StreamingNode::Element {
                tag: tag.clone(),
                children: wrapped_children,
            }]
        }
    }
}

fn wrap_streaming_text(
    text: &str,
    start_path: &[usize],
    last_word_path: &mut Option<Vec<usize>>,
) -> Vec<StreamingNode> {
    let mut nodes = Vec::new();
    if text.is_empty() {
        return nodes;
    }

    let mut segment_start = 0;
    let mut segment_is_whitespace = None;
    for (index, character) in text.char_indices() {
        let is_whitespace = is_streaming_whitespace(character);
        if let Some(previous) = segment_is_whitespace
            && previous != is_whitespace
        {
            push_text_segment(
                &mut nodes,
                &text[segment_start..index],
                previous,
                start_path,
                last_word_path,
            );
            segment_start = index;
        }
        segment_is_whitespace = Some(is_whitespace);
    }

    push_text_segment(
        &mut nodes,
        &text[segment_start..],
        segment_is_whitespace.expect("non-empty text has a first character"),
        start_path,
        last_word_path,
    );
    nodes
}

fn push_text_segment(
    nodes: &mut Vec<StreamingNode>,
    segment: &str,
    is_whitespace: bool,
    start_path: &[usize],
    last_word_path: &mut Option<Vec<usize>>,
) {
    if segment.is_empty() {
        return;
    }

    let output_index = nodes.len();
    if is_whitespace {
        nodes.push(StreamingNode::Text(segment.to_owned()));
        return;
    }

    nodes.push(StreamingNode::word(segment));
    let mut path = start_path.to_vec();
    let final_index = path
        .last_mut()
        .expect("a wrapped node always has a path")
        .checked_add(output_index)
        .expect("node path index overflow");
    *path.last_mut().expect("a wrapped node always has a path") = final_index;
    *last_word_path = Some(path);
}

fn mark_word_incoming(nodes: &mut [StreamingNode], path: &[usize]) {
    let Some(first) = path.first() else {
        return;
    };
    let Some(mut node) = nodes.get_mut(*first) else {
        return;
    };

    for index in &path[1..] {
        let StreamingNode::Element { children, .. } = node else {
            return;
        };
        let Some(next) = children.get_mut(*index) else {
            return;
        };
        node = next;
    }

    if let StreamingNode::Word { incoming, .. } = node {
        *incoming = true;
    }
}

fn trailing_word_start(text: &str) -> Option<usize> {
    let mut characters = text.char_indices().rev();
    let (last_index, last_character) = characters.next()?;
    if is_streaming_whitespace(last_character) {
        return None;
    }

    let mut start = last_index;
    for (index, character) in characters {
        if is_streaming_whitespace(character) {
            break;
        }
        start = index;
    }
    Some(start)
}

fn current_word_start(prefix: &str) -> usize {
    let mut start = prefix.len();
    for (index, character) in prefix.char_indices().rev() {
        if is_streaming_whitespace(character) {
            break;
        }
        start = index;
    }
    start
}

/// JavaScript's Unicode `/\s/u` set used by the legacy policy.
///
/// `char::is_whitespace` is intentionally not used: Rust additionally treats
/// several control characters (including U+0085) as whitespace, while the
/// ECMAScript expression treats U+FEFF as whitespace.
fn is_streaming_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'..='\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

fn is_excluded_streaming_subtree(tag: &str) -> bool {
    matches!(
        tag.to_ascii_lowercase().as_str(),
        "code" | "math" | "mermaid" | "pre" | "stream-word"
    )
}
