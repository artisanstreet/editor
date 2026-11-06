use crate::core::{ConnectionStatus, Core, HostId, Screen, SettingsPage};
use anyhow::{Context as _, Result, anyhow, bail};
use artisan_api::{EditorCall, EditorReply, SettingOwner, setting_decl};
use serde_json::{Value, json};

impl Core {
    pub(super) fn editor_call(&mut self, host_id: &HostId, id: String, call: EditorCall) {
        let Some(client) = self.host_any(host_id).map(|host| host.client.clone()) else {
            return;
        };
        if matches!(call, EditorCall::HostAdd { .. }) {
            let reply = EditorReply::Error("Forges join through the account: enroll this Forge from the Editor's Account page.".to_owned());
            crate::core::runtime::runtime().spawn(async move {
                if let Err(error) = client.reply_editor_call(&id, &reply).await {
                    tracing::warn!(call = %id, "answering Forge's call: {error:#}");
                }
            });
            return;
        }
        if let EditorCall::McpSignIn {
            server,
            navigate_user,
        } = call
        {
            if navigate_user {
                self.navigate(Screen::Settings(SettingsPage::Extensions));
            }
            let started = self.start_mcp_sign_in(host_id, &server, navigate_user);
            crate::core::runtime::runtime().spawn(async move {
                let reply = match started.await {
                    Ok(Ok(url)) => EditorReply::Ok(json!({ "authorize_url": url })),
                    Ok(Err(error)) => EditorReply::Error(format!("{error:#}")),
                    Err(error) => EditorReply::Error(format!("The sign-in stopped: {error}")),
                };
                if let Err(error) = client.reply_editor_call(&id, &reply).await {
                    tracing::warn!(call = %id, "answering Forge's call: {error:#}");
                }
            });
            return;
        }
        let reply = match self.run_call(host_id, call) {
            Ok(value) => EditorReply::Ok(value),
            Err(error) => EditorReply::Error(format!("{error:#}")),
        };
        crate::core::runtime::runtime().spawn(async move {
            if let Err(error) = client.reply_editor_call(&id, &reply).await {
                tracing::warn!(call = %id, "answering Forge's call: {error:#}");
            }
        });
    }
    fn run_call(&mut self, from: &HostId, call: EditorCall) -> Result<Value> {
        match call {
            EditorCall::HostsList => Ok(Value::Array(self.hosts().map(|(config, host)| json!({ "id": config.id.0, "name": config.name, "address": config.address, "kind": config.kind, "connected": host.status == ConnectionStatus::Live, "is_caller": &config.id == from })).collect())),
            EditorCall::HostRemove { host } => { let id = self.find_host(&host)?; self.remove_host(&id)?; Ok(json!({ "removed": id.0 })) },
            EditorCall::HostSwitch { host } => { let id = self.find_host(&host)?; self.navigate(Screen::NewThread { host: id.clone(), project: None }); Ok(json!({ "switched_to": id.0 })) },
            EditorCall::SettingGet { key } => { let decl = editor_setting(&key)?; Ok(self.settings.get(&key).cloned().unwrap_or_else(|| decl.default_value())) },
            EditorCall::SettingSet { key, value } => { let decl = editor_setting(&key)?; decl.check(&value).map_err(|error| anyhow!(error))?; self.set_setting(&key, value.clone()); Ok(json!({ "key": key, "value": value })) },
            EditorCall::OpenThread { thread } => { self.navigate(Screen::Thread { host: from.clone(), thread: thread.clone() }); Ok(json!({ "opened": thread })) },
            EditorCall::OpenSettings { page } => { let found = SettingsPage::from_name(&page).with_context(|| { let names: Vec<_> = SettingsPage::ALL.iter().map(|(_, name)| *name).collect(); format!("There is no settings page {page}; pages: {}.", names.join(", ")) })?; self.navigate(Screen::Settings(found)); Ok(json!({ "opened": page })) },
            EditorCall::HostAdd { .. } | EditorCall::McpSignIn { .. } => unreachable!(),
        }
    }
    fn find_host(&self, name: &str) -> Result<HostId> {
        self.hosts()
            .map(|(config, _)| config)
            .find(|config| config.id.0 == name || config.name.eq_ignore_ascii_case(name))
            .map(|config| config.id.clone())
            .with_context(|| format!("No paired host is called {name}."))
    }
}
fn editor_setting(key: &str) -> Result<&'static artisan_api::SettingDecl> {
    let decl = setting_decl(key).with_context(|| format!("There is no setting {key}."))?;
    if decl.owner != SettingOwner::Editor {
        bail!("{key} is a Forge setting, not one of this Editor's.");
    }
    Ok(decl)
}
