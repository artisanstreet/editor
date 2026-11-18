#![expect(
    clippy::let_underscore_untyped,
    clippy::literal_string_with_formatting_args,
    clippy::renamed_function_params,
    missing_debug_implementations,
    unreachable_pub,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::allow_attributes,
    clippy::allow_attributes_without_reason,
    clippy::assertions_on_result_states,
    clippy::borrow_as_ptr,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::enum_glob_use,
    clippy::let_underscore_must_use,
    clippy::undocumented_unsafe_blocks,
    clippy::unreadable_literal,
    clippy::wildcard_imports,
    let_underscore_drop,
    unused_qualifications,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
pub mod desktop;
pub mod frame;
mod operations;
pub mod runner;
pub mod steps;
mod tools;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

use artisan_api::{Envelope, Event, ThreadId};
use desktop::{
    CaptureError, ClipboardError, CursorError, Desktop, DisplayError, LaunchError, ProcessesError,
    Rect, WindowError, WindowId,
};
use forge_mcp::{Content, Failure, Tool, ToolError, ToolSet};
use forge_runs::{Hub, Refused};
use frame::EncodeError;
use runner::{Halt, Halted, RunError, StepError, View};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, PoisonError, TryLockError,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::broadcast::{self, error::RecvError};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum ComputerError {
    #[error(transparent)]
    Refused(#[from] Refused),
    #[error(transparent)]
    Halted(#[from] Halted),
    #[error("read the computer use setting")]
    Setting(#[source] anyhow::Error),
    #[error("ask the user to take over from thread {thread}")]
    Handoff {
        thread: ThreadId,
        #[source]
        source: anyhow::Error,
    },
    #[error("ask approval for computer {tool} in thread {thread}")]
    Approval {
        tool: String,
        thread: ThreadId,
        #[source]
        source: anyhow::Error,
    },
    #[error("run computer {tool}")]
    Worker {
        tool: String,
        #[source]
        source: tokio::task::JoinError,
    },
    #[error("unknown computer tool {tool}")]
    UnknownTool { tool: String },
    #[error("find the agent display")]
    Display(#[source] DisplayError),
    #[error("capture the agent display")]
    Capture(#[source] CaptureError),
    #[error("read the cursor")]
    Cursor(#[source] CursorError),
    #[error("encode the screenshot")]
    Encode(#[source] EncodeError),
    #[error("list the windows")]
    Windows(#[source] WindowError),
    #[error("change window {window}")]
    Window {
        window: WindowId,
        #[source]
        source: WindowError,
    },
    #[error("launch the program")]
    Launch(#[source] LaunchError),
    #[error("list the processes")]
    Processes(#[source] ProcessesError),
    #[error("read the clipboard")]
    ReadClipboard(#[source] ClipboardError),
    #[error("write the clipboard")]
    WriteClipboard(#[source] ClipboardError),
    #[error("run the input batch")]
    Run(#[source] RunError),
    #[error("display changed; take a new screenshot")]
    DisplayChanged,
    #[error("zoom rectangle is outside the screenshot")]
    ZoomOutside,
    #[error("computer input is paused; use handoff to resume")]
    Paused,
    #[error("user is active; window {window} was not changed")]
    UserActive { window: WindowId },
    #[error("window {window} is outside the agent display; use bring_here")]
    WindowOutside { window: WindowId },
    #[error("unknown window action {action}")]
    WindowAction { action: String },
    #[error("clipboard action must be read or write, not {action}")]
    ClipboardAction { action: String },
    #[error("encode the result")]
    Json(#[source] serde_json::Error),
}

impl ToolError for ComputerError {
    fn failure(&self) -> Failure<'_> {
        match self {
            Self::Refused(refused) => Failure::Refused(refused.to_string()),
            Self::Halted(_)
            | Self::Run(RunError::Stopped {
                source: StepError::Halted(_),
                ..
            }) => Failure::Refused(diagnostics::chain(self)),
            Self::Setting(source)
            | Self::Handoff { source, .. }
            | Self::Approval { source, .. } => Failure::of_forge(source),
            Self::Worker { .. }
            | Self::UnknownTool { .. }
            | Self::Display(_)
            | Self::Capture(_)
            | Self::Cursor(_)
            | Self::Encode(_)
            | Self::Windows(_)
            | Self::Window { .. }
            | Self::Launch(_)
            | Self::Processes(_)
            | Self::ReadClipboard(_)
            | Self::WriteClipboard(_)
            | Self::Run(_)
            | Self::DisplayChanged
            | Self::ZoomOutside
            | Self::Paused
            | Self::UserActive { .. }
            | Self::WindowOutside { .. }
            | Self::WindowAction { .. }
            | Self::ClipboardAction { .. }
            | Self::Json(_) => Failure::Broken,
        }
    }
}

type Result<T, E = ComputerError> = std::result::Result<T, E>;

#[derive(Default)]
struct Session {
    views: HashMap<ThreadId, View>,
    borrowed: HashMap<(ThreadId, WindowId), Rect>,
}

#[derive(Clone)]
pub struct ComputerMcp {
    hub: Hub,
    desktop: Arc<dyn Desktop>,
    session: Arc<Mutex<Session>>,
    paused: Arc<AtomicBool>,
}
impl ComputerMcp {
    pub fn for_host(hub: &Hub) -> Option<Self> {
        #[cfg(windows)]
        {
            match windows::WindowsDesktop::new() {
                Ok(desktop) => Some(Self::with_desktop(hub.clone(), Arc::new(desktop))),
                Err(error) => {
                    tracing::info!("computer use unavailable: {}", diagnostics::chain(&error));
                    None
                }
            }
        }
        #[cfg(not(windows))]
        {
            let _ = hub;
            None
        }
    }
    pub fn with_desktop(hub: Hub, desktop: Arc<dyn Desktop>) -> Self {
        Self {
            hub,
            desktop,
            session: Arc::default(),
            paused: Arc::default(),
        }
    }
    async fn computer_use(&self) -> Result<bool> {
        Ok(self
            .hub
            .store()
            .setting(forge_runs::COMPUTER_USE_KEY)
            .await
            .map_err(|source| ComputerError::Setting(source.into()))?
            .and_then(|v| v.as_bool())
            == Some(true))
    }
    async fn enabled(&self) -> Result<()> {
        if !self.computer_use().await? {
            return Err(Refused(
                "Computer use is off. Enable it in Artisan settings first.".into(),
            )
            .into());
        }
        Ok(())
    }
    fn snapshot(&self, session: &mut Session, caller: &ThreadId) -> Result<Vec<Content>> {
        let view = View::new(
            self.desktop
                .agent_display()
                .map_err(ComputerError::Display)?,
        );
        let frame = self
            .desktop
            .capture(view.rect)
            .map_err(ComputerError::Capture)?
            .shrink(view.width, view.height);
        let cursor = self.desktop.cursor().map_err(ComputerError::Cursor)?;
        session.views.insert(caller.clone(), view);
        let (width, height) = (view.width.get(), view.height.get());
        Ok(vec![Content::Text(json!({"width":width,"height":height,
            "cursor": view.rect.contains(cursor).then(|| [f64::from(cursor.x-view.rect.x)*f64::from(width)/f64::from(view.rect.width), f64::from(cursor.y-view.rect.y)*f64::from(height)/f64::from(view.rect.height)]),
            "focused_window":self.desktop.foreground(),"paused":self.paused.load(Ordering::SeqCst)}).to_string()),
            Content::Image { mime_type: "image/png", data: frame.png_base64().map_err(ComputerError::Encode)? }])
    }
    async fn turned_off(&self, settings: &mut broadcast::Receiver<Envelope>) {
        loop {
            match settings.recv().await {
                Ok(Envelope {
                    event: Event::Setting { key, value },
                    ..
                }) if key == forge_runs::COMPUTER_USE_KEY => {
                    if value.as_bool() != Some(true) {
                        return;
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(missed)) => match self.computer_use().await {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => {
                        tracing::warn!(
                            missed,
                            "recheck computer use after missed events: {}",
                            diagnostics::chain(&error)
                        );
                        return;
                    }
                },
                Err(RecvError::Closed) => return,
            }
        }
    }
    async fn halted(
        &self,
        stop: &CancellationToken,
        settings: &mut broadcast::Receiver<Envelope>,
    ) -> Halted {
        tokio::select! {
            () = stop.cancelled() => Halted::RunStopped,
            () = self.turned_off(settings) => Halted::ComputerUseOff,
        }
    }
    fn execute(
        &self,
        caller: &ThreadId,
        name: &str,
        args: &Value,
        halt: &Halt,
    ) -> Result<Vec<Content>> {
        halt.check()?;
        let mut session = match self.session.try_lock() {
            Ok(session) => session,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                return Err(Refused(
                    "Another computer tool is running. Try after it finishes.".into(),
                )
                .into());
            }
        };
        let area = self
            .desktop
            .agent_display()
            .map_err(ComputerError::Display)?;
        match name {
            "screenshot" => self.snapshot(&mut session, caller),
            "zoom" => self.zoom(&session, caller, args, area),
            "windows" => self.list_windows(args, area),
            "input" => self.input(&mut session, caller, args, halt),
            "window" => self.change_window(&mut session, caller, args, area),
            "launch" => self.launch(args, area, halt),
            "clipboard" => match string(args, "action")? {
                "read" => text(&json!({
                    "text": self.desktop.clipboard().map_err(ComputerError::ReadClipboard)?
                })),
                "write" => {
                    self.desktop
                        .set_clipboard(string(args, "text")?)
                        .map_err(ComputerError::WriteClipboard)?;
                    text(&json!({"ok":true}))
                }
                action => Err(ComputerError::ClipboardAction {
                    action: action.to_owned(),
                }),
            },
            tool => Err(ComputerError::UnknownTool {
                tool: tool.to_owned(),
            }),
        }
    }
}
fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str, Refused> {
    args[key]
        .as_str()
        .ok_or_else(|| Refused(format!("{key} must be text")))
}
fn number(args: &Value, key: &str) -> Result<u32, Refused> {
    args[key]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| Refused(format!("{key} must be a nonnegative integer")))
}
impl ToolSet for ComputerMcp {
    type Error = ComputerError;

    fn name(&self) -> &'static str {
        "computer"
    }
    fn title(&self) -> &'static str {
        "Artisan Computer Use"
    }
    fn instructions(&self) -> &'static str {
        "One display, the same tools for every app. Take screenshot before input. Input coordinates refer to the full screenshot, never zoom pixels. Batches release held keys/buttons at the end. On interruption inspect the screenshot and use handoff; never repeat a batch blindly. A virtual monitor shares the user's keyboard and cursor. Physical input pauses automation. UAC and elevated windows require user handoff. Clipboard is shared with the user. Launch places new process windows for three seconds; windows created later may need bring_here."
    }
    fn tools(&self) -> Vec<Tool> {
        tools::list()
    }
    async fn call(&self, caller: &ThreadId, name: &str, args: Value) -> Result<Vec<Content>> {
        let stop = self
            .hub
            .run_cancellation(caller)
            .ok_or(Halted::RunStopped)?;
        self.enabled().await?;
        if name == "handoff" {
            self.paused.store(true, Ordering::SeqCst);
            self.hub
                .ask_user_on(
                    "computer",
                    caller,
                    name,
                    format!(
                        "Computer use paused: {}. Allow to resume.",
                        string(&args, "reason")?
                    ),
                )
                .await
                .map_err(|source| ComputerError::Handoff {
                    thread: caller.clone(),
                    source,
                })?;
            self.enabled().await?;
            let this = self.clone();
            tokio::task::spawn_blocking(move || {
                this.session
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .views
                    .clear();
                this.paused.store(false, Ordering::SeqCst);
            })
            .await
            .map_err(|source| ComputerError::Worker {
                tool: name.to_owned(),
                source,
            })?;
            return Ok(vec![Content::Text(
                "User resumed computer use. Take a fresh screenshot.".into(),
            )]);
        }
        if name == "windows" && args["include_user"].as_bool() == Some(true)
            || name == "window" && args["action"] == "bring_here"
            || name == "clipboard"
            || name == "launch"
        {
            self.hub
                .ask_approval_on("computer", caller, name, format!("Computer {name}: {args}"))
                .await
                .map_err(|source| ComputerError::Approval {
                    tool: name.to_owned(),
                    thread: caller.clone(),
                    source,
                })?;
        }
        let mut settings = self.hub.store().subscribe();
        self.enabled().await?;
        let halt = Arc::new(Halt::default());
        let _withdraw = WithdrawOnDrop(Arc::clone(&halt));
        let this = self.clone();
        let caller = caller.clone();
        let tool = name.to_owned();
        let worker_halt = Arc::clone(&halt);
        let mut worker =
            tokio::task::spawn_blocking(move || this.execute(&caller, &tool, &args, &worker_halt));
        let joined = tokio::select! {
            joined = &mut worker => joined,
            why = self.halted(&stop, &mut settings) => {
                halt.halt(why);
                worker.await
            }
        };
        joined.map_err(|source| ComputerError::Worker {
            tool: name.to_owned(),
            source,
        })?
    }
}

struct WithdrawOnDrop(Arc<Halt>);

impl Drop for WithdrawOnDrop {
    fn drop(&mut self) {
        self.0.halt(Halted::Withdrawn);
    }
}

#[cfg(test)]
mod tests;

fn text(value: &Value) -> Result<Vec<Content>> {
    Ok(vec![Content::Text(
        serde_json::to_string(value).map_err(ComputerError::Json)?,
    )])
}
