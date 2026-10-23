//! What a Forge asks this Editor to do for an agent: the Artisan MCP tools
//! whose state lives here (hosts, this device's settings, what is on
//! screen). Each runs through the same code as the UI and answers the Forge
//! that asked.

use crate::host::HostData;
use crate::hosts::{self, HostKind, LocalTarget};
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
        if let EditorCall::HostAdd {
            kind,
            distro,
            address,
            code,
            fingerprint,
        } = call
        {
            let pairing = self.start_agent_pairing(&kind, distro, address, code, fingerprint);
            let pairing = match pairing {
                Ok(pairing) => pairing,
                Err(error) => return answer(EditorReply::Error(format!("{error:#}")), cx),
            };
            cx.spawn(async move |this, cx| {
                let paired = pairing.await;
                let reply = this
                    .update(cx, |core, cx| {
                        let config = paired?;
                        let summary = json!({ "id": config.id.0, "name": config.name, "address": config.address });
                        core.add_host(config, cx)?;
                        anyhow::Ok(summary)
                    })
                    .map_err(|error| anyhow!("{error:#}"))
                    .and_then(|added| added);
                let reply = match reply {
                    Ok(added) => EditorReply::Ok(added),
                    Err(error) => EditorReply::Error(format!("Couldn't pair: {error:#}")),
                };
                if let Err(error) = client.reply_editor_call(&id, &reply).await {
                    tracing::warn!(call = %id, "answering Forge's call: {error:#}");
                }
            })
            .detach();
            return;
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

    /// A host by id or by name, case-insensitively.
    fn find_host(&self, host: &str) -> Result<HostId> {
        self.hosts()
            .map(|(config, _)| config)
            .find(|config| config.id.0 == host || config.name.eq_ignore_ascii_case(host))
            .map(|config| config.id.clone())
            .with_context(|| format!("No paired host is called {host}."))
    }

    fn start_agent_pairing(
        &self,
        kind: &str,
        distro: Option<String>,
        address: Option<String>,
        code: Option<String>,
        fingerprint: Option<String>,
    ) -> Result<impl std::future::Future<Output = Result<crate::HostConfig>> + 'static> {
        let identity = self.identity.clone().context(
            "This device's identity could not be loaded; repair its configuration before pairing.",
        )?;
        let device = self.device_name.clone();
        let target = match kind {
            "this_computer" => Ok(LocalTarget::ThisMachine),
            "wsl" => Ok(LocalTarget::Wsl {
                distro: distro.context("A WSL host needs its distro.")?,
            }),
            "remote" => Err((
                address.context("A remote host needs its address (host:port).")?,
                code.context("A remote host needs the pairing code `ae pair` shows on it.")?,
                fingerprint
                    .context("A remote host needs the fingerprint `ae pair` shows on it.")?,
            )),
            other => bail!("A host is this_computer, wsl or remote, not {other}."),
        };
        Ok(async move {
            match target {
                Ok(target) => hosts::pair_local(target, &identity, &device).await,
                Err((address, code, fingerprint)) => {
                    let name = address
                        .rsplit_once(':')
                        .map_or(address.as_str(), |(host, _)| host)
                        .to_owned();
                    hosts::pair(
                        &address,
                        &code,
                        &fingerprint,
                        &identity,
                        &device,
                        name,
                        HostKind::Remote,
                    )
                    .await
                }
            }
        })
    }
}

fn editor_setting(key: &str) -> Result<&'static artisan_api::SettingDecl> {
    let decl = setting_decl(key).with_context(|| format!("There is no setting {key}."))?;
    if decl.owner != SettingOwner::Editor {
        bail!("{key} is a Forge setting, not one of this Editor's.");
    }
    Ok(decl)
}
