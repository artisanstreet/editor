use crate::desktop::Button;
use forge_runs::Refused;
use serde_json::{Map, Value};
use std::time::Duration;

const MAX_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct At {
    x: f64,
    y: f64,
}

impl At {
    #[must_use]
    pub fn new(x: f64, y: f64) -> Option<Self> {
        let valid = |coordinate: f64| coordinate.is_finite() && coordinate >= 0.0;
        (valid(x) && valid(y)).then_some(Self { x, y })
    }

    #[must_use]
    pub const fn x(self) -> f64 {
        self.x
    }

    #[must_use]
    pub const fn y(self) -> f64 {
        self.y
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Move(At),
    Click {
        at: At,
        button: Button,
        count: u8,
    },
    Down {
        at: Option<At>,
        button: Button,
    },
    Up {
        at: Option<At>,
        button: Button,
    },
    Drag {
        from: At,
        through: Vec<At>,
        to: At,
        button: Button,
    },
    Scroll {
        at: At,
        dx: i32,
        dy: i32,
    },
    Key(Vec<Key>),
    KeyDown(Key),
    KeyUp(Key),
    Type(String),
    Wait(Wait),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wait {
    For(Duration),
    Changed(Duration),
    Settled(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Named(Named),
    Char(char),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Named {
    Ctrl,
    Shift,
    Alt,
    Super,
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    Space,
    CapsLock,
    NumLock,
    ScrollLock,
    PrintScreen,
    Pause,
    Menu,
    F(u8),
}

pub fn parse(steps: &Value) -> Result<Vec<Step>, Refused> {
    let steps = steps
        .as_array()
        .ok_or_else(|| Refused("steps is a list of steps.".to_owned()))?;
    if steps.is_empty() {
        return Err(Refused("steps is empty; give at least one.".to_owned()));
    }
    if steps.len() > 256 {
        return Err(Refused("at most 256 steps per batch".into()));
    }
    steps
        .iter()
        .enumerate()
        .map(|(index, step)| {
            step.as_object()
                .ok_or_else(|| "a step is an object such as {\"click\": [x, y]}".to_owned())
                .and_then(parse_step)
                .map_err(|error| Refused(format!("Step {}: {error}", index + 1)))
        })
        .collect()
}

fn parse_step(step: &Map<String, Value>) -> Result<Step, String> {
    const ACTIONS: [&str; 11] = [
        "move", "click", "down", "up", "drag", "scroll", "key", "key_down", "key_up", "type",
        "wait",
    ];
    let actions: Vec<&str> = ACTIONS
        .into_iter()
        .filter(|action| step.contains_key(*action))
        .collect();
    let [action] = actions.as_slice() else {
        return Err(format!("give exactly one action of {}", ACTIONS.join(", ")));
    };
    for field in step.keys() {
        if field != action
            && !matches!(
                (*action, field.as_str()),
                ("click", "button" | "count")
                    | ("down" | "up" | "drag", "button")
                    | ("scroll", "dx" | "dy")
            )
        {
            return Err(format!("unexpected field {field} for {action}"));
        }
    }
    let value = &step[*action];
    let button = || button(step.get("button"));
    Ok(match *action {
        "move" => Step::Move(at(value)?),
        "click" => Step::Click {
            at: at(value)?,
            button: button()?,
            count: match step.get("count") {
                None => 1,
                Some(v) => v
                    .as_u64()
                    .filter(|n| (1..=3).contains(n))
                    .and_then(|n| u8::try_from(n).ok())
                    .ok_or("count must be 1 to 3")?,
            },
        },
        "down" => Step::Down {
            at: optional_at(value)?,
            button: button()?,
        },
        "up" => Step::Up {
            at: optional_at(value)?,
            button: button()?,
        },
        "drag" => {
            let path = value.as_array().ok_or("drag must be a list of points")?;
            let (Some((from, rest)), true) = (path.split_first(), path.len() <= 256) else {
                return Err("drag needs 2 to 256 points".into());
            };
            let Some((to, through)) = rest.split_last() else {
                return Err("drag needs 2 to 256 points".into());
            };
            Step::Drag {
                from: at(from)?,
                through: through.iter().map(at).collect::<Result<_, _>>()?,
                to: at(to)?,
                button: button()?,
            }
        }
        "scroll" => Step::Scroll {
            at: at(value)?,
            dx: wheel(step.get("dx"))?,
            dy: wheel(step.get("dy"))?,
        },
        "key" => {
            let chord = value.as_str().ok_or("key must be a chord such as ctrl+s")?;
            let keys = chord.split('+').map(key).collect::<Result<Vec<_>, _>>()?;
            if keys.len() > 8 {
                return Err("a chord has at most 8 keys".into());
            }
            Step::Key(keys)
        }
        "key_down" => Step::KeyDown(key(value.as_str().ok_or("key_down must be a key name")?)?),
        "key_up" => Step::KeyUp(key(value.as_str().ok_or("key_up must be a key name")?)?),
        "type" => {
            let text = value.as_str().ok_or("type must be text")?;
            if text.len() > 16384 {
                return Err("type is limited to 16384 bytes per batch".into());
            }
            Step::Type(text.to_owned())
        }
        "wait" => Step::Wait(wait(value)?),
        other => unreachable!("invariant: {other} is one of ACTIONS"),
    })
}

fn at(value: &Value) -> Result<At, String> {
    let xy = value
        .as_array()
        .filter(|v| v.len() == 2)
        .ok_or("a point is [x, y]")?;
    xy[0]
        .as_f64()
        .zip(xy[1].as_f64())
        .and_then(|(x, y)| At::new(x, y))
        .ok_or_else(|| "coordinates must be finite and nonnegative".to_owned())
}
fn optional_at(value: &Value) -> Result<Option<At>, String> {
    if value == &Value::Bool(true) {
        Ok(None)
    } else {
        at(value).map(Some)
    }
}
fn button(value: Option<&Value>) -> Result<Button, String> {
    let name = match value {
        None => "left",
        Some(v) => v.as_str().ok_or("button must be text")?,
    };
    match name {
        "left" => Ok(Button::Left),
        "right" => Ok(Button::Right),
        "middle" => Ok(Button::Middle),
        _ => Err("button must be left, right or middle".into()),
    }
}
fn wheel(value: Option<&Value>) -> Result<i32, String> {
    match value {
        None => Ok(0),
        Some(v) => v
            .as_i64()
            .filter(|n| (-100..=100).contains(n))
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| "wheel notches must be an integer from -100 to 100".into()),
    }
}
fn wait(value: &Value) -> Result<Wait, String> {
    if !value.is_object() {
        return Err("wait must be an object".into());
    }
    for key in ["ms", "timeout_ms"] {
        if value.get(key).is_some_and(|v| v.as_u64().is_none()) {
            return Err(format!("{key} must be a nonnegative integer"));
        }
    }
    let ms = value
        .get("ms")
        .or_else(|| value.get("timeout_ms"))
        .and_then(Value::as_u64)
        .unwrap_or(1000);
    let duration = Duration::from_millis(ms);
    if duration > MAX_WAIT {
        return Err("wait is limited to 30000 ms".into());
    }
    match value.get("until").and_then(Value::as_str) {
        None if value.get("ms").is_some() => Ok(Wait::For(duration)),
        Some("changed") => Ok(Wait::Changed(duration)),
        Some("settled") => Ok(Wait::Settled(duration)),
        _ => Err("wait needs ms or until: changed/settled".into()),
    }
}

pub fn key(name: &str) -> Result<Key, String> {
    use Named::{
        Alt, Backspace, CapsLock, Ctrl, Delete, Down, End, Enter, Escape, F, Home, Insert, Left,
        Menu, NumLock, PageDown, PageUp, Pause, PrintScreen, Right, ScrollLock, Shift, Space,
        Super, Tab, Up,
    };
    let lower = name.to_ascii_lowercase();
    let named = match lower.as_str() {
        "ctrl" | "control" => Ctrl,
        "shift" => Shift,
        "alt" => Alt,
        "super" | "win" | "meta" => Super,
        "enter" | "return" => Enter,
        "tab" => Tab,
        "escape" | "esc" => Escape,
        "backspace" => Backspace,
        "delete" | "del" => Delete,
        "insert" => Insert,
        "home" => Home,
        "end" => End,
        "pageup" | "page_up" => PageUp,
        "pagedown" | "page_down" => PageDown,
        "up" => Up,
        "down" => Down,
        "left" => Left,
        "right" => Right,
        "space" => Space,
        "capslock" => CapsLock,
        "numlock" => NumLock,
        "scrolllock" => ScrollLock,
        "printscreen" => PrintScreen,
        "pause" => Pause,
        "menu" => Menu,
        _ => {
            if let Some(n) = lower
                .strip_prefix('f')
                .and_then(|s| s.parse::<u8>().ok())
                .filter(|n| (1..=24).contains(n))
            {
                return Ok(Key::Named(F(n)));
            }
            let mut chars = lower.chars();
            if let (Some(c), None) = (chars.next(), chars.next()) {
                return Ok(Key::Char(c));
            }
            return Err(format!("unknown key {name}"));
        }
    };
    Ok(Key::Named(named))
}
