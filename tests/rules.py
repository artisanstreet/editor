import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("rules", Path(__file__).resolve().parents[1] / "src/rules.py")
rules = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(rules)
SRC = Path("crates/forge-store/src/lib.rs")
TESTS = Path("crates/forge-store/tests/lib.rs")
SUPPORT = Path("crates/test-support/src/lib.rs")


def found(source, path=SRC):
    return [(violation.line, violation.rule) for violation in rules.check(path, source)]


class Comments(unittest.TestCase):
    def test_line_doc_and_nested_block_comments_are_reported_on_their_own_lines(self):
        source = 'fn a() {}\n/// doc\nfn b() {} // trailing\n//! inner\n/* outer /* inner */ still\n comment */ fn c() {}\n'
        self.assertEqual(found(source), [(2, "comment"), (3, "comment"), (4, "comment"), (5, "comment")])
        self.assertEqual(found(source, TESTS), found(source))

    def test_comment_markers_inside_literals_are_code(self):
        source = "\n".join((
            'const URL: &str = "https://example.com/a//b";',
            'const SLASHES: &[u8] = b"// /* bytes */";',
            r'const RAW: &str = r#"a "quoted" // value /* and */"#;',
            r'const ESCAPED: &str = "a \" // still inside";',
            "fn borrow<'a>(text: &'a str) -> &'a str { if text.is_empty() { \"//\" } else { text } }",
            "const QUOTE: char = '\"';",
            "const TICK: char = '\\'';",
            "const LABEL: () = 'outer: loop { break 'outer; };",
            'const IDENT: u8 = r#type::VALUE; const MULTI: &str = "line\n// two";',
        ))
        self.assertEqual(found(source), [])

    def test_a_quote_char_does_not_hide_the_comment_after_it(self):
        self.assertEqual(found("let quote = '\"'; // a comment\nlet x = 1;\n"), [(1, "comment")])

    def test_lines_are_counted_through_multiline_raw_strings(self):
        self.assertEqual(found('const A: &str = r"one\ntwo\nthree";\n/* four */\n'), [(4, "comment")])

    def test_only_a_leading_license_header_is_allowed(self):
        header = "// SPDX-License-Identifier: MIT OR Apache-2.0\n// Copyright (c) 2026 Artisan Street\n\nfn a() {}\n"
        self.assertEqual(found(header), [])
        self.assertEqual(found(header + "// SPDX-License-Identifier: MIT\n"), [(5, "comment")])
        self.assertEqual(found("// a note\nfn a() {}\n"), [(1, "comment")])

    def test_an_unterminated_literal_is_reported_instead_of_misread(self):
        self.assertEqual(found('fn a() {}\nconst B: &str = "open;\n'), [(2, "lex")])
        self.assertEqual(found("fn a() {}\n/* open /* nested */\n"), [(2, "lex")])


class Sql(unittest.TestCase):
    def test_queries_and_pragmas_in_src_strings_are_reported(self):
        source = "\n".join((
            'conn.query_row("SELECT id, title FROM threads WHERE id = ?1", [id], read)?;',
            'conn.execute(r#"INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)"#, args)?;',
            'conn.execute("UPDATE runs SET state = ?1", [state])?;',
            'conn.execute_batch("DELETE FROM items; VACUUM INTO \'x\'")?;',
            'conn.pragma("PRAGMA journal_mode = WAL")?;',
            'conn.execute_batch("CREATE TABLE IF NOT EXISTS t (id INTEGER)")?;',
            'conn.execute_batch("BEGIN EXCLUSIVE; COMMIT;")?;',
        ))
        self.assertEqual(found(source), [(line, "sql") for line in range(1, 8)])

    def test_prose_sql_files_and_tests_are_not_reported(self):
        source = "\n".join((
            'const HINT: &str = "Select All copies these lines; select a file from the list.";',
            'const UPDATE: &str = "UPDATE available. Delete from history to free space.";',
            'const LOOP: &str = "one select loop; each arm is one event source";',
            'conn.query_row(include_str!("sql/coordination/get.sql"), [id], read)?;',
        ))
        self.assertEqual(found(source), [])
        self.assertEqual(found('conn.execute("DELETE FROM items", [])?;', TESTS), [])


class InlineTests(unittest.TestCase):
    def test_tests_written_inside_src_are_reported(self):
        source = "\n".join((
            "#[test]",
            "fn a() {}",
            "#[tokio::test(flavor = \"multi_thread\")]",
            "async fn b() {}",
            "#[cfg(test)]",
            "mod tests { fn c() {} }",
            "#[cfg(all(test, unix))]",
            "pub(crate) mod unix_tests { }",
            "#[cfg(test)]",
            "mod beside;",
        ))
        self.assertEqual(found(source), [(1, "inline-test"), (3, "inline-test"), (5, "inline-test"), (7, "inline-test"), (9, "inline-test")])

    def test_modules_pointed_into_tests_and_non_test_cfgs_pass(self):
        source = "\n".join((
            "#[cfg(test)]",
            '#[path = "../tests/store/unit.rs"]',
            "mod tests;",
            '#[cfg(test)] #[path = "../../tests/ui/select.rs"] mod select_tests;',
            "#[cfg(not(test))]",
            "mod production { }",
            "#[cfg_attr(test, derive(Debug))]",
            "struct Shown;",
            "#[cfg(any(test, feature = \"qualification\"))]",
            "mod fixture { }",
            "#[should_panic] #[ignore = \"slow\"] fn helper() {}",
        ))
        self.assertEqual(found(source), [])

    def test_tests_directory_may_hold_tests(self):
        self.assertEqual(found("#[test]\nfn a() {}\n#[cfg(test)]\nmod more { }\n", TESTS), [])


class Invariants(unittest.TestCase):
    def test_expect_and_unreachable_must_name_the_invariant(self):
        source = "\n".join((
            'let a = map.get(&id).expect("thread exists");',
            "let b = next.checked_add(1).expect(",
            '    "the counter never overflows",',
            ");",
            "match state { State::Done => unreachable!(), _ => {} }",
            'match state { State::Gone => unreachable!(""), _ => {} }',
            'let c = value.expect(&format!("item {id}"));',
        ))
        self.assertEqual(found(source), [(1, "invariant"), (2, "invariant"), (5, "invariant"), (6, "invariant"), (7, "invariant")])

    def test_invariant_messages_constants_and_other_expect_methods_pass(self):
        source = "\n".join((
            'const STRING_WRITE: &str = "invariant: writing to a String cannot fail";',
            'let a = map.get(&id).expect("invariant: the thread was inserted above");',
            "let b = next.checked_add(1).expect(",
            '    "invariant: the sequence never wraps",',
            ");",
            "write!(out, \"x\").expect(STRING_WRITE);",
            "parser.expect(Token::Comma)?;",
            'match state { State::Done => unreachable!("a finished run never resumes ({run_id})"), _ => {} }',
        ))
        self.assertEqual(found(source), [])

    def test_unconfirmed_messages_are_allowed_outside_production_code(self):
        source = 'let listener = bind().expect("bind the stub Forge");\nunreachable!();\n'
        self.assertEqual(found(source, TESTS), [])
        self.assertEqual(found(source, SUPPORT), [])
        self.assertEqual(found(source, Path("crates/forge/benches/recall.rs")), [])


class Scan(unittest.TestCase):
    def test_scan_covers_our_crates_and_skips_vendor(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for relative in ("crates/forge/src/main.rs", "crates/forge/build.rs", "vendor/mermaid/src/lib.rs"):
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text("// a comment\nfn main() {}\n", encoding="utf-8")
            self.assertEqual([(violation.path, violation.line) for violation in rules.scan(root)], [("crates/forge/build.rs", 1), ("crates/forge/src/main.rs", 1)])


if __name__ == "__main__":
    unittest.main()
