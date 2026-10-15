//! `elicitation/create`: an MCP-style form, shown as one question per schema
//! property, and the typed values the answers become.

use super::requests::Pending;
use super::tools::str_field;
use super::{AcpDecoder, Profile};
use crate::EngineEvent;
use artisan_api::{ItemKind, Phase, Question, QuestionAnswer, QuestionOption, Status};
use serde_json::{Map, Value, json};

#[derive(Debug)]
pub(super) struct ElicitationField {
    name: String,
    schema: Value,
    required: bool,
}

impl ElicitationField {
    #[expect(
        clippy::too_many_lines,
        reason = "one reading of a JSON-schema property: choices, scalars, ranges and arrays"
    )]
    fn value(&self, answer: Option<&QuestionAnswer>) -> Result<Option<Value>, String> {
        let Some(answer) = answer else {
            return if self.required {
                Err(format!("{} is required", self.name))
            } else {
                Ok(None)
            };
        };
        let kind = self.schema["type"].as_str().unwrap_or("string");
        let source = if kind == "array" {
            &self.schema["items"]
        } else {
            &self.schema
        };
        let choices = source["oneOf"]
            .as_array()
            .or_else(|| source["anyOf"].as_array())
            .or_else(|| source["enum"].as_array());
        let values = match answer {
            QuestionAnswer::Choices { ids } if choices.is_some() => ids
                .inline()
                .ok_or_else(|| format!("{} submitted choices must be inline", self.name))?
                .to_vec(),
            QuestionAnswer::Text { text } if choices.is_none() => {
                if kind == "array" {
                    let values: Vec<Value> = serde_json::from_str(text)
                        .map_err(|_| format!("{} requires a JSON array", self.name))?;
                    values
                        .into_iter()
                        .map(|value| {
                            value
                                .as_str()
                                .map_or_else(|| value.to_string(), str::to_owned)
                        })
                        .collect()
                } else {
                    vec![text.clone()]
                }
            }
            _ => return Err(format!("{} has an answer of the wrong kind", self.name)),
        };
        for unsupported in [
            "pattern",
            "format",
            "allOf",
            "not",
            "$ref",
            "multipleOf",
            "uniqueItems",
        ] {
            if source.get(unsupported).is_some() || self.schema.get(unsupported).is_some() {
                return Err(format!(
                    "{} uses unsupported schema constraint {unsupported}",
                    self.name
                ));
            }
        }
        let scalar_kind = source["type"].as_str().unwrap_or("string");
        let scalar = |text: &str| -> Result<Value, String> {
            if let Some(choices) = choices {
                return choices
                    .iter()
                    .enumerate()
                    .find_map(|(index, choice)| {
                        (text == format!("option-{index}")).then(|| {
                            choice
                                .get("const")
                                .cloned()
                                .unwrap_or_else(|| choice.clone())
                        })
                    })
                    .ok_or_else(|| format!("{} has an invalid choice", self.name));
            }
            let value = match scalar_kind {
                "boolean" => match text.trim().to_ascii_lowercase().as_str() {
                    "true" | "yes" => Value::Bool(true),
                    "false" | "no" => Value::Bool(false),
                    _ => return Err(format!("{} must be true or false", self.name)),
                },
                "integer" => text
                    .trim()
                    .parse::<i64>()
                    .map(Value::from)
                    .map_err(|_| format!("{} must be an integer", self.name))?,
                "number" => {
                    let number = text
                        .trim()
                        .parse::<f64>()
                        .map_err(|_| format!("{} must be a number", self.name))?;
                    serde_json::Number::from_f64(number)
                        .map(Value::Number)
                        .ok_or_else(|| format!("{} must be finite", self.name))?
                }
                "string" => Value::String(text.to_owned()),
                other => return Err(format!("{} has unsupported schema type {other}", self.name)),
            };
            if let Some(number) = value.as_f64()
                && (source["minimum"]
                    .as_f64()
                    .is_some_and(|minimum| number < minimum)
                    || source["maximum"]
                        .as_f64()
                        .is_some_and(|maximum| number > maximum)
                    || source["exclusiveMinimum"]
                        .as_f64()
                        .is_some_and(|minimum| number <= minimum)
                    || source["exclusiveMaximum"]
                        .as_f64()
                        .is_some_and(|maximum| number >= maximum))
            {
                return Err(format!("{} is outside its allowed range", self.name));
            }
            if let Some(text) = value.as_str() {
                let count = text.chars().count() as u64;
                if source["minLength"]
                    .as_u64()
                    .is_some_and(|minimum| count < minimum)
                    || source["maxLength"]
                        .as_u64()
                        .is_some_and(|maximum| count > maximum)
                {
                    return Err(format!("{} has an invalid length", self.name));
                }
            }
            Ok(value)
        };
        if kind == "array" {
            let count = values.len() as u64;
            if self.schema["minItems"]
                .as_u64()
                .is_some_and(|minimum| count < minimum)
                || self.schema["maxItems"]
                    .as_u64()
                    .is_some_and(|maximum| count > maximum)
            {
                return Err(format!("{} has an invalid number of selections", self.name));
            }
            Ok(Some(Value::Array(
                values
                    .iter()
                    .map(|value| scalar(value))
                    .collect::<Result<_, _>>()?,
            )))
        } else {
            if values.len() != 1 {
                return Err(format!("{} needs one value", self.name));
            }
            scalar(&values[0]).map(Some)
        }
    }
}

impl AcpDecoder {
    pub(super) fn elicitation(
        &mut self,
        rpc: Value,
        params: &Value,
        events: &mut Vec<EngineEvent>,
    ) {
        let schema = &params["requestedSchema"];
        let Some(properties) = schema["properties"]
            .as_object()
            .filter(|properties| {
                !properties.is_empty() && properties.keys().all(|name| !name.trim().is_empty())
            })
            .filter(|_| params["mode"].as_str().is_none_or(|mode| mode == "form"))
        else {
            self.reply(rpc, json!({ "action": "decline" }));
            return;
        };
        let message = params["message"].as_str().unwrap_or_default();
        let mut fields = Vec::new();
        let mut questions = Vec::new();
        for (key, property) in properties {
            let kind = property["type"].as_str().unwrap_or("string").to_owned();
            let title = property["title"].as_str().filter(|title| !title.is_empty());
            let description = property["description"]
                .as_str()
                .filter(|text| !text.is_empty());
            questions.push(Question {
                id: key.clone(),
                header: title.map(str::to_owned),
                text: (description
                    .or(title)
                    .or_else(|| (!message.trim().is_empty()).then_some(message))
                    .unwrap_or(key)
                    .to_owned())
                .into(),
                multi_select: kind == "array",
                options: (schema_choices(property)).into(),
            });
            fields.push(ElicitationField {
                name: key.clone(),
                schema: property.clone(),
                required: schema["required"]
                    .as_array()
                    .is_some_and(|required| required.iter().any(|name| name.as_str() == Some(key))),
            });
        }
        self.questions += 1;
        let key = format!("question:{}", self.questions);
        // Grok asks in passing and keeps working; nothing waits on the answer.
        let blocking = self.profile == Profile::Cursor;
        self.pending.insert(
            key.clone(),
            Pending::Elicitation {
                rpc,
                fields,
                asked: questions.clone(),
            },
        );
        self.close_message(Phase::Commentary, events);
        events.push(EngineEvent::Item {
            key,
            parent: None,
            status: Status::AwaitingUser,
            kind: ItemKind::Question {
                blocking,
                questions: questions.into(),
                answers: None,
                answer_text: None,
            },
        });
    }

    /// The user's answers as the form's values, or why they don't fit it:
    /// then the request stays open for another answer.
    pub(super) fn answer_elicitation(
        &mut self,
        key: &str,
        rpc: Value,
        fields: Vec<ElicitationField>,
        asked: Vec<Question>,
        answers: &[Option<QuestionAnswer>],
        events: &mut Vec<EngineEvent>,
    ) {
        if answers.iter().all(Option::is_none) {
            self.reply(rpc, json!({ "action": "decline" }));
        } else {
            let validated = fields
                .iter()
                .enumerate()
                .map(|(index, field)| {
                    field
                        .value(answers.get(index).and_then(Option::as_ref))
                        .map(|value| (field.name.clone(), value))
                })
                .collect::<Result<Vec<_>, _>>();
            let values = match validated {
                Ok(values) => values,
                Err(message) => {
                    tracing::warn!(engine = ?self.profile, session = ?self.session_id, key, message, "invalid elicitation answer; request remains pending");
                    events.push(crate::question::invalid_answer(key, message));
                    self.pending
                        .insert(key.to_owned(), Pending::Elicitation { rpc, fields, asked });
                    return;
                }
            };
            let content: Map<String, Value> = values
                .into_iter()
                .filter_map(|(name, value)| Some((name, value?)))
                .collect();
            self.reply(rpc, json!({ "action": "accept", "content": content }));
        }
        events.push(self.answered_question(key, asked, answers));
    }
}

/// Choices offered by one elicitation schema property.
fn schema_choices(property: &Value) -> Vec<QuestionOption> {
    let items = &property["items"];
    let choices = property["oneOf"]
        .as_array()
        .or_else(|| property["anyOf"].as_array())
        .or_else(|| property["enum"].as_array())
        .or_else(|| items["anyOf"].as_array())
        .or_else(|| items["oneOf"].as_array())
        .or_else(|| items["enum"].as_array());
    choices
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, choice)| match choice {
            Value::String(label) => Some(QuestionOption {
                id: format!("option-{index}"),
                label: label.clone(),
                description: None,
            }),
            Value::Object(_) => Some(QuestionOption {
                id: format!("option-{index}"),
                label: str_field(choice, &["title"]).or_else(|| {
                    choice.get("const").map(|value| {
                        value
                            .as_str()
                            .map_or_else(|| value.to_string(), str::to_owned)
                    })
                })?,
                description: (str_field(choice, &["description"])).map(Into::into),
            }),
            Value::Number(_) | Value::Bool(_) => Some(QuestionOption {
                id: format!("option-{index}"),
                label: choice.to_string(),
                description: None,
            }),
            _ => None,
        })
        .collect()
}
