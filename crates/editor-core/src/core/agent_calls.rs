use crate::core::{
    ConnectionStatus, Core, HostId, Screen, SetSettingError, Settings, SettingsPage,
    config_file::ReadConfigError, store::RemoveHostError,
};
use artisan_api::{EditorCall, EditorReply, SettingOwner, setting_decl};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
enum EditorCallError {
    #[error("No paired host is called {name}.")]
    NoHost { name: String },
    #[error("remove host {host}")]
    Remove {
        host: HostId,
        #[source]
        source: RemoveHostError,
    },
    #[error("There is no setting {key}.")]
    UnknownSetting { key: String },
    #[error("{key} is a Forge setting, not one of this Editor's.")]
    ForgeSetting { key: String },
    #[error(
        "The Editor's settings.json could not be read, so {key} is unknown until it is recovered"
    )]
    Unreadable {
        key: String,
        #[source]
        source: std::sync::Arc<ReadConfigError>,
    },
    #[error("{key} in settings.json is ignored until it is corrected")]
    Stored {
        key: String,
        #[source]
        source: std::sync::Arc<crate::core::settings::StoredSettingError>,
    },
    #[error("save {key}")]
    Save {
        key: String,
        #[source]
        source: SetSettingError,
    },
    #[error("There is no settings page {page}; pages: {pages}.")]
    NoPage { page: String, pages: String },
}

impl Core {
    pub(super) fn editor_call(&mut self, host_id: &HostId, id: String, call: EditorCall) {
        let Some(client) = self.host_any(host_id).map(|host| host.client.clone()) else {
            return;
        };
        if matches!(call, EditorCall::HostAdd { .. }) {
            let reply = EditorReply::Error("Forges join through the account: enroll this Forge from the Editor's Account page.".to_owned());
            crate::core::runtime::runtime().spawn(async move {
                if let Err(error) = client.reply_editor_call(&id, &reply).await {
                    tracing::warn!(call = %id, "answering Forge's call: {}", ::diagnostics::chain(&error));
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
                    Ok(Err(error)) => EditorReply::Error(::diagnostics::chain(&error)),
                    Err(error) => EditorReply::Error(format!("The sign-in stopped: {error}")),
                };
                if let Err(error) = client.reply_editor_call(&id, &reply).await {
                    tracing::warn!(call = %id, "answering Forge's call: {}", ::diagnostics::chain(&error));
                }
            });
            return;
        }
        let reply = match self.run_call(host_id, call) {
            Ok(value) => EditorReply::Ok(value),
            Err(error) => EditorReply::Error(::diagnostics::chain(&error)),
        };
        crate::core::runtime::runtime().spawn(async move {
            if let Err(error) = client.reply_editor_call(&id, &reply).await {
                tracing::warn!(call = %id, "answering Forge's call: {}", ::diagnostics::chain(&error));
            }
        });
    }
    fn run_call(&mut self, from: &HostId, call: EditorCall) -> Result<Value, EditorCallError> {
        match call {
            EditorCall::HostsList => Ok(Value::Array(self.hosts().map(|(config, host)| json!({ "id": config.id.0, "name": config.name, "address": config.address, "kind": config.kind, "connected": host.status == ConnectionStatus::Live, "is_caller": &config.id == from })).collect())),
            EditorCall::HostRemove { host } => { let id = self.find_host(&host)?; self.remove_host(&id).map_err(|source| EditorCallError::Remove { host: id.clone(), source })?; Ok(json!({ "removed": id.0 })) },
            EditorCall::HostSwitch { host } => { let id = self.find_host(&host)?; self.navigate(Screen::NewThread { host: id.clone(), project: None }); Ok(json!({ "switched_to": id.0 })) },
            EditorCall::SettingGet { key } => {
                let decl = editor_setting(&key)?;
                if let Settings::Unreadable(source) = &self.settings {
                    return Err(EditorCallError::Unreadable { key, source: source.clone() });
                }
                if let Some(source) = self.settings.rejected(&key) {
                    return Err(EditorCallError::Stored { key, source: source.clone() });
                }
                Ok(self.settings.get(&key).cloned().unwrap_or_else(|| decl.default_value()))
            }
            EditorCall::SettingSet { key, value } => {
                editor_setting(&key)?;
                self.set_setting(&key, value.clone()).map_err(|source| EditorCallError::Save { key: key.clone(), source })?;
                Ok(json!({ "key": key, "value": value }))
            }
            EditorCall::OpenThread { thread } => { self.navigate(Screen::Thread { host: from.clone(), thread: thread.clone() }); Ok(json!({ "opened": thread })) },
            EditorCall::OpenSettings { page } => { let found = SettingsPage::from_name(&page).ok_or_else(|| { let names: Vec<_> = SettingsPage::ALL.iter().map(|(_, name)| *name).collect(); EditorCallError::NoPage { page: page.clone(), pages: names.join(", ") } })?; self.navigate(Screen::Settings(found)); Ok(json!({ "opened": page })) },
            EditorCall::HostAdd { .. } | EditorCall::McpSignIn { .. } => unreachable!(),
        }
    }
    fn find_host(&self, name: &str) -> Result<HostId, EditorCallError> {
        self.hosts()
            .map(|(config, _)| config)
            .find(|config| config.id.0 == name || config.name.eq_ignore_ascii_case(name))
            .map(|config| config.id.clone())
            .ok_or_else(|| EditorCallError::NoHost {
                name: name.to_owned(),
            })
    }
}
fn editor_setting(key: &str) -> Result<&'static artisan_api::SettingDecl, EditorCallError> {
    let decl = setting_decl(key).ok_or_else(|| EditorCallError::UnknownSetting {
        key: key.to_owned(),
    })?;
    if decl.owner != SettingOwner::Editor {
        return Err(EditorCallError::ForgeSetting {
            key: key.to_owned(),
        });
    }
    Ok(decl)
}
