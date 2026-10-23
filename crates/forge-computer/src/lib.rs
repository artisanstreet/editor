//! App-independent computer use, served by Forge's authenticated MCP gateway.
pub mod desktop;
pub mod frame;
mod operations;
pub mod runner;
pub mod steps;
mod tools;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

use anyhow::{Result, bail};
use artisan_api::ThreadId;
use desktop::{Desktop, Rect, WindowId};
use forge_mcp::{Content, Tool, ToolSet};
use forge_runs::{Hub, Refused};
use runner::View;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

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
    /// Available only when the host has an explicitly configured agent display.
    pub fn for_host(hub: &Hub) -> Option<Self> {
        #[cfg(windows)]
        {
            match windows::WindowsDesktop::new() {
                Ok(desktop) => Some(Self::with_desktop(hub.clone(), Arc::new(desktop))),
                Err(error) => {
                    tracing::info!("computer use unavailable: {error:#}");
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
    async fn enabled(&self) -> Result<()> {
        if self
            .hub
            .store()
            .setting(forge_runs::COMPUTER_USE_KEY)
            .await?
            .and_then(|v| v.as_bool())
            != Some(true)
        {
            return Err(Refused(
                "Computer use is off. Enable it in Artisan settings first.".into(),
            )
            .into());
        }
        Ok(())
    }
    fn snapshot(&self, session: &mut Session, caller: &ThreadId) -> Result<Vec<Content>> {
        let view = View::new(self.desktop.agent_display()?);
        let frame = self
            .desktop
            .capture(view.rect)?
            .shrink(view.width, view.height);
        let cursor = self.desktop.cursor()?;
        session.views.insert(caller.clone(), view);
        Ok(vec![Content::Text(json!({"width":view.width,"height":view.height,
            "cursor": if view.rect.contains(cursor) { Some([f64::from(cursor.x-view.rect.x)*f64::from(view.width)/f64::from(view.rect.width), f64::from(cursor.y-view.rect.y)*f64::from(view.height)/f64::from(view.rect.height)]) } else { None },
            "focused_window":self.desktop.foreground(),"paused":self.paused.load(Ordering::SeqCst)}).to_string()),
            Content::Image { mime_type: "image/png", data: frame.png_base64()? }])
    }
    fn execute(&self, caller: &ThreadId, name: &str, args: &Value) -> Result<Vec<Content>> {
        let mut session = self.session.try_lock().map_err(|_| {
            Refused("Another computer tool is running. Try after it finishes.".into())
        })?;
        let area = self.desktop.agent_display()?;
        match name {
            "screenshot" => self.snapshot(&mut session, caller),
            "zoom" => self.zoom(&session, caller, args, area),
            "windows" => self.list_windows(args, area),
            "input" => self.input(&mut session, caller, args),
            "window" => self.change_window(&mut session, caller, args, area),
            "launch" => self.launch(args, area),
            "clipboard" => match string(args, "action")? {
                "read" => text(&json!({"text":self.desktop.clipboard()?})),
                "write" => {
                    self.desktop.set_clipboard(string(args, "text")?)?;
                    text(&json!({"ok":true}))
                }
                _ => bail!("clipboard action must be read or write"),
            },
            _ => bail!("unknown computer tool"),
        }
    }
}
fn string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| Refused(format!("{key} must be text")).into())
}
fn number(args: &Value, key: &str) -> Result<u32> {
    args[key]
        .as_u64()
        .and_then(|n| n.try_into().ok())
        .ok_or_else(|| Refused(format!("{key} must be a nonnegative integer")).into())
}
impl ToolSet for ComputerMcp {
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
                .await?;
            self.enabled().await?;
            let this = self.clone();
            tokio::task::spawn_blocking(move || -> Result<()> {
                let mut session = this
                    .session
                    .lock()
                    .map_err(|_| anyhow::anyhow!("computer state lock poisoned"))?;
                session.views.clear();
                this.paused.store(false, Ordering::SeqCst);
                Ok(())
            })
            .await??;
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
                .await?;
            self.enabled().await?;
        }
        let this = self.clone();
        let caller = caller.clone();
        let name = name.to_owned();
        tokio::task::spawn_blocking(move || this.execute(&caller, &name, &args)).await?
    }
}

#[cfg(test)]
mod tests;

fn text(value: &Value) -> Result<Vec<Content>> {
    Ok(vec![Content::Text(serde_json::to_string(value)?)])
}
