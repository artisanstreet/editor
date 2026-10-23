//! Display operations, called while the computer session is locked.
use super::{ComputerMcp, Session, number, string, text};
use crate::{
    desktop::{Rect, WindowChange},
    frame,
    runner::{self, View},
    steps,
};
use anyhow::{Result, bail};
use artisan_api::ThreadId;
use forge_mcp::Content;
use forge_runs::Refused;
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

impl ComputerMcp {
    pub(super) fn zoom(
        &self,
        session: &Session,
        caller: &ThreadId,
        args: &Value,
        area: Rect,
    ) -> Result<Vec<Content>> {
        let view = *session
            .views
            .get(caller)
            .ok_or_else(|| Refused("Take a screenshot first.".into()))?;
        if view.rect != area {
            bail!("display changed; take a new screenshot");
        }
        let x = number(args, "x")?;
        let y = number(args, "y")?;
        let width = number(args, "width")?;
        let height = number(args, "height")?;
        if width == 0
            || height == 0
            || x.checked_add(width).is_none_or(|n| n > view.width)
            || y.checked_add(height).is_none_or(|n| n > view.height)
        {
            bail!("zoom rectangle is outside the screenshot");
        }
        let point = view.point(steps::At {
            x: f64::from(x),
            y: f64::from(y),
        })?;
        let right = u64::from(x + width) * u64::from(area.width) / u64::from(view.width);
        let bottom = u64::from(y + height) * u64::from(area.height) / u64::from(view.height);
        let crop = Rect {
            x: point.x,
            y: point.y,
            width: u32::try_from(right)? - u32::try_from(point.x - area.x)?,
            height: u32::try_from(bottom)? - u32::try_from(point.y - area.y)?,
        };
        let frame = self.desktop.capture(crop)?;
        let scale = frame::scale_for(frame.width, frame.height);
        let frame = frame.shrink(
            frame::scaled(frame.width, scale),
            frame::scaled(frame.height, scale),
        );
        Ok(vec![Content::Text(json!({"region":[x,y,width,height],"width":frame.width,"height":frame.height,"input_coordinates":"use the original screenshot coordinates"}).to_string()),Content::Image { mime_type:"image/png", data:frame.png_base64()? }])
    }
    pub(super) fn list_windows(&self, args: &Value, area: Rect) -> Result<Vec<Content>> {
        let view = View::new(area);
        let focused = self.desktop.foreground();
        let mut windows = Vec::new();
        for window in self.desktop.windows()? {
            let on_display = area.contains(window.rect.center());
            if !on_display && args["include_user"].as_bool() != Some(true) {
                continue;
            }
            let sx = f64::from(view.width) / f64::from(area.width);
            let sy = f64::from(view.height) / f64::from(area.height);
            windows.push(json!({
                "id":window.id, "title":window.title, "process":window.process,
                "pid":window.pid, "focused":focused == Some(window.id),
                "on_agent_display":on_display,
                "bounds":[f64::from(window.rect.x-area.x)*sx, f64::from(window.rect.y-area.y)*sy,
                    f64::from(window.rect.width)*sx, f64::from(window.rect.height)*sy]
            }));
        }
        text(&json!(windows))
    }
    pub(super) fn input(
        &self,
        session: &mut Session,
        caller: &ThreadId,
        args: &Value,
    ) -> Result<Vec<Content>> {
        if self.paused.load(Ordering::SeqCst) {
            return Err(Refused(
                "Computer input is paused. Use handoff and wait for the user to resume.".into(),
            )
            .into());
        }
        let view = *session
            .views
            .get(caller)
            .ok_or_else(|| Refused("Take a screenshot before input.".into()))?;
        let steps = steps::parse(&args["steps"])?;
        let result = runner::run(self.desktop.as_ref(), view, &steps, &self.paused)?;
        if result.interrupted {
            self.paused.store(true, Ordering::SeqCst);
        }
        let mut content = vec![Content::Text(json!({"completed_steps":result.completed,"total_steps":steps.len(),"interrupted":result.interrupted,"screen_changed":result.screen_changed,"settled":result.settled,"focused_window":self.desktop.foreground()}).to_string())];
        if result.interrupted || args["screenshot"].as_bool() == Some(true) {
            content.extend(self.snapshot(session, caller)?);
        }
        Ok(content)
    }
    pub(super) fn change_window(
        &self,
        session: &mut Session,
        caller: &ThreadId,
        args: &Value,
        area: Rect,
    ) -> Result<Vec<Content>> {
        let id = args["id"]
            .as_u64()
            .ok_or_else(|| Refused("id must be a window id".into()))?;
        let action = string(args, "action")?;
        let window = self
            .desktop
            .windows()?
            .into_iter()
            .find(|w| w.id == id)
            .ok_or_else(|| Refused("Window no longer exists.".into()))?;
        if self.paused.load(Ordering::SeqCst) {
            bail!("computer input is paused; use handoff to resume");
        }
        if self.desktop.user_idle() < std::time::Duration::from_millis(500) {
            bail!("user is active; window was not changed");
        }
        let change = match action {
            "bring_here" => {
                session
                    .borrowed
                    .entry((caller.clone(), id))
                    .or_insert(window.rect);
                WindowChange::Place(Rect {
                    width: window.rect.width.min(area.width),
                    height: window.rect.height.min(area.height),
                    ..area
                })
            }
            "send_back" => WindowChange::Place(
                *session
                    .borrowed
                    .get(&(caller.clone(), id))
                    .ok_or_else(|| Refused("This thread did not borrow that window.".into()))?,
            ),
            _ if !area.contains(window.rect.center()) => {
                bail!("window is outside the agent display; use bring_here")
            }
            "focus" => WindowChange::Focus,
            "close" => WindowChange::Close,
            "maximize" => WindowChange::Maximize,
            _ => bail!("unknown window action"),
        };
        self.desktop.change_window(id, change)?;
        if action == "send_back" {
            session.borrowed.remove(&(caller.clone(), id));
        }
        text(&json!({"ok":true}))
    }
    pub(super) fn launch(&self, args: &Value, area: Rect) -> Result<Vec<Content>> {
        if self.paused.load(Ordering::SeqCst) {
            bail!("computer input is paused; use handoff to resume");
        }
        let command = string(args, "command")?;
        let arguments = args
            .get("args")
            .map(|v| {
                v.as_array()
                    .ok_or_else(|| Refused("args must be a list".into()))
            })
            .transpose()?
            .map(|a| {
                a.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| Refused("args must contain strings".into()))
                    })
                    .collect::<std::result::Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        let pid =
            self.desktop
                .launch(command, &arguments, args.get("cwd").and_then(Value::as_str))?;
        let generation = self.desktop.user_activity();
        let start = std::time::Instant::now();
        let mut windows = Vec::new();
        while start.elapsed() < std::time::Duration::from_secs(3) {
            if self.desktop.user_activity() != generation {
                self.paused.store(true, Ordering::SeqCst);
                break;
            }
            if self.paused.load(Ordering::SeqCst) {
                break;
            }
            let processes = self.desktop.processes()?;
            let mut pids = std::collections::HashSet::from([pid]);
            loop {
                let before = pids.len();
                for (child, parent) in &processes {
                    if pids.contains(parent) {
                        pids.insert(*child);
                    }
                }
                if before == pids.len() {
                    break;
                }
            }
            for window in self
                .desktop
                .windows()?
                .iter()
                .filter(|w| pids.contains(&w.pid))
            {
                if !windows.contains(&window.id) {
                    self.desktop.change_window(
                        window.id,
                        WindowChange::Place(Rect {
                            width: window.rect.width.min(area.width),
                            height: window.rect.height.min(area.height),
                            ..area
                        }),
                    )?;
                    windows.push(window.id);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        text(&json!({"pid":pid,"windows":windows,"paused":self.paused.load(Ordering::SeqCst)}))
    }
}
