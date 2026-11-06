use anyhow::{Context, Result, bail};
use aqua_core::{Key, Modifiers, NamedKey, Point};
use std::{path::PathBuf, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
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

pub fn parse(text: &str) -> Result<Vec<Step>> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.trim();
            !line.is_empty() && !line.starts_with('#')
        })
        .map(|(number, line)| {
            parse_step(line.trim_start()).with_context(|| format!("line {}: {line}", number + 1))
        })
        .collect()
}

fn parse_step(line: &str) -> Result<Step> {
    let (command, rest) = line.split_once(' ').unwrap_or((line, ""));
    let numbers = || -> Result<Vec<f32>> { rest.split_whitespace().map(finite_number).collect() };
    let position = || -> Result<Point> {
        match numbers()?.as_slice() {
            [x, y] => Ok(Point::new(*x, *y)),
            _ => bail!("expected <x> <y>"),
        }
    };
    Ok(match command {
        "wait" => match numbers()?.as_slice() {
            [ms] if (0.0..=3_600_000.0).contains(ms) => {
                Step::Wait(Duration::from_millis(ms.round() as u64))
            }
            _ => bail!("expected <milliseconds>"),
        },
        "move" => Step::Move(position()?),
        "click" => Step::Click(position()?, Button::Left),
        "rclick" => Step::Click(position()?, Button::Right),
        "scroll" => match numbers()?.as_slice() {
            [x, y, delta] => Step::Scroll(Point::new(*x, *y), *delta),
            _ => bail!("expected <x> <y> <pixels>"),
        },
        "type" => Step::Type(rest.to_owned()),
        "key" => Step::Key(parse_keystroke(rest.trim())?),
        "settle" => match numbers()?.as_slice() {
            [] => Step::Settle(DEFAULT_SETTLE),
            [ms] if (1.0..=3_600_000.0).contains(ms) => {
                Step::Settle(Duration::from_millis(ms.round() as u64))
            }
            _ => bail!("expected an optional <timeout milliseconds>"),
        },
        "capture" if !rest.trim().is_empty() => Step::Capture(PathBuf::from(rest.trim())),
        "quit" => Step::Quit,
        _ => bail!("unknown step"),
    })
}

fn finite_number(word: &str) -> Result<f32> {
    let value = word
        .parse::<f32>()
        .with_context(|| format!("not a number: {word}"))?;
    if !value.is_finite() {
        bail!("expected a finite number: {word}");
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

pub fn parse_keystroke(text: &str) -> Result<Keystroke> {
    let mut modifiers = Modifiers::NONE;
    let mut rest = text;
    while let Some((prefix, flag)) = MODIFIER_PREFIXES
        .iter()
        .find(|(prefix, _)| rest.len() > prefix.len() && rest.starts_with(prefix))
    {
        modifiers = modifiers.union(*flag);
        rest = &rest[prefix.len()..];
    }
    Ok(Keystroke {
        key: parse_key(rest).with_context(|| format!("not a key: {text}"))?,
        modifiers,
    })
}

fn parse_key(name: &str) -> Result<Key> {
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
        return Ok(Key::Named(named));
    }
    if name == "space" {
        return Ok(Key::Char(' '));
    }
    if let Some(number) = name
        .strip_prefix('f')
        .and_then(|digits| digits.parse::<u8>().ok())
        .filter(|number| (1..=24).contains(number))
    {
        return Ok(Key::Named(NamedKey::Function(number)));
    }
    let mut characters = name.chars();
    match (characters.next(), characters.next()) {
        (Some(character), None) => Ok(Key::Char(character)),
        _ => bail!("unknown key name"),
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
        _ => None,
    }
}
