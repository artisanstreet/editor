use super::*;

fn screen_after(chunks: &[&[u8]]) -> TerminalScreen {
    let mut screen = TerminalScreen::new();
    for chunk in chunks {
        screen.feed(chunk);
    }
    screen
}

#[test]
fn a_joined_emoji_split_across_chunks_takes_one_wide_column() {
    let emoji = "👩\u{200d}💻".as_bytes();
    let (head, tail) = emoji.split_at(5);
    let screen = screen_after(&[head, tail, b"X\x1b[3GY"]);
    assert_eq!(
        screen.plain_text(),
        "👩\u{200d}💻Y",
        "column 3 is the cell right after the emoji"
    );
}

#[test]
fn a_flag_and_a_skin_tone_each_take_one_wide_column() {
    let screen = screen_after(&[
        "🇳".as_bytes(),
        "🇴".as_bytes(),
        "👍".as_bytes(),
        "🏽".as_bytes(),
        b"ab\x1b[5G!",
    ]);
    assert_eq!(screen.plain_text(), "🇳🇴👍🏽!b");
}

#[test]
fn erasing_from_the_second_half_of_a_wide_character_erases_all_of_it() {
    let screen = screen_after(&["a界".as_bytes(), b"\x1b[3G\x1b[K"]);
    assert_eq!(screen.plain_text().trim_end(), "a");
}

#[test]
fn erasing_up_to_the_first_half_of_a_wide_character_keeps_later_columns_in_place() {
    let screen = screen_after(&["a界b".as_bytes(), b"\x1b[2G\x1b[1K"]);
    assert_eq!(screen.plain_text(), "   b");
}
