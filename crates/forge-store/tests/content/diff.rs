use super::*;

const DIFF: &str = "diff --git a/a.rs b/a.rs\r\n--- a/a.rs\r\n+++ b/a.rs\r\n@@ -1,3 +1,3 @@\r\n context\r\n-old\r\n+new\r\n--- removed dashes\r\n+++ added pluses\r\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -0,0 +1 @@\n+tail without newline";

fn split_at_every_byte(text: &str) -> Vec<EditTotals> {
    (0..=text.len())
        .flat_map(|first| (first..=text.len()).map(move |second| (first, second)))
        .map(|(first, second)| {
            let mut scan = DiffScan::default();
            for part in [
                &text.as_bytes()[..first],
                &text.as_bytes()[first..second],
                &text.as_bytes()[second..],
            ] {
                scan.feed(part);
            }
            scan.finish()
        })
        .collect()
}

#[test]
fn a_diff_counts_its_changed_lines_but_not_its_file_headers() {
    assert_eq!(
        totals(DIFF),
        EditTotals {
            added: 3,
            removed: 2
        }
    );
}

#[test]
fn a_diff_counts_the_same_however_its_chunks_are_cut() {
    let whole = totals(DIFF);
    assert!(
        split_at_every_byte(DIFF)
            .into_iter()
            .all(|split| split == whole)
    );
}

#[test]
fn prose_outside_any_hunk_counts_like_the_lines_it_starts_with() {
    assert_eq!(
        totals("- a bullet\n+ a plus\n--- a rule\n\n"),
        EditTotals {
            added: 1,
            removed: 1
        }
    );
}
