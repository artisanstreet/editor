#![cfg(test)]

#[path = "composer/draft_session.rs"]
mod draft_session;
#[path = "composer/encode.rs"]
mod encode;
#[path = "composer/persistence.rs"]
mod persistence;
#[cfg(windows)]
#[path = "composer/tsf.rs"]
mod tsf;
#[path = "composer/model_line.rs"]
mod model_line;
#[path = "composer/model_list.rs"]
mod model_list;
#[path = "composer/picker.rs"]
mod picker;
#[path = "composer/questionnaire.rs"]
mod questionnaire;
#[path = "composer/sliding_tab.rs"]
mod sliding_tab;
