use super::{answer_text, plain_block};

#[test]
fn an_answer_keeps_the_words_you_answered_inside_it() {
    assert_eq!(
        answer_text("You answered: Print \"You answered: yes\" literally"),
        "Print \"You answered: yes\" literally"
    );
    assert_eq!(
        answer_text("Print \"You answered: yes\" literally"),
        "Print \"You answered: yes\" literally"
    );
}

#[test]
fn a_long_inline_error_shows_each_part_once() {
    let error = (0..400)
        .map(|line| format!("error[{line}]: the build broke"))
        .collect::<Vec<_>>()
        .join(" ");
    let blocks = (0..)
        .map_while(|block| plain_block(&error, block))
        .collect::<Vec<_>>();
    assert!(blocks.len() > 3, "{}", blocks.len());
    assert!(blocks.iter().all(|block| block.len() <= 1024));
    assert_eq!(blocks.concat(), error);
}
