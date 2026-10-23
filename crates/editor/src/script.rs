//! `ARTISAN_EDITOR_SCRIPT=<file>` (the `capture` feature): drives the
//! Editor's own window with synthetic input and saves what it paints, so
//! menus and hover states can be checked by eye without touching the real
//! mouse or keyboard. One step per line, coordinates in logical pixels:
//!
//! ```text
//! wait 500             # milliseconds
//! move 120 340         # hover
//! click 120 340        # left button down and up
//! rclick 120 340       # right button down and up
//! scroll 120 340 -200  # wheel, pixels (negative scrolls down)
//! type hello $         # the rest of the line, one character at a time
//! key ctrl-k           # one keystroke, as in a key binding
//! capture C:\out.png   # save the window
//! quit
//! ```
//!
//! Blank lines and lines starting with `#` are skipped. The window opens
//! without taking focus; `ARTISAN_EDITOR_WINDOW_ORIGIN=<x>,<y>` places it
//! (off screen keeps the real mouse from hovering it).

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, bail};
use gpui::{
    AnyWindowHandle, App, AppContext as _, AsyncApp, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformInput, Point, ScrollDelta,
    ScrollWheelEvent, TouchPhase, WindowBounds, WindowHandle, WindowOptions, point, px,
};

use crate::workspace::Workspace;

#[derive(Debug)]
enum Step {
    Wait(Duration),
    Move(Point<Pixels>),
    Click(Point<Pixels>, MouseButton),
    Scroll(Point<Pixels>, f32),
    Type(String),
    Key(Keystroke),
    Capture(PathBuf),
    Quit,
}

/// The script file named by `ARTISAN_EDITOR_SCRIPT`, if any.
pub fn path() -> Option<PathBuf> {
    std::env::var_os("ARTISAN_EDITOR_SCRIPT").map(PathBuf::from)
}

/// A scripted window doesn't take the user's focus; it opens where
/// `ARTISAN_EDITOR_WINDOW_ORIGIN=<x>,<y>` says, if set.
pub fn window_options(options: &mut WindowOptions) {
    options.focus = false;
    let origin = std::env::var("ARTISAN_EDITOR_WINDOW_ORIGIN")
        .ok()
        .and_then(|value| {
            let (x, y) = value.split_once(',')?;
            Some(point(
                px(finite_number(x.trim()).ok()?),
                px(finite_number(y.trim()).ok()?),
            ))
        });
    if let (Some(origin), Some(WindowBounds::Windowed(bounds))) =
        (origin, options.window_bounds.as_mut())
    {
        bounds.origin = origin;
    }
}

pub fn run(path: PathBuf, handle: WindowHandle<Workspace>, cx: &mut App) {
    let steps = match std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))
        .and_then(|text| parse(&text))
    {
        Ok(steps) => steps,
        Err(error) => {
            tracing::error!("couldn't read the script: {error:#}");
            std::process::exit(1);
        }
    };
    tracing::info!(steps = steps.len(), script = %path.display(), "running the script");
    // Input goes through the window like the platform's does, without
    // holding the root view: handlers may update the workspace.
    let target: AnyWindowHandle = handle.into();
    cx.spawn(async move |cx| {
        for (index, step) in steps.into_iter().enumerate() {
            tracing::info!(step = index + 1, ?step, "script step");
            let quit = matches!(step, Step::Quit);
            if let Err(error) = perform(step, target, cx).await {
                tracing::error!(step = index + 1, "script step failed: {error:#}");
                std::process::exit(1);
            }
            if quit {
                return;
            }
        }
        tracing::info!("script finished");
        cx.update(|cx| cx.quit());
    })
    .detach();
}

async fn perform(step: Step, target: AnyWindowHandle, cx: &mut AsyncApp) -> Result<()> {
    let modifiers = Modifiers::default();
    let dispatch = |events: Vec<PlatformInput>, cx: &mut AsyncApp| {
        cx.update_window(target, |_, window, cx| {
            for event in events {
                window.dispatch_event(event, cx);
            }
        })
    };
    match step {
        Step::Wait(duration) => {
            cx.background_executor().timer(duration).await;
            return Ok(());
        }
        Step::Move(position) => dispatch(vec![mouse_move(position, modifiers)], cx)?,
        Step::Click(position, button) => dispatch(
            vec![
                mouse_move(position, modifiers),
                PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count: 1,
                    first_mouse: false,
                }),
                PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count: 1,
                }),
            ],
            cx,
        )?,
        Step::Scroll(position, delta) => dispatch(
            vec![
                mouse_move(position, modifiers),
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.0), px(delta))),
                    modifiers,
                    touch_phase: TouchPhase::Moved,
                }),
            ],
            cx,
        )?,
        Step::Type(text) => cx.update_window(target, |_, window, cx| {
            for character in text.chars() {
                let key = character.to_string();
                window.dispatch_keystroke(
                    Keystroke {
                        modifiers: Modifiers::default(),
                        key: key.clone(),
                        key_char: Some(key),
                    },
                    cx,
                );
            }
        })?,
        Step::Key(keystroke) => {
            cx.update_window(target, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })?;
        }
        Step::Capture(path) => {
            let (scale, size) = cx.update_window(target, |_, window, _| {
                let image = window.render_to_image()?;
                image
                    .save(&path)
                    .with_context(|| format!("save {}", path.display()))?;
                anyhow::Ok((window.scale_factor(), image.dimensions()))
            })??;
            tracing::info!(path = %path.display(), scale, ?size, "captured the window");
            return Ok(());
        }
        Step::Quit => {
            cx.update(|cx| cx.quit());
            return Ok(());
        }
    }
    // Await a frame boundary instead of assuming a fixed machine speed.
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    cx.update_window(target, |_, window, _| {
        window.on_next_frame(move |_, _| {
            let _ = send.send(());
        });
        window.refresh();
    })?;
    cx.background_executor()
        .spawn(async move {
            receive
                .recv_timeout(Duration::from_secs(5))
                .context("window did not produce a frame")
        })
        .await?;
    Ok(())
}

fn mouse_move(position: Point<Pixels>, modifiers: Modifiers) -> PlatformInput {
    PlatformInput::MouseMove(MouseMoveEvent {
        position,
        pressed_button: None,
        modifiers,
    })
}

fn parse(text: &str) -> Result<Vec<Step>> {
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
    let position = || -> Result<Point<Pixels>> {
        match numbers()?.as_slice() {
            [x, y] => Ok(point(px(*x), px(*y))),
            _ => bail!("expected <x> <y>"),
        }
    };
    Ok(match command {
        "wait" => match numbers()?.as_slice() {
            [ms] if *ms >= 0.0 && *ms <= 3_600_000.0 => {
                Step::Wait(Duration::from_millis(*ms as u64))
            }
            _ => bail!("expected <milliseconds>"),
        },
        "move" => Step::Move(position()?),
        "click" => Step::Click(position()?, MouseButton::Left),
        "rclick" => Step::Click(position()?, MouseButton::Right),
        "scroll" => match numbers()?.as_slice() {
            [x, y, delta] => Step::Scroll(point(px(*x), px(*y)), *delta),
            _ => bail!("expected <x> <y> <pixels>"),
        },
        "type" => Step::Type(rest.to_owned()),
        "key" => {
            Step::Key(Keystroke::parse(rest.trim()).map_err(|error| anyhow::anyhow!("{error}"))?)
        }
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

#[cfg(test)]
mod tests {
    use super::parse_step;
    #[gpui::test]
    fn capture_failure_exit_child(cx: &mut gpui::TestAppContext) {
        if std::env::var_os("ARTISAN_REFORM_CAPTURE_FAILURE_CHILD").is_none() {
            return;
        }
        use gpui::AppContext as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.txt");
        let script = format!(
            "capture {}\nquit\n",
            dir.path().join("missing/capture.png").display()
        );
        assert!(super::parse(&script).is_ok());
        std::fs::write(&path, script).unwrap();
        let core = cx.new(|cx| {
            editor_core::Core::new(
                editor_core::EditorFiles {
                    dir: dir.path().join("preferences"),
                },
                cx,
            )
            .unwrap()
        });
        core.update(cx, |core, cx| {
            core.navigate(
                editor_core::Screen::Settings(editor_core::SettingsPage::General),
                cx,
            )
        });
        let (_, cx) = cx.add_window_view(|window, cx| {
            crate::workspace::Workspace::new(core.clone(), None, window, cx)
        });
        println!("executing real capture step in isolated test window");
        cx.update(|window, cx| {
            super::run(
                path,
                window
                    .window_handle()
                    .downcast::<crate::workspace::Workspace>()
                    .unwrap(),
                cx,
            )
        });
        cx.run_until_parked();
        panic!("a failed real capture step must exit before a later quit step");
    }

    #[test]
    fn failed_capture_step_exits_nonzero_instead_of_reporting_success() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "script::tests::capture_failure_exit_child",
                "--nocapture",
            ])
            .env("ARTISAN_REFORM_CAPTURE_FAILURE_CHILD", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("owned capture test child did not exit on failure");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "actual script failure must exit1, not test panic or successful quit: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout)
                .contains("executing real capture step in isolated test window")
        );
    }

    #[test]
    fn rejects_non_finite_script_input() {
        for line in [
            "move NaN 1",
            "click 1 inf",
            "scroll 1 2 -inf",
            "wait inf",
            "wait 3600001",
        ] {
            assert!(parse_step(line).is_err(), "{line}");
        }
    }
}
