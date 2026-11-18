use super::answer_text;

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
