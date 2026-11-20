use artisan_api::{Collection, ItemId, Question, QuestionOption, TextContent};
use divan::Bencher;
use editor_composer::questionnaire::{ComposerQuestionnaire, QuestionnairePanel};

const QUESTIONNAIRES: usize = 8;
const QUESTIONS: usize = 10;
const OPTIONS: usize = 4;
const DISPATCHES: usize = 1_000;

fn main() {
    divan::main();
}

fn asked() -> Vec<(ItemId, Collection<Question>)> {
    (0..QUESTIONNAIRES)
        .map(|questionnaire| {
            let questions = (0..QUESTIONS)
                .map(|question| Question {
                    id: format!("question-{question}"),
                    header: Some(format!("Step {question}")),
                    text: TextContent::Inline(format!(
                        "Which renderer should questionnaire {questionnaire} use for step {question}?"
                    )),
                    multi_select: false,
                    options: Collection::Inline(
                        (0..OPTIONS)
                            .map(|option| QuestionOption {
                                id: format!("option-{option}"),
                                label: format!("Renderer {option}"),
                                description: Some(TextContent::Inline(
                                    "Keeps every glyph in an atlas and repaints damaged rows."
                                        .into(),
                                )),
                            })
                            .collect(),
                    ),
                })
                .collect();
            (
                ItemId(format!("questionnaire-{questionnaire}")),
                Collection::Inline(questions),
            )
        })
        .collect()
}

fn sync(
    panel: &mut QuestionnairePanel,
    asked: &[(ItemId, Collection<Question>)],
    text: &mut String,
) {
    if panel.holds(asked.iter().map(|(item, questions)| (item, questions))) {
        return;
    }
    panel.set_questionnaires(
        asked
            .iter()
            .map(|(item, questions)| ComposerQuestionnaire {
                item: item.clone(),
                questions: questions.clone(),
            })
            .collect(),
        text,
    );
}

#[divan::bench(sample_count = 20)]
fn keep_the_held_questionnaires_across_dispatches(bencher: Bencher<'_, '_>) {
    let asked = asked();
    let mut panel = QuestionnairePanel::default();
    let mut text = String::new();
    sync(&mut panel, &asked, &mut text);
    bencher.bench_local(|| {
        for _ in 0..DISPATCHES {
            sync(&mut panel, &asked, &mut text);
        }
    });
}
