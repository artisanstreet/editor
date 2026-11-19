use aqua_core::{Key, Modifiers, NamedKey, Point};
use std::{path::PathBuf, time::Duration};

#[derive(Debug, thiserror::Error)]
pub enum StepError {
    #[error("expected {expected}")]
    Arguments { expected: &'static str },
    #[error("not a number: {word}")]
    Number {
        word: String,
        #[source]
        source: std::num::ParseFloatError,
    },
    #[error("expected a finite number: {word}")]
    NotFinite { word: String },
    #[error("unknown step")]
    Unknown,
    #[error("not a key: {text}")]
    Key { text: String },
}

#[derive(Debug, thiserror::Error)]
#[error("line {line}: {text}")]
pub struct ScriptError {
    line: usize,
    text: String,
    #[source]
    source: StepError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keystroke {
    pub key: Key,
    pub modifiers: Modifiers,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Wait(Duration),
    Move(Point),
    Click(Point, Button),
    Scroll(Point, f32),
    Type(String),
    Key(Keystroke),
    Settle(Duration),
    Capture(PathBuf),
    Quit,
}

pub const DEFAULT_SETTLE: Duration = Duration::from_secs(20);

pub fn parse(text: &str) -> Result<Vec<Step>, ScriptError> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|(number, line)| {
            parse_step(line.trim_start()).map_err(|source| ScriptError {
                line: number + 1,
                text: line.to_owned(),
                source,
            })
        })
        .collect()
}

fn arguments<T>(expected: &'static str) -> Result<T, StepError> {
    Err(StepError::Arguments { expected })
}

fn parse_step(line: &str) -> Result<Step, StepError> {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let numbers =
        || -> Result<Vec<f32>, StepError> { rest.split_whitespace().map(finite_number).collect() };
    let position = || -> Result<Point, StepError> {
        match numbers()?.as_slice() {
            [x, y] => Ok(Point::new(*x, *y)),
            _ => arguments("<x> <y>"),
        }
    };
    Ok(match command {
        "wait" => match numbers()?.as_slice() {
            [ms] if (0.0..=3_600_000.0).contains(ms) => {
                Step::Wait(Duration::from_millis(crate::measure::whole_millis(*ms)))
            }
            _ => return arguments("<milliseconds>"),
        },
        "move" => Step::Move(position()?),
        "click" => Step::Click(position()?, Button::Left),
        "rclick" => Step::Click(position()?, Button::Right),
        "scroll" => match numbers()?.as_slice() {
            [x, y, delta] => Step::Scroll(Point::new(*x, *y), *delta),
            _ => return arguments("<x> <y> <pixels>"),
        },
        "type" => Step::Type(rest.to_owned()),
        "key" => Step::Key(parse_keystroke(rest.trim())?),
        "settle" => match numbers()?.as_slice() {
            [] => Step::Settle(DEFAULT_SETTLE),
            [ms] if (1.0..=3_600_000.0).contains(ms) => {
                Step::Settle(Duration::from_millis(crate::measure::whole_millis(*ms)))
            }
            _ => return arguments("an optional <timeout milliseconds>"),
        },
        "capture" if !rest.trim().is_empty() => Step::Capture(PathBuf::from(rest.trim())),
        "quit" => Step::Quit,
        _ => return Err(StepError::Unknown),
    })
}

fn finite_number(word: &str) -> Result<f32, StepError> {
    let value = word.parse::<f32>().map_err(|source| StepError::Number {
        word: word.to_owned(),
        source,
    })?;
    if !value.is_finite() {
        return Err(StepError::NotFinite {
            word: word.to_owned(),
        });
    }
    Ok(value)
}

const MODIFIER_PREFIXES: [(&str, Modifiers); 8] = [
    ("ctrl-", Modifiers::CONTROL),
    ("control-", Modifiers::CONTROL),
    ("alt-", Modifiers::ALT),
    ("shift-", Modifiers::SHIFT),
    ("cmd-", Modifiers::LOGO),
    ("super-", Modifiers::LOGO),
    ("win-", Modifiers::LOGO),
    ("meta-", Modifiers::LOGO),
];

pub fn parse_keystroke(text: &str) -> Result<Keystroke, StepError> {
    let mut modifiers = Modifiers::NONE;
    let mut rest = text;
    while let Some((remainder, flag)) = MODIFIER_PREFIXES.iter().find_map(|(prefix, flag)| {
        rest.strip_prefix(prefix)
            .filter(|remainder| !remainder.is_empty())
            .map(|remainder| (remainder, flag))
    }) {
        modifiers = modifiers.union(*flag);
        rest = remainder;
    }
    Ok(Keystroke {
        key: parse_key(rest).ok_or_else(|| StepError::Key {
            text: text.to_owned(),
        })?,
        modifiers,
    })
}

fn parse_key(name: &str) -> Option<Key> {
    let named = match name {
        "enter" | "return" => Some(NamedKey::Enter),
        "tab" => Some(NamedKey::Tab),
        "backspace" => Some(NamedKey::Backspace),
        "delete" => Some(NamedKey::Delete),
        "insert" => Some(NamedKey::Insert),
        "escape" | "esc" => Some(NamedKey::Escape),
        "left" => Some(NamedKey::Left),
        "right" => Some(NamedKey::Right),
        "up" => Some(NamedKey::Up),
        "down" => Some(NamedKey::Down),
        "home" => Some(NamedKey::Home),
        "end" => Some(NamedKey::End),
        "pageup" => Some(NamedKey::PageUp),
        "pagedown" => Some(NamedKey::PageDown),
        _ => None,
    };
    if let Some(named) = named {
        return Some(Key::Named(named));
    }
    if name == "space" {
        return Some(Key::Char(' '));
    }
    if let Some(number) = name
        .strip_prefix('f')
        .and_then(|digits| digits.parse::<u8>().ok())
        .filter(|number| (1..=24).contains(number))
    {
        return Some(Key::Named(NamedKey::Function(number)));
    }
    let mut characters = name.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Some(Key::Char(character)),
        _ => None,
    }
}

pub fn types_text(keystroke: &Keystroke) -> Option<char> {
    match keystroke.key {
        Key::Char(character)
            if !keystroke.modifiers.contains(Modifiers::CONTROL)
                && !keystroke.modifiers.contains(Modifiers::ALT)
                && !keystroke.modifiers.contains(Modifiers::LOGO) =>
        {
            Some(character)
        }
        Key::Char(_) | Key::Named(_) => None,
    }
}
