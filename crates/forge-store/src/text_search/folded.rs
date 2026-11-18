use icu_casemap::CaseMapper;
use icu_normalizer::DecomposingNormalizerBorrowed;
use icu_normalizer::properties::CanonicalCombiningClassMapBorrowed;
use std::ops::Range;

pub(crate) fn needle(text: &str) -> String {
    let folded = CaseMapper::new().fold_string(text);
    DecomposingNormalizerBorrowed::new_nfd()
        .normalize(&folded)
        .into_owned()
}

pub(crate) fn text(text: &str) -> String {
    fold_clusters(text, |_, _| ())
}

fn fold_clusters(text: &str, mut mark: impl FnMut(usize, usize)) -> String {
    if text.is_ascii() {
        (0..text.len()).for_each(|at| mark(at, at));
        return text.to_ascii_lowercase();
    }
    let classes = CanonicalCombiningClassMapBorrowed::new();
    let mut starts = text
        .char_indices()
        .filter(|(offset, character)| *offset == 0 || classes.get_u8(*character) == 0)
        .map(|(offset, _)| offset)
        .peekable();
    let mut folded = String::with_capacity(text.len());
    while let Some(start) = starts.next() {
        let end = starts.peek().copied().unwrap_or(text.len());
        mark(folded.len(), start);
        match text
            .get(start..end)
            .expect("invariant: clusters start on character boundaries")
        {
            ascii if ascii.len() == 1 => folded.push_str(&ascii.to_ascii_lowercase()),
            cluster => folded.push_str(&needle(cluster)),
        }
    }
    folded
}

#[derive(Debug)]
pub(crate) struct Folded {
    text: String,
    clusters: Vec<(usize, usize)>,
}

impl Folded {
    pub(crate) fn new(original: &str) -> Self {
        let mut clusters = Vec::new();
        let text = fold_clusters(original, |folded, start| clusters.push((folded, start)));
        clusters.push((text.len(), original.len()));
        Self { text, clusters }
    }

    fn original(&self, folded: usize) -> Option<usize> {
        self.clusters
            .binary_search_by_key(&folded, |(at, _)| *at)
            .ok()
            .and_then(|index| self.clusters.get(index))
            .map(|(_, original)| *original)
    }

    pub(crate) fn find(&self, needle: &str) -> Option<Range<usize>> {
        if needle.is_empty() {
            return None;
        }
        let haystack = self.text.as_bytes();
        let finder = memchr::memmem::Finder::new(needle.as_bytes());
        let mut from = 0;
        while let Some(found) = haystack.get(from..).and_then(|rest| finder.find(rest)) {
            let at = from + found;
            if let (Some(start), Some(end)) = (self.original(at), self.original(at + needle.len()))
            {
                return Some(start..end);
            }
            from = at + 1;
        }
        None
    }
}
