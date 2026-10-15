use std::collections::BTreeSet;
use syntect::parsing::syntax_definition::Pattern;
use syntect::parsing::{Regex, Region, SyntaxSet};
fn main() {
    let set = SyntaxSet::load_defaults_newlines().into_builder();
    let mut patterns = BTreeSet::new();
    let mut raw = 0;
    let mut dynamic = 0;
    for syntax in set.syntaxes() {
        if let Some(ref first) = syntax.first_line_match {
            patterns.insert(first.clone());
        }
        for context in syntax.contexts.values() {
            for pattern in &context.patterns {
                if let Pattern::Match(pattern) = pattern {
                    raw += 1;
                    if pattern.has_captures {
                        dynamic += 1;
                        for token in ["", "EOF", "ع😀[]()\\$.*+?^", "\"'`"] {
                            let text = token.repeat(9);
                            let capture =
                                Regex::new(format!("(.{{{}}})", token.chars().count()).repeat(9));
                            let mut region = Region::new();
                            assert!(capture.search(&text, 0, text.len(), Some(&mut region)));
                            patterns.insert(
                                pattern
                                    .regex_with_refs(&region, &text)
                                    .regex_str()
                                    .to_owned(),
                            );
                        }
                    } else {
                        patterns.insert(pattern.regex.regex_str().to_owned());
                    }
                }
            }
        }
    }
    let mut fancy_rejected = 0;
    let mut onig_rejected = 0;
    for pattern in &patterns {
        if let Err(err) = fancy_regex::RegexBuilder::new(pattern)
            .oniguruma_mode(true)
            .build()
        {
            fancy_rejected += 1;
            eprintln!("FANCY_REJECT {err:?} {pattern:?}");
        }
        if let Some(err) = Regex::try_compile(pattern) {
            onig_rejected += 1;
            eprintln!("ONIG_REJECT {err:?} {pattern:?}");
        }
    }
    println!(
        "syntaxes={} raw_patterns={raw} dynamic_patterns={dynamic} unique_expansions={} fancy_rejected={fancy_rejected} onig_rejected={onig_rejected}",
        set.syntaxes().len(),
        patterns.len()
    );
    if fancy_rejected > 0 || onig_rejected > 0 {
        std::process::exit(1);
    }
}
