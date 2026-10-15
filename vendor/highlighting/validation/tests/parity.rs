use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};

const RUST: &str = "fn total(values: &[u32]) -> u32 {\n    values.iter().sum()\n}\n";

fn parse(set: &SyntaxSet, language: &str, text: &str, hash: &mut DefaultHasher) {
    let syntax = set
        .find_syntax_by_token(language)
        .unwrap_or_else(|| set.find_syntax_plain_text());
    let mut parser = ParseState::new(syntax);
    let mut scopes = ScopeStack::new();
    for line in text.split_inclusive('\n') {
        line.hash(hash);
        for (offset, operation) in parser.parse_line(line, set).unwrap() {
            offset.hash(hash);
            scopes.apply(&operation).unwrap();
            format!("{scopes}").hash(hash);
        }
    }
}

fn warm(set: &SyntaxSet, hash: &mut DefaultHasher) {
    for _ in 0..1000 {
        parse(set, "rust", RUST, hash);
    }
}

// Golden scope-position digests were produced by unmodified Syntect 5.3.0 /
// fancy-regex 0.16.2 on the repository's pinned Rust 1.98 toolchain. They include
// line text, offsets and scope stacks, not timing or allocator data.
#[test]
fn bundled_grammars_and_71_source_samples_keep_original_scopes() {
    let set = SyntaxSet::load_defaults_newlines();
    let mut hash = DefaultHasher::new();
    parse(&set, "rust", RUST, &mut hash);
    for syntax in set.syntaxes() {
        parse(
            &set,
            &syntax.name,
            "/* comment */\nfn main() { print(\"hello \\\" world\"); }\nlet unicode = 'ع字😀';\n<!-- </script> -->\n0123 0xff 1.23e-4 # end\n",
            &mut hash,
        );
    }
    for case in include_str!("../fixtures/corpus.txt").split('\u{1e}') {
        let (language, source) = case.split_once('\u{1f}').unwrap();
        parse(&set, language, source, &mut hash);
    }
    warm(&set, &mut hash);
    assert_eq!(hash.finish(), 2_893_524_685_548_207_384);
}

#[test]
fn unterminated_cpp_templates_keep_original_scopes() {
    let set = SyntaxSet::load_defaults_newlines();
    let mut hash = DefaultHasher::new();
    parse(&set, "rust", RUST, &mut hash);
    let source =
        "template<typename T> using Type = ".to_owned() + &"std::vector<".repeat(4096) + "T;\n";
    parse(&set, "cpp", &source, &mut hash);
    warm(&set, &mut hash);
    assert_eq!(hash.finish(), 10054874753544959921);
}
