//! The MCP servers of one host (§2.10a). Forge keeps the servers in its
//! settings and hands them to every engine on its next turn. Skills are the
//! Marketplace's; this page only points there.

use crate::Settings;
use crate::chrome::{card, error_text, header, row, section_shell};
use crate::page;
use artisan_api::{McpMutation, McpServer};
use editor_core::{HostData, HostId, Screen, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::text_editor::TextEditor;
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, div, px,
};

pub(crate) const MCP_SERVERS_KEY: &str = "mcp_servers";
/// The Skills section's way to the Marketplace.
pub(crate) const MANAGE_SKILLS_SELECTOR: &str = "settings-manage-skills";

pub(crate) struct ExtensionsForm {
    /// The host the form was opened for.
    host: Option<HostId>,
    /// `Some(None)` adds a server, `Some(Some(name))` edits that one.
    mcp_editing: Option<Option<McpServer>>,
    generation: u64,
    mcp_name: Entity<TextEditor>,
    mcp_command: Entity<TextEditor>,
    mcp_args: Entity<TextEditor>,
    mcp_url: Entity<TextEditor>,
    mcp_env: Entity<TextEditor>,
    mcp_saving: bool,
    mcp_error: Option<String>,
}

impl ExtensionsForm {
    pub(crate) fn new(cx: &mut Context<Settings>) -> Self {
        let line = |placeholder: &'static str, cx: &mut Context<Settings>| {
            cx.new(|cx| TextEditor::new("", placeholder, cx).single_line())
        };
        Self {
            host: None,
            mcp_editing: None,
            generation: 0,
            mcp_name: line("github", cx),
            mcp_command: line("npx", cx),
            mcp_args: line("[\"-y\", \"@modelcontextprotocol/server-github\"]", cx),
            mcp_url: line("https://example.com/mcp", cx),
            mcp_env: line("{\"GITHUB_TOKEN\": \"…\"}", cx),
            mcp_saving: false,
            mcp_error: None,
        }
    }

    /// Closes the server form when the host changes.
    pub(crate) fn sync(&mut self, host: Option<&Entity<HostData>>, cx: &mut Context<Settings>) {
        let id = host.map(|host| host.read(cx).id.clone());
        if self.host == id {
            return;
        }
        self.host = id;
        self.generation += 1;
        self.mcp_saving = false;
        self.mcp_editing = None;
        self.mcp_error = None;
    }
}

fn mcp_servers(host: &HostData) -> Result<Vec<McpServer>, String> {
    host.setting(MCP_SERVERS_KEY)
        .map_or(Ok(Vec::new()), |value| {
            serde_json::from_value(value.clone())
                .map_err(|error| format!("Couldn't read MCP servers: {error}"))
        })
}

impl Settings {
    fn open_mcp_form(&mut self, previous: Option<McpServer>, cx: &mut Context<Self>) {
        let server = previous.clone().unwrap_or_default();
        let args = serde_json::to_string(&server.args).expect("string array serializes");
        let env = serde_json::to_string(&server.env).expect("string map serializes");
        let form = &mut self.extensions;
        form.generation += 1;
        form.mcp_saving = false;
        for (editor, text) in [
            (&form.mcp_name, server.name.as_str()),
            (&form.mcp_command, server.command.as_str()),
            (&form.mcp_args, args.as_str()),
            (&form.mcp_url, server.url.as_deref().unwrap_or_default()),
            (&form.mcp_env, env.as_str()),
        ] {
            editor.update(cx, |editor, cx| editor.set_text(text, cx));
        }
        form.mcp_editing = Some(previous);
        form.mcp_error = None;
        cx.notify();
    }

    fn save_mcp_form(&mut self, cx: &mut Context<Self>) {
        if self.extensions.mcp_saving {
            return;
        }
        let Some(previous) = self.extensions.mcp_editing.clone() else {
            return;
        };
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        let form = &self.extensions;
        let read = |editor: &Entity<TextEditor>| editor.read(cx).text().to_owned();
        match parse_server(
            read(&form.mcp_name),
            read(&form.mcp_command),
            &read(&form.mcp_args),
            read(&form.mcp_url),
            &read(&form.mcp_env),
        ) {
            Ok(server) => {
                self.mutate_mcp(&host, McpMutation::Upsert { previous, server }, true, cx)
            }
            Err(error) => {
                self.extensions.mcp_error = Some(error);
                cx.notify();
            }
        }
    }

    fn remove_mcp(&mut self, previous: McpServer, cx: &mut Context<Self>) {
        if self.extensions.mcp_saving {
            return;
        }
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        self.mutate_mcp(&host, McpMutation::Remove { previous }, false, cx);
    }

    fn mutate_mcp(
        &mut self,
        host: &Entity<HostData>,
        mutation: McpMutation,
        close_form: bool,
        cx: &mut Context<Self>,
    ) {
        let data = host.read(cx);
        let (client, host_id) = (data.client.clone(), data.id.clone());
        let form = &mut self.extensions;
        form.generation += 1;
        let generation = form.generation;
        form.mcp_saving = true;
        form.mcp_error = None;
        cx.notify();
        #[cfg(test)]
        let completion_gate = self.completion_gate.clone();
        cx.spawn(async move |this, cx| {
            let result = client.mutate_mcp(&mutation).await;
            #[cfg(test)]
            if let Some(gate) = completion_gate {
                gate.wait().await;
            }
            let _ = this.update(cx, |this, cx| {
                let form = &mut this.extensions;
                if form.host.as_ref() != Some(&host_id) || form.generation != generation {
                    return;
                }
                form.mcp_saving = false;
                match result {
                    Ok(_) if close_form => form.mcp_editing = None,
                    Ok(_) => {}
                    Err(error) => {
                        form.mcp_error = Some(format!("Couldn't save MCP servers: {error:#}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn parse_server(
    name: String,
    command: String,
    args: &str,
    url: String,
    env: &str,
) -> Result<McpServer, String> {
    let name = name.trim().to_owned();
    let url = url.trim().to_owned();
    if name.is_empty() {
        return Err("Give the server a name.".into());
    }
    if command.is_empty() == url.is_empty() {
        return Err("Specify either a program or an HTTP URL.".into());
    }
    let args: Vec<String> = if args.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(args)
            .map_err(|error| format!("Arguments must be a JSON array of strings: {error}"))?
    };
    let env: std::collections::BTreeMap<String, String> = if env.trim().is_empty() {
        Default::default()
    } else {
        serde_json::from_str(env)
            .map_err(|error| format!("Environment must be a JSON object of strings: {error}"))?
    };
    if !url.is_empty() && (!args.is_empty() || !env.is_empty()) {
        return Err("HTTP servers do not take process arguments or environment variables.".into());
    }
    Ok(McpServer {
        name,
        command,
        args,
        env,
        url: (!url.is_empty()).then_some(url),
    })
}

pub(crate) fn render(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::Extensions),
            page::description(SettingsPage::Extensions),
        ))
        .child(mcp_section(settings, host, theme, cx))
        .child(skills_section(settings, theme, cx))
}

/// The skills live in the Marketplace, which shows where each stands in
/// each engine and turns them on and off.
fn skills_section(settings: &Settings, theme: &ArtisanTheme, cx: &mut Context<Settings>) -> Div {
    let manage = div()
        .debug_selector(|| MANAGE_SKILLS_SELECTOR.to_owned())
        .child(settings.button(
            "settings-manage-skills",
            "Manage skills in Marketplace",
            ButtonVariant::Outline,
            false,
            cx.listener(|this, _, _, cx| {
                this.core
                    .update(cx, |core, cx| core.navigate(Screen::Marketplace, cx));
            }),
        ));
    section_shell(
        theme,
        "skills",
        "Skills",
        Some(
            "Artisan's skills and your own reach every engine on this host; the Marketplace turns each on or off.",
        ),
        None,
        div().child(manage),
    )
}

fn mcp_section(
    settings: &Settings,
    host: &Entity<HostData>,
    theme: &ArtisanTheme,
    cx: &mut Context<Settings>,
) -> Div {
    let form = &settings.extensions;
    let servers = mcp_servers(host.read(cx));
    let mut rows: Vec<AnyElement> = Vec::new();
    if let Err(error) = &servers {
        rows.push(error_text(theme, error.clone()).into_any_element());
    }
    for server in servers.unwrap_or_default() {
        let summary = match &server.url {
            Some(url) => url.clone(),
            None => format!(
                "{} {}",
                server.command,
                serde_json::to_string(&server.args).expect("string array serializes")
            ),
        };
        let (edit, remove) = (server.clone(), server.clone());
        let controls = div()
            .flex()
            .flex_row()
            .gap(px(6.0))
            .child(settings.button(
                SharedString::from(format!("mcp-edit-{}", server.name)),
                "Edit",
                ButtonVariant::Ghost,
                false,
                cx.listener(move |this, _, _, cx| this.open_mcp_form(Some(edit.clone()), cx)),
            ))
            .child(settings.button(
                SharedString::from(format!("mcp-remove-{}", server.name)),
                "Remove",
                ButtonVariant::Ghost,
                form.mcp_saving,
                cx.listener(move |this, _, _, cx| this.remove_mcp(remove.clone(), cx)),
            ));
        rows.push(
            row(
                theme,
                server.name.clone(),
                summary,
                Some(controls.into_any_element()),
            )
            .into_any_element(),
        );
    }
    if rows.is_empty() && form.mcp_editing.is_none() {
        rows.push(
            row(
                theme,
                "No MCP servers",
                "Servers added here are passed to every engine at launch.",
                None,
            )
            .into_any_element(),
        );
    }
    if form.mcp_editing.is_some() {
        rows.push(mcp_form(settings, theme, cx));
    }
    let mut body = div()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .child(card(theme, rows));
    if let Some(error) = &form.mcp_error {
        body = body.child(error_text(theme, error.clone()));
    }
    let add = (form.mcp_editing.is_none()).then(|| {
        settings.button(
            "mcp-add",
            "Add server",
            ButtonVariant::Outline,
            false,
            cx.listener(|this, _, _, cx| this.open_mcp_form(None, cx)),
        )
    });
    if let Some(add) = add {
        body = body.child(div().mt(px(8.0)).child(add));
    }
    section_shell(
        theme,
        "mcp",
        "MCP servers",
        Some("Tools every engine on this host can call."),
        None,
        body,
    )
}

fn mcp_form(settings: &Settings, theme: &ArtisanTheme, cx: &mut Context<Settings>) -> AnyElement {
    let form = &settings.extensions;
    let field = |label: &'static str, editor: &Entity<TextEditor>| {
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_sm()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(label),
            )
            .child(editor.clone())
    };
    let save = settings.button(
        "mcp-save",
        if form.mcp_saving {
            "Saving…"
        } else {
            "Save server"
        },
        ButtonVariant::Default,
        form.mcp_saving,
        cx.listener(|this, _, _, cx| this.save_mcp_form(cx)),
    );
    let cancel = settings.button(
        "mcp-cancel",
        "Cancel",
        ButtonVariant::Ghost,
        false,
        cx.listener(|this, _, _, cx| {
            this.extensions.generation += 1;
            this.extensions.mcp_saving = false;
            this.extensions.mcp_editing = None;
            this.extensions.mcp_error = None;
            cx.notify();
        }),
    );
    div()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .py(px(14.0))
        .child(field("Name", &form.mcp_name))
        .child(field(
            "Program (literal path or executable name)",
            &form.mcp_command,
        ))
        .child(field("Arguments (JSON array)", &form.mcp_args))
        .child(field("Or URL (HTTP server)", &form.mcp_url))
        .child(field("Environment (JSON object)", &form.mcp_env))
        .child(
            div()
                .flex()
                .flex_row()
                .gap(px(6.0))
                .child(save)
                .child(cancel),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::parse_server;
    #[test]
    fn structured_fields_preserve_literal_arguments() {
        let server = parse_server(
            "test".into(),
            r"C:\Program Files\server.exe".into(),
            r#"["a b","\"quoted\"","", "line\nnext"]"#,
            String::new(),
            r#"{"TOKEN":"a=b \"quoted\""}"#,
        )
        .unwrap();
        assert_eq!(server.command, r"C:\Program Files\server.exe");
        assert_eq!(server.args, ["a b", "\"quoted\"", "", "line\nnext"]);
        assert_eq!(server.env["TOKEN"], "a=b \"quoted\"");
    }
}

#[cfg(test)]
mod delayed_mutation_tests {
    use super::*;
    use crate::tests::{CompletionGate, SettingsFixture, finish_gate, wait_gate};
    use gpui::TestAppContext;
    #[gpui::test]
    fn completed_mcp_save_cannot_close_reopened_form_or_new_host_form(cx: &mut TestAppContext) {
        let fixture = SettingsFixture::new(cx);
        let (settings, cx) = fixture.open(cx, SettingsPage::Extensions);
        for switch_host in [false, true] {
            let gate = CompletionGate::new();
            settings.update(cx, |settings, cx| {
                settings.completion_gate = Some(gate.clone());
                let host = settings.selected_host(cx).unwrap();
                settings.mutate_mcp(
                    &host,
                    McpMutation::Remove {
                        previous: McpServer {
                            name: "nonexistent-test-server".into(),
                            ..McpServer::default()
                        },
                    },
                    true,
                    cx,
                );
            });
            wait_gate(cx, &gate);
            if switch_host {
                fixture.select(1, SettingsPage::Extensions, cx);
            }
            settings.update(cx, |settings, cx| {
                settings.open_mcp_form(None, cx);
                settings
                    .extensions
                    .mcp_name
                    .update(cx, |editor, cx| editor.set_text("new form", cx));
                settings.extensions.mcp_error = Some("new form owns this error".into());
            });
            finish_gate(cx, &gate);
            settings.read_with(cx, |settings, cx| {
                assert!(settings.extensions.mcp_editing.is_some());
                assert_eq!(settings.extensions.mcp_name.read(cx).text(), "new form");
                assert_eq!(
                    settings.extensions.mcp_error.as_deref(),
                    Some("new form owns this error")
                );
                assert!(!settings.extensions.mcp_saving);
            });
        }
    }
}
