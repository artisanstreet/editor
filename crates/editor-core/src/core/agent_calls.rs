use crate::core::settings_writer::Settled;
use crate::core::{
    ConnectionStatus, Core, ForgeClient, HostId, Screen, SetSettingError, Settings, SettingsPage,
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
    #[error("save {key}")]
    NotSaved {
        key: String,
        #[source]
        source: std::sync::Arc<SetSettingError>,
    },
    #[error("There is no settings page {page}; pages: {pages}.")]
    NoPage { page: String, pages: String },
}

fn answer(client: ForgeClient, id: String, reply: EditorReply) {
    crate::core::runtime::runtime().spawn(async move {
        send_answer(&client, &id, &reply).await;
    });
}

async fn send_answer(client: &ForgeClient, id: &str, reply: &EditorReply) {
    if let Err(error) = client.reply_editor_call(id, reply).await {
        tracing::warn!(call = %id, "answering Forge's call: {}", ::diagnostics::chain(&error));
    }
}

pub(crate) struct SettingCall {
    revision: u64,
    client: ForgeClient,
    id: String,
    key: String,
    value: Value,
}

enum ImmediateCall {
    HostsList,
    HostRemove { host: String },
    HostSwitch { host: String },
    SettingGet { key: String },
    OpenThread { thread: artisan_api::ThreadId },
    OpenSettings { page: String },
}

impl Core {
    pub(super) fn editor_call(&mut self, host_id: &HostId, id: String, call: EditorCall) {
        let Some(client) = self.host_any(host_id).map(|host| host.client.clone()) else {
            return;
        };
        let call = match call {
            EditorCall::HostAdd { .. } => {
                answer(
                    client,
                    id,
                    EditorReply::Error(
                        "Forges join through the account: enroll this Forge from the Editor's Account page."
                            .to_owned(),
                    ),
                );
                return;
            }
            EditorCall::McpSignIn {
                server,
                navigate_user,
            } => {
                self.mcp_sign_in_call(host_id, client, id, &server, navigate_user);
                return;
            }
            EditorCall::HostsList => ImmediateCall::HostsList,
            EditorCall::HostRemove { host } => ImmediateCall::HostRemove { host },
            EditorCall::HostSwitch { host } => ImmediateCall::HostSwitch { host },
            EditorCall::SettingGet { key } => ImmediateCall::SettingGet { key },
            EditorCall::SettingSet { key, value } => {
                self.setting_set_call(client, id, key, value);
                return;
            }
            EditorCall::OpenThread { thread } => ImmediateCall::OpenThread { thread },
            EditorCall::OpenSettings { page } => ImmediateCall::OpenSettings { page },
        };
        let reply = match self.run_call(host_id, call) {
            Ok(value) => EditorReply::Ok(value),
            Err(error) => EditorReply::Error(::diagnostics::chain(&error)),
        };
        answer(client, id, reply);
    }
    fn setting_set_call(&mut self, client: ForgeClient, id: String, key: String, value: Value) {
        let revision = editor_setting(&key).and_then(|_| {
            self.put_setting(&key, value.clone())
                .map_err(|source| EditorCallError::Save {
                    key: key.clone(),
                    source,
                })
        });
        match revision {
            Ok(Some(revision)) => self.setting_calls.push(SettingCall {
                revision,
                client,
                id,
                key,
                value,
            }),
            Ok(None) => answer(
                client,
                id,
                EditorReply::Ok(json!({ "key": key, "value": value })),
            ),
            Err(error) => answer(client, id, EditorReply::Error(::diagnostics::chain(&error))),
        }
    }
    pub(super) fn answer_setting_calls(&mut self, settled: &Settled) {
        let (done, waiting) = std::mem::take(&mut self.setting_calls)
            .into_iter()
            .partition(|call| call.revision <= settled.through);
        self.setting_calls = waiting;
        for call in done {
            let reply = match &settled.failure {
                Some(failure) if call.revision >= settled.first => {
                    EditorReply::Error(::diagnostics::chain(&EditorCallError::NotSaved {
                        key: call.key,
                        source: std::sync::Arc::clone(failure),
                    }))
                }
                Some(_) | None => EditorReply::Ok(json!({ "key": call.key, "value": call.value })),
            };
            answer(call.client, call.id, reply);
        }
    }
    fn mcp_sign_in_call(
        &mut self,
        host_id: &HostId,
        client: ForgeClient,
        id: String,
        server: &str,
        navigate_user: bool,
    ) {
        if navigate_user {
            self.navigate(Screen::Settings(SettingsPage::Extensions));
        }
        let started = self.start_mcp_sign_in(host_id, server, navigate_user);
        crate::core::runtime::runtime().spawn(async move {
            let reply = match started.await {
                Ok(Ok(url)) => EditorReply::Ok(json!({ "authorize_url": url })),
                Ok(Err(error)) => EditorReply::Error(::diagnostics::chain(&error)),
                Err(error) => EditorReply::Error(format!("The sign-in stopped: {error}")),
            };
            send_answer(&client, &id, &reply).await;
        });
    }
    fn run_call(&mut self, from: &HostId, call: ImmediateCall) -> Result<Value, EditorCallError> {
        match call {
            ImmediateCall::HostsList => Ok(Value::Array(self.hosts().map(|(config, host)| json!({ "id": config.id.0, "name": config.name, "address": config.address, "kind": config.kind, "connected": host.status == ConnectionStatus::Live, "is_caller": &config.id == from })).collect())),
            ImmediateCall::HostRemove { host } => { let id = self.find_host(&host)?; self.remove_host(&id).map_err(|source| EditorCallError::Remove { host: id.clone(), source })?; Ok(json!({ "removed": id.0 })) },
            ImmediateCall::HostSwitch { host } => { let id = self.find_host(&host)?; self.navigate(Screen::NewThread { host: id.clone(), project: None }); Ok(json!({ "switched_to": id.0 })) },
            ImmediateCall::SettingGet { key } => {
                let decl = editor_setting(&key)?;
                if let Settings::Unreadable(source) = &self.settings {
                    return Err(EditorCallError::Unreadable { key, source: source.clone() });
                }
                if let Some(source) = self.settings.rejected(&key) {
                    return Err(EditorCallError::Stored { key, source: source.clone() });
                }
                Ok(self.settings.get(&key).cloned().unwrap_or_else(|| decl.default_value()))
            }
            ImmediateCall::OpenThread { thread } => { self.navigate(Screen::Thread { host: from.clone(), thread: thread.clone() }); Ok(json!({ "opened": thread })) },
            ImmediateCall::OpenSettings { page } => { let found = SettingsPage::from_name(&page).ok_or_else(|| { let names: Vec<_> = SettingsPage::ALL.iter().map(|(_, name)| *name).collect(); EditorCallError::NoPage { page: page.clone(), pages: names.join(", ") } })?; self.navigate(Screen::Settings(found)); Ok(json!({ "opened": page })) },
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
