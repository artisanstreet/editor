//! The MCP servers of one host (§2.10a). Forge keeps the servers in its
//! settings and hands them to every engine on its next turn. Skills are the
//! Marketplace's; this page only points there.

use crate::Settings;
use crate::chrome::{card, error_text, header, row, section_shell};
use crate::page;
use editor_core::{HostData, HostId, Screen, SettingsPage};
use editor_ui::button::ButtonVariant;
use editor_ui::text_editor::TextEditor;
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, SharedString, Styled as _, div, px,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub(crate) const MCP_SERVERS_KEY: &str = "mcp_servers";
/// The Skills section's way to the Marketplace.
pub(crate) const MANAGE_SKILLS_SELECTOR: &str = "settings-manage-skills";

/// One entry of the `mcp_servers` setting: a stdio server (`command`,
/// `args`, `env`) or, with `url`, an HTTP one. Mirrors Forge's `McpServer`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct McpServer {
    pub name: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

pub(crate) struct ExtensionsForm {
    /// The host the form was opened for.
    host: Option<HostId>,
    /// `Some(None)` adds a server, `Some(Some(name))` edits that one.
    mcp_editing: Option<Option<String>>,
    mcp_name: Entity<TextEditor>,
    mcp_command: Entity<TextEditor>,
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
            mcp_name: line("github", cx),
            mcp_command: line("npx -y @modelcontextprotocol/server-github", cx),
            mcp_url: line("https://example.com/mcp", cx),
            mcp_env: line("GITHUB_TOKEN=… OTHER=…", cx),
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
        self.mcp_editing = None;
        self.mcp_error = None;
    }
}

fn mcp_servers(host: &HostData) -> Vec<McpServer> {
    host.setting(MCP_SERVERS_KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

impl Settings {
    fn open_mcp_form(&mut self, server: Option<McpServer>, cx: &mut Context<Self>) {
        let server = server.map_or((None, McpServer::default()), |server| {
            (Some(server.name.clone()), server)
        });
        let (original, server) = server;
        let command = join_command(
            std::iter::once(&server.command)
                .filter(|c| !c.is_empty())
                .chain(&server.args),
        );
        let env = join_command(
            server
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .iter(),
        );
        let form = &self.extensions;
        for (editor, text) in [
            (&form.mcp_name, server.name.as_str()),
            (&form.mcp_command, command.as_str()),
            (&form.mcp_url, server.url.as_deref().unwrap_or_default()),
            (&form.mcp_env, env.as_str()),
        ] {
            editor.update(cx, |editor, cx| editor.set_text(text, cx));
        }
        self.extensions.mcp_editing = Some(original);
        self.extensions.mcp_error = None;
        cx.notify();
    }

    fn save_mcp_form(&mut self, cx: &mut Context<Self>) {
        let Some(original) = self.extensions.mcp_editing.clone() else {
            return;
        };
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        let form = &self.extensions;
        let read = |editor: &Entity<TextEditor>| editor.read(cx).text().trim().to_owned();
        let server = parse_server(
            read(&form.mcp_name),
            &read(&form.mcp_command),
            read(&form.mcp_url),
            &read(&form.mcp_env),
        );
        let mut servers = mcp_servers(host.read(cx));
        let result = server.and_then(|server| {
            if servers
                .iter()
                .any(|known| known.name == server.name && Some(&known.name) != original.as_ref())
            {
                return Err(format!("There is already a server called {}.", server.name));
            }
            match original
                .as_ref()
                .and_then(|name| servers.iter().position(|known| &known.name == name))
            {
                Some(index) => servers[index] = server,
                None => servers.push(server),
            }
            Ok(servers)
        });
        match result {
            Ok(servers) => self.store_mcp_servers(&host, servers, true, cx),
            Err(error) => {
                self.extensions.mcp_error = Some(error);
                cx.notify();
            }
        }
    }

    fn remove_mcp(&mut self, name: &str, cx: &mut Context<Self>) {
        let Some(host) = self.selected_host(cx) else {
            return;
        };
        let mut servers = mcp_servers(host.read(cx));
        servers.retain(|server| server.name != name);
        self.store_mcp_servers(&host, servers, false, cx);
    }

    fn store_mcp_servers(
        &mut self,
        host: &Entity<HostData>,
        servers: Vec<McpServer>,
        close_form: bool,
        cx: &mut Context<Self>,
    ) {
        let client = host.read(cx).client.clone();
        let value = match serde_json::to_value(&servers) {
            Ok(value) => value,
            Err(error) => {
                self.extensions.mcp_error =
                    Some(format!("Couldn't encode the MCP servers: {error}"));
                cx.notify();
                return;
            }
        };
        self.extensions.mcp_saving = true;
        self.extensions.mcp_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.set_setting(MCP_SERVERS_KEY, &value).await;
            let _ = this.update(cx, |this, cx| {
                this.extensions.mcp_saving = false;
                match result {
                    Ok(()) if close_form => this.extensions.mcp_editing = None,
                    Ok(()) => {}
                    Err(error) => {
                        this.extensions.mcp_error =
                            Some(format!("Couldn't save the MCP servers: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn parse_server(name: String, command: &str, url: String, env: &str) -> Result<McpServer, String> {
    if name.is_empty() {
        return Err("Give the server a name.".to_owned());
    }
    let mut parts = split_command(command).into_iter();
    let program = parts.next().unwrap_or_default();
    if program.is_empty() && url.is_empty() {
        return Err(format!("{name} needs a command to run or a URL."));
    }
    if !program.is_empty() && !url.is_empty() {
        return Err(format!("{name} has both a command and a URL; keep one."));
    }
    let mut variables = BTreeMap::new();
    for pair in split_command(env) {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("{pair:?} in the environment isn't KEY=value."));
        };
        variables.insert(key.to_owned(), value.to_owned());
    }
    Ok(McpServer {
        name,
        command: program,
        args: parts.collect(),
        env: variables,
        url: (!url.is_empty()).then_some(url),
    })
}

/// Splits on whitespace outside double quotes.
fn split_command(line: &str) -> Vec<String> {
    let (mut parts, mut current, mut quoted, mut started) =
        (Vec::new(), String::new(), false, false);
    for c in line.chars() {
        match c {
            '"' => (quoted, started) = (!quoted, true),
            c if c.is_whitespace() && !quoted => {
                if started {
                    parts.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        parts.push(current);
    }
    parts
}

fn join_command<'a>(parts: impl Iterator<Item = &'a String>) -> String {
    parts
        .map(|part| {
            if part.is_empty() || part.contains(char::is_whitespace) {
                format!("\"{part}\"")
            } else {
                part.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
    for server in servers {
        let summary = match &server.url {
            Some(url) => url.clone(),
            None => join_command(std::iter::once(&server.command).chain(&server.args)),
        };
        let (edit, remove) = (server.clone(), server.name.clone());
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
                cx.listener(move |this, _, _, cx| this.remove_mcp(&remove, cx)),
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
    let mut block = section_shell(
        theme,
        "mcp",
        "MCP servers",
        Some("Tools every engine on this host can call."),
        None,
        body,
    );
    if let Some(add) = add {
        block = block.child(div().mt(px(8.0)).child(add));
    }
    block
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
            "Command (with arguments; quote ones with spaces)",
            &form.mcp_command,
        ))
        .child(field("Or URL (HTTP server)", &form.mcp_url))
        .child(field(
            "Environment (KEY=value, space-separated)",
            &form.mcp_env,
        ))
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
