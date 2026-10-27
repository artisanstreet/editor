//! What a Forge asks this Editor to do for an agent: the Artisan MCP tools
//! whose state lives here (hosts, this device's settings, what is on
//! screen). Each runs through the same code as the UI and answers the Forge
//! that asked.

use crate::host::HostData;
use crate::{ConnectionStatus, Core, HostId, Screen, SettingsPage};
use anyhow::{Context as _, Result, anyhow, bail};
use artisan_api::{EditorCall, EditorReply, SettingOwner, setting_decl};
use gpui::{Context, Entity};
use serde_json::{Value, json};

impl Core {
    pub(crate) fn editor_call(
        &mut self,
        host: &Entity<HostData>,
        id: String,
        call: EditorCall,
        cx: &mut Context<Self>,
    ) {
        let client = host.read(cx).client.clone();
        let host_id = host.read(cx).id.clone();
        let answer = |reply: EditorReply, cx: &mut Context<Self>| {
            let client = client.clone();
            let id = id.clone();
            cx.spawn(async move |_, _| {
                if let Err(error) = client.reply_editor_call(&id, &reply).await {
                    tracing::warn!(call = %id, "answering Forge's call: {error:#}");
                }
            })
            .detach();
        };
        // Forbidden legacy access: rejected before any pairing work. No `ae
        // pair` subprocess, no outgoing pair network, no persistence, no
        // connect. Enrollment is account-side; the reply guides there.
        if matches!(call, EditorCall::HostAdd { .. }) {
            return answer(
                EditorReply::Error(crate::DIRECT_PAIRING_DISABLED.to_owned()),
                cx,
            );
        }
        if let EditorCall::McpSignIn {
            server,
            navigate_user,
        } = call
        {
            // Only when the user asked to be taken there: the card shows the
            // sign-in and the browser opens on its page.
            if navigate_user {
                self.navigate(Screen::Settings(SettingsPage::Extensions), cx);
            }
            let started = self.start_mcp_sign_in(host, &server, navigate_user, cx);
            cx.spawn(async move |_, _| {
                let reply = match started.await {
                    Ok(url) => EditorReply::Ok(json!({ "authorize_url": url })),
                    Err(error) => EditorReply::Error(format!("{error:#}")),
                };
                if let Err(error) = client.reply_editor_call(&id, &reply).await {
                    tracing::warn!(call = %id, "answering Forge's call: {error:#}");
                }
            })
            .detach();
            return;
        }
        let reply = match self.run_call(&host_id, call, cx) {
            Ok(value) => EditorReply::Ok(value),
            Err(error) => EditorReply::Error(format!("{error:#}")),
        };
        answer(reply, cx);
    }

    fn run_call(
        &mut self,
        from: &HostId,
        call: EditorCall,
        cx: &mut Context<Self>,
    ) -> Result<Value> {
        // Account-only: host inventory is empty unless `Ready` (gated
        // inside `hosts()`); agent calls cannot bypass the gate.
        match call {
            EditorCall::HostsList => Ok(Value::Array(
                self.hosts()
                    .map(|(config, host)| {
                        let host = host.read(cx);
                        json!({
                            "id": config.id.0,
                            "name": config.name,
                            "address": config.address,
                            "kind": config.kind,
                            "connected": host.status == ConnectionStatus::Live,
                            "is_caller": &config.id == from,
                        })
                    })
                    .collect(),
            )),
            EditorCall::HostRemove { host } => {
                let id = self.find_host(&host)?;
                self.remove_host(&id, cx)?;
                Ok(json!({ "removed": id.0 }))
            }
            EditorCall::HostSwitch { host } => {
                let id = self.find_host(&host)?;
                self.navigate(
                    Screen::NewThread {
                        host: id.clone(),
                        project: None,
                    },
                    cx,
                );
                Ok(json!({ "switched_to": id.0 }))
            }
            EditorCall::SettingGet { key } => {
                let decl = editor_setting(&key)?;
                Ok(self
                    .settings
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| decl.default_value()))
            }
            EditorCall::SettingSet { key, value } => {
                let decl = editor_setting(&key)?;
                decl.check(&value).map_err(|error| anyhow!(error))?;
                self.set_setting(&key, value.clone(), cx);
                Ok(json!({ "key": key, "value": value }))
            }
            EditorCall::OpenThread { thread } => {
                self.navigate(
                    Screen::Thread {
                        host: from.clone(),
                        thread: thread.clone(),
                    },
                    cx,
                );
                Ok(json!({ "opened": thread }))
            }
            EditorCall::OpenSettings { page } => {
                let found = SettingsPage::from_name(&page).with_context(|| {
                    let names: Vec<_> = SettingsPage::ALL.iter().map(|(_, name)| *name).collect();
                    format!(
                        "There is no settings page {page}; pages: {}.",
                        names.join(", ")
                    )
                })?;
                self.navigate(Screen::Settings(found), cx);
                Ok(json!({ "opened": page }))
            }
            EditorCall::HostAdd { .. } => unreachable!("pairing runs apart"),
            EditorCall::McpSignIn { .. } => unreachable!("sign-ins run apart"),
        }
    }

    /// A host by id or by name, case-insensitively. Empty unless `Ready`.
    fn find_host(&self, host: &str) -> Result<HostId> {
        self.hosts()
            .map(|(config, _)| config)
            .find(|config| config.id.0 == host || config.name.eq_ignore_ascii_case(host))
            .map(|config| config.id.clone())
            .with_context(|| format!("No paired host is called {host}."))
    }
}

fn editor_setting(key: &str) -> Result<&'static artisan_api::SettingDecl> {
    let decl = setting_decl(key).with_context(|| format!("There is no setting {key}."))?;
    if decl.owner != SettingOwner::Editor {
        bail!("{key} is a Forge setting, not one of this Editor's.");
    }
    Ok(decl)
}
