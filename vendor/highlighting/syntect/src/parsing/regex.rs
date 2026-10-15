use once_cell::sync::OnceCell;
use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};
use std::error::Error;

/// An abstraction for regex patterns.
///
/// * Allows swapping out the regex implementation because it's only in this module.
/// * Makes regexes serializable and deserializable using just the pattern string.
/// * Lazily compiles regexes on first use to improve initialization time.
#[derive(Debug)]
pub struct Regex {
    regex_str: String,
    regex: OnceCell<regex_impl::Regex>,
}

/// A region contains text positions for capture groups in a match result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Region {
    region: regex_impl::Region,
}

impl Regex {
    /// Create a new regex from the pattern string.
    ///
    /// Note that the regex compilation happens on first use, which is why this method does not
    /// return a result.
    pub fn new(regex_str: String) -> Self {
        Self {
            regex_str,
            regex: OnceCell::new(),
        }
    }

    /// Check whether the pattern compiles as a valid regex or not.
    pub fn try_compile(regex_str: &str) -> Option<Box<dyn Error + Send + Sync + 'static>> {
        regex_impl::Regex::new(regex_str).err()
    }

    /// Return the regex pattern.
    pub fn regex_str(&self) -> &str {
        &self.regex_str
    }

    /// Check if the regex matches the given text.
    pub fn is_match(&self, text: &str) -> bool {
        self.regex().is_match(text)
    }

    /// Search for the pattern in the given text from begin/end positions.
    ///
    /// If a region is passed, it is used for storing match group positions. The argument allows
    /// the [`Region`] to be reused between searches, which makes a significant performance
    /// difference.
    ///
    /// [`Region`]: struct.Region.html
    pub fn search(
        &self,
        text: &str,
        begin: usize,
        end: usize,
        region: Option<&mut Region>,
    ) -> bool {
        self.regex()
            .search(text, begin, end, region.map(|r| &mut r.region))
    }

    fn regex(&self) -> &regex_impl::Regex {
        self.regex.get_or_init(|| {
            regex_impl::Regex::new(&self.regex_str).expect("regex string should be pre-tested")
        })
    }
}

impl Clone for Regex {
    fn clone(&self) -> Self {
        Regex {
            regex_str: self.regex_str.clone(),
            regex: OnceCell::new(),
        }
    }
}

impl PartialEq for Regex {
    fn eq(&self, other: &Regex) -> bool {
        self.regex_str == other.regex_str
    }
}

impl Eq for Regex {}

impl Serialize for Regex {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.regex_str)
    }
}

impl<'de> Deserialize<'de> for Regex {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let regex_str = String::deserialize(deserializer)?;
        Ok(Regex::new(regex_str))
    }
}

impl Region {
    pub fn new() -> Self {
        Self {
            region: regex_impl::new_region(),
        }
    }

    /// Get the start/end positions of the capture group with given index.
    ///
    /// If there is no match for that group or the index does not correspond to a group, `None` is
    /// returned. The index 0 returns the whole match.
    pub fn pos(&self, index: usize) -> Option<(usize, usize)> {
        self.region.pos(index)
    }
}

impl Default for Region {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "regex-onig")]
mod regex_impl {
    use once_cell::sync::OnceCell;
    use onig::{MatchParam, RegexOptions, SearchOptions, Syntax};
    use std::error::Error;

    // This is a search-work budget, not a claim of equivalence to fancy's VM counter.
    // Reaching it delegates to the original engine with its unchanged default budget.
    const SEARCH_RETRY_LIMIT: u32 = 1_000_000;
    const MATCH_STACK_LIMIT_OVER: i32 = -15;
    const RETRY_LIMIT_IN_MATCH_OVER: i32 = -17;
    const RETRY_LIMIT_IN_SEARCH_OVER: i32 = -18;

    #[derive(Debug)]
    pub struct Regex {
        regex: onig::Regex,
        pattern: String,
        fallback: OnceCell<Box<fancy_regex::Regex>>,
        search_retry_limit: u32,
    }

    #[derive(Clone, Debug)]
    pub struct Region {
        native: onig::Region,
        fallback: Vec<Option<(usize, usize)>>,
        using_fallback: bool,
    }

    impl Region {
        pub fn pos(&self, index: usize) -> Option<(usize, usize)> {
            if self.using_fallback {
                self.fallback.get(index).copied().flatten()
            } else {
                self.native.pos(index)
            }
        }

        fn len(&self) -> usize {
            if self.using_fallback {
                self.fallback.len()
            } else {
                self.native.len()
            }
        }

        fn clear(&mut self) {
            self.native.clear();
            self.fallback.clear();
        }
    }

    impl PartialEq for Region {
        fn eq(&self, other: &Self) -> bool {
            self.len() == other.len() && (0..self.len()).all(|i| self.pos(i) == other.pos(i))
        }
    }
    impl Eq for Region {}

    pub fn new_region() -> Region {
        Region {
            native: onig::Region::with_capacity(8),
            fallback: Vec::new(),
            using_fallback: false,
        }
    }

    impl Regex {
        pub fn new(regex_str: &str) -> Result<Regex, Box<dyn Error + Send + Sync + 'static>> {
            let regex = onig::Regex::with_options(
                regex_str,
                RegexOptions::REGEX_OPTION_CAPTURE_GROUP,
                Syntax::default(),
            )?;
            Ok(Regex {
                regex,
                pattern: regex_str.to_owned(),
                fallback: OnceCell::new(),
                search_retry_limit: SEARCH_RETRY_LIMIT,
            })
        }

        fn fallback(&self) -> &fancy_regex::Regex {
            self.fallback.get_or_init(|| {
                Box::new(
                    fancy_regex::RegexBuilder::new(&self.pattern)
                        .oniguruma_mode(true)
                        .build()
                        .expect("regex string should be pre-tested"),
                )
            })
        }

        pub fn is_match(&self, text: &str) -> bool {
            // Preserve the original fancy backend's unanchored metadata search.
            self.fallback().is_match(text).unwrap_or(false)
        }

        fn search_fallback(
            &self,
            regex: &fancy_regex::Regex,
            text: &str,
            begin: usize,
            end: usize,
            mut region: Option<&mut Region>,
        ) -> bool {
            if let Some(region) = region.as_mut() {
                region.using_fallback = true;
                region.clear();
            }
            if let Ok(Some(captures)) = regex.captures_from_pos(&text[..end], begin) {
                if let Some(region) = region {
                    region
                        .fallback
                        .extend(captures.iter().map(|m| m.map(|m| (m.start(), m.end()))));
                }
                true
            } else {
                false
            }
        }

        pub fn search(
            &self,
            text: &str,
            begin: usize,
            end: usize,
            mut region: Option<&mut Region>,
        ) -> bool {
            // Onig forms a raw pointer before validating its start offset. Keep
            // invalid ranges on the safe Rust side, as in the original backend.
            assert!(begin <= end, "search start must not exceed end");
            if let Some(regex) = self.fallback.get() {
                return self.search_fallback(regex, text, begin, end, region);
            }
            if let Some(region) = region.as_mut() {
                region.using_fallback = false;
            }
            let mut param = MatchParam::default();
            if param
                .set_retry_limit_in_search(self.search_retry_limit)
                .is_err()
            {
                // A system Oniguruma build may omit retry support. Keep the
                // original bounded engine instead of silently searching without a limit.
                return self.search_fallback(self.fallback(), text, begin, end, region);
            }
            let matched = self.regex.search_with_param(
                &text[..end],
                begin,
                end,
                SearchOptions::SEARCH_OPTION_NONE,
                region.as_mut().map(|r| &mut r.native),
                param,
            );
            match matched {
                Ok(found) => found.is_some(),
                Err(error)
                    if matches!(
                        error.code(),
                        MATCH_STACK_LIMIT_OVER
                            | RETRY_LIMIT_IN_MATCH_OVER
                            | RETRY_LIMIT_IN_SEARCH_OVER
                    ) =>
                {
                    self.search_fallback(self.fallback(), text, begin, end, region)
                }
                Err(_) => {
                    if let Some(region) = region {
                        region.clear();
                    }
                    false
                }
            }
        }
    }

    #[cfg(test)]
    mod bounded_tests {
        use super::*;

        fn original(
            pattern: &str,
            text: &str,
            begin: usize,
            end: usize,
        ) -> Option<Vec<Option<(usize, usize)>>> {
            fancy_regex::RegexBuilder::new(pattern)
                .oniguruma_mode(true)
                .build()
                .unwrap()
                .captures_from_pos(&text[..end], begin)
                .unwrap()
                .map(|caps| {
                    caps.iter()
                        .map(|m| m.map(|m| (m.start(), m.end())))
                        .collect()
                })
        }

        #[test]
        fn limit_fallback_preserves_captures_and_is_reused() {
            let pattern = r"(a|ab)+(😀)?(z)?";
            let mut regex = Regex::new(pattern).unwrap();
            regex.search_retry_limit = 1;
            let text = "abab😀";
            let mut region = new_region();
            assert!(regex.search(text, 0, text.len(), Some(&mut region)));
            assert!(regex.fallback.get().is_some());
            let expected = original(pattern, text, 0, text.len()).unwrap();
            assert_eq!(region.len(), expected.len());
            for (i, pos) in expected.into_iter().enumerate() {
                assert_eq!(region.pos(i), pos);
            }
            let compiled = regex.fallback.get().unwrap() as *const _;
            assert!(regex.search("abz", 0, 3, Some(&mut region)));
            assert_eq!(compiled, regex.fallback.get().unwrap() as *const _);
            assert!(!regex.search("!", 0, 1, Some(&mut region)));
            assert_eq!(region.pos(0), None);
        }

        #[test]
        fn native_and_fallback_preserve_truncation_and_unicode_offsets() {
            for (pattern, text, begin, end) in [
                ("a$", "ab", 0, 1),
                ("a(?=b)", "ab", 0, 1),
                (r"(?<=😀)(a*)(b)?", "😀aa!", 4, 6),
                (r"()$", "😀a!", 4, 5),
                (r"^(a*)", "😀a!", 4, 5),
            ] {
                let regex = Regex::new(pattern).unwrap();
                let expected = original(pattern, text, begin, end);
                let mut native = new_region();
                assert_eq!(
                    regex.search(text, begin, end, Some(&mut native)),
                    expected.is_some(),
                    "native {pattern}"
                );
                if let Some(ref positions) = expected {
                    for (i, pos) in positions.iter().enumerate() {
                        assert_eq!(native.pos(i), *pos, "native {pattern} group{i}");
                    }
                }
                regex.fallback();
                let mut fallback = new_region();
                assert_eq!(
                    regex.search(text, begin, end, Some(&mut fallback)),
                    expected.is_some(),
                    "fallback {pattern}"
                );
                if let Some(ref positions) = expected {
                    for (i, pos) in positions.iter().enumerate() {
                        assert_eq!(fallback.pos(i), *pos, "fallback {pattern} group{i}");
                    }
                    assert_eq!(native, fallback);
                }
            }
        }

        #[test]
        fn invalid_ranges_panic_before_native_search_and_match_original_behavior() {
            for force_fallback in [false, true] {
                let regex = Regex::new("a").unwrap();
                if force_fallback {
                    regex.fallback();
                }
                // The first case would make Onig's pointer addition invalid.
                // The others keep end-range and UTF-8 checks in Rust slicing.
                for (text, begin, end) in [("a", usize::MAX, 1), ("a", 0, 2), ("😀a", 0, 1)] {
                    let original = std::panic::catch_unwind(|| original("a", text, begin, end));
                    assert!(original.is_err());
                    let bounded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        regex.search(text, begin, end, None)
                    }));
                    assert!(
                        bounded.is_err(),
                        "fallback={force_fallback}, {begin}..{end}"
                    );
                }
            }
        }

        #[test]
        fn metadata_is_match_keeps_unanchored_search() {
            let regex = Regex::new("needle").unwrap();
            assert!(regex.is_match("prefix needle suffix"));
            assert!(!regex.is_match("prefix suffix"));
        }

        #[test]
        fn fallback_remains_safe_for_concurrent_searches() {
            let mut regex = Regex::new(r"(a|ab)+(😀)?(z)?").unwrap();
            regex.search_retry_limit = 1;
            let regex = std::sync::Arc::new(regex);
            let threads: Vec<_> = (0..4)
                .map(|_| {
                    let regex = regex.clone();
                    std::thread::spawn(move || {
                        let mut region = new_region();
                        for _ in 0..32 {
                            assert!(regex.search("abab😀", 0, 8, Some(&mut region)));
                            assert_eq!(region.pos(0), Some((0, 1)));
                        }
                    })
                })
                .collect();
            for thread in threads {
                thread.join().unwrap();
            }
            assert!(regex.fallback.get().is_some());
        }
    }
}

// If both regex-fancy and regex-onig are requested, this condition makes regex-onig win.
#[cfg(all(feature = "regex-fancy", not(feature = "regex-onig")))]
mod regex_impl {
    use std::error::Error;

    #[derive(Debug)]
    pub struct Regex {
        regex: fancy_regex::Regex,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Region {
        positions: Vec<Option<(usize, usize)>>,
    }

    pub fn new_region() -> Region {
        Region {
            positions: Vec::with_capacity(8),
        }
    }

    impl Regex {
        pub fn new(regex_str: &str) -> Result<Regex, Box<dyn Error + Send + Sync + 'static>> {
            let result = fancy_regex::RegexBuilder::new(regex_str)
                .oniguruma_mode(true)
                .build();
            match result {
                Ok(regex) => Ok(Regex { regex }),
                Err(error) => Err(Box::new(error)),
            }
        }

        pub fn is_match(&self, text: &str) -> bool {
            // Errors are treated as non-matches
            self.regex.is_match(text).unwrap_or(false)
        }

        pub fn search(
            &self,
            text: &str,
            begin: usize,
            end: usize,
            region: Option<&mut Region>,
        ) -> bool {
            // If there's an error during search, treat it as non-matching.
            // For example, in case of catastrophic backtracking, fancy-regex should
            // fail with an error eventually.
            if let Ok(Some(captures)) = self.regex.captures_from_pos(&text[..end], begin) {
                if let Some(region) = region {
                    region.init_from_captures(&captures);
                }
                true
            } else {
                false
            }
        }
    }

    impl Region {
        fn init_from_captures(&mut self, captures: &fancy_regex::Captures) {
            self.positions.clear();
            for i in 0..captures.len() {
                let pos = captures.get(i).map(|m| (m.start(), m.end()));
                self.positions.push(pos);
            }
        }

        pub fn pos(&self, i: usize) -> Option<(usize, usize)> {
            if i < self.positions.len() {
                self.positions[i]
            } else {
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_compiled_regex() {
        let regex = Regex::new(String::from(r"\w+"));

        assert!(regex.regex.get().is_none());
        assert!(regex.is_match("test"));
        assert!(regex.regex.get().is_some());
    }

    #[test]
    fn serde_as_string() {
        let pattern: Regex = serde_json::from_str("\"just a string\"").unwrap();
        assert_eq!(pattern.regex_str(), "just a string");
        let back_to_str = serde_json::to_string(&pattern).unwrap();
        assert_eq!(back_to_str, "\"just a string\"");
    }
}
