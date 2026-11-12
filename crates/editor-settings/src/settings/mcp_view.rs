use std::{collections::BTreeMap, rc::Rc};

use aqua_ui::{
    element::{Box as UiBox, Element},
    style::{Length, Style},
};
use artisan_api::{MCP_AUTH_KEY, McpAuthStatus, McpServer};

use super::{
    controller::SettingsController,
    extensions::MCP_SERVERS_KEY,
    ui::{self, Ink},
    view::ViewAction,
};
use editor_core::{Core, HostData, McpSignIn, McpSignInPhase};
use editor_ui::{
    kit::{ButtonTone, Space, Spaced, TextRole},
    text::{
        Mode,
        mount::{EditorController, EditorEvent},
    },
    theme::ArtisanTheme,
};

fn button(
    theme: &ArtisanTheme,
    id: String,
    label: &str,
    tone: ButtonTone,
    disabled: bool,
    action: ViewAction,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    ui::button(theme, id, label, tone, disabled, move || emit(action.clone()))
}

fn note(status: Option<McpAuthStatus>, sign_in: Option<&McpSignIn>) -> Option<String> {
    if let Some(sign_in) = sign_in {
        return Some(match &sign_in.phase {
            McpSignInPhase::Starting => "Opening the sign-in…".into(),
            McpSignInPhase::Waiting => "Waiting for the browser…".into(),
            McpSignInPhase::Finishing => "Signing in…".into(),
            McpSignInPhase::Failed(reason) => format!("Couldn't sign in: {reason}"),
        });
    }
    match status? {
        McpAuthStatus::NeedsSignIn => Some("Needs sign-in".into()),
        McpAuthStatus::SignedIn => Some("Signed in".into()),
        McpAuthStatus::NeedsToken => {
            Some("Refused without a token: add an Authorization header (Edit)".into())
        }
    }
}

fn controls(
    theme: &ArtisanTheme,
    name: &str,
    status: Option<McpAuthStatus>,
    sign_in: Option<&McpSignIn>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Vec<Element> {
    let make = |kind: &str, label: &str, tone, disabled, action| {
        button(
            theme,
            format!("mcp-{kind}-{name}"),
            label,
            tone,
            disabled,
            action,
            emit.clone(),
        )
    };
    match sign_in.map(|sign_in| (&sign_in.phase, sign_in.authorize_url.clone())) {
        Some((McpSignInPhase::Waiting, Some(url))) => vec![
            make(
                "open",
                "Open sign-in page",
                ButtonTone::Quiet,
                false,
                ViewAction::OpenUrl(url.clone()),
            ),
            make(
                "copy",
                "Copy link",
                ButtonTone::Quiet,
                false,
                ViewAction::CopyText(url),
            ),
            make(
                "cancel-sign-in",
                "Cancel",
                ButtonTone::Quiet,
                false,
                ViewAction::McpCancelSignIn(name.into()),
            ),
        ],
        Some((McpSignInPhase::Starting | McpSignInPhase::Waiting, _)) => vec![make(
            "busy",
            "Opening…",
            ButtonTone::Neutral,
            true,
            ViewAction::McpEdited,
        )],
        Some((McpSignInPhase::Finishing, _)) => vec![make(
            "busy",
            "Signing in…",
            ButtonTone::Neutral,
            true,
            ViewAction::McpEdited,
        )],
        Some((McpSignInPhase::Failed(_), _)) => vec![make(
            "sign-in",
            "Try again",
            ButtonTone::Neutral,
            false,
            ViewAction::McpSignIn(name.into()),
        )],
        None => match status {
            Some(McpAuthStatus::NeedsSignIn) => vec![make(
                "sign-in",
                "Sign in",
                ButtonTone::Primary,
                false,
                ViewAction::McpSignIn(name.into()),
            )],
            Some(McpAuthStatus::SignedIn) => vec![make(
                "sign-out",
                "Sign out",
                ButtonTone::Quiet,
                false,
                ViewAction::McpSignOut(name.into()),
            )],
            _ => Vec::new(),
        },
    }
}

fn form(
    controller: &SettingsController,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let fields = [
        ("Name", "github"),
        ("Program (literal path or executable name)", "npx"),
        (
            "Arguments (JSON array)",
            "[\"-y\", \"@modelcontextprotocol/server-github\"]",
        ),
        ("Or URL (HTTP server)", "https://example.com/mcp"),
        ("Environment (JSON object)", "{\"GITHUB_TOKEN\": \"…\"}"),
        (
            "Headers, HTTP only (JSON object; leave out Authorization to sign in instead)",
            "{\"Authorization\": \"Bearer …\"}",
        ),
    ];
    let mut body = ui::stack(Space::S12);
    for (index, (label, placeholder)) in fields.into_iter().enumerate() {
        let send = emit.clone();
        let editor =
            EditorController::from_shared(controller.mcp_editors[index].clone(), move |event| {
                match event {
                    EditorEvent::Changed => send(ViewAction::McpEdited),
                    EditorEvent::PresentationChanged => send(ViewAction::McpPresentationChanged),
                    _ => {}
                }
            });
        body = body.child(
            ui::stack(Space::S4)
                .child(ui::text(theme, TextRole::Label, Ink::Muted, label))
                .child(editor.element_with_notice_keyed(
                    format!("settings-mcp-field-{index}"),
                    Mode::SingleLine,
                    theme,
                    placeholder,
                    false,
                    false,
                ))
                .build(),
        );
    }
    let saving = controller.extensions.saving;
    let actions = ui::actions()
        .child(button(
            theme,
            "mcp-save".into(),
            if saving { "Saving…" } else { "Save server" },
            ButtonTone::Primary,
            saving,
            ViewAction::McpSave,
            emit.clone(),
        ))
        .child(button(
            theme,
            "mcp-cancel".into(),
            "Cancel",
            ButtonTone::Quiet,
            false,
            ViewAction::McpCancel,
            emit,
        ));
    ui::block(body.child(actions.build()).build())
}

pub fn section(
    controller: &SettingsController,
    core: &Core,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let servers: Result<Vec<McpServer>, String> =
        host.setting(MCP_SERVERS_KEY)
            .map_or(Ok(Vec::new()), |value| {
                serde_json::from_value(value.clone())
                    .map_err(|error| format!("Couldn't read MCP servers: {error}"))
            });
    let auth: BTreeMap<String, McpAuthStatus> = host
        .setting(MCP_AUTH_KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    let mut rows = Vec::new();
    if let Err(error) = &servers {
        rows.push(ui::block(ui::error_text(theme, error)));
    }
    for server in servers.unwrap_or_default() {
        let target = server.url.clone().unwrap_or_else(|| {
            format!(
                "{} {}",
                server.command,
                serde_json::to_string(&server.args).unwrap_or_default()
            )
        });
        let status = auth.get(&server.name).copied();
        let sign_in = core.mcp_sign_in(&host.id, &server.name);
        let summary =
            note(status, sign_in).map_or(target.clone(), |note| format!("{target} · {note}"));
        let mut actions = UiBox::new()
            .spaced(Space::S8)
            .style(Style::new().wrap(true))
            .children(controls(theme, &server.name, status, sign_in, emit.clone()));
        actions = actions.child(button(
            theme,
            format!("mcp-edit-{}", server.name),
            "Edit",
            ButtonTone::Quiet,
            false,
            ViewAction::McpOpen(Some(server.clone())),
            emit.clone(),
        ));
        actions = actions.child(button(
            theme,
            format!("mcp-remove-{}", server.name),
            "Remove",
            ButtonTone::Quiet,
            controller.extensions.saving,
            ViewAction::McpRemove(server.clone()),
            emit.clone(),
        ));
        rows.push(ui::row(theme, server.name, summary, Some(actions.build())));
    }
    if rows.is_empty() && controller.extensions.editing.is_none() {
        rows.push(ui::row(
            theme,
            "No MCP servers",
            "Servers added here are passed to every engine at launch.",
            None,
        ));
    }
    if controller.extensions.editing.is_some() {
        rows.push(form(controller, theme, emit.clone()));
    }
    let mut body = ui::stack(Space::S8).child(ui::card(theme, rows));
    if let Some(error) = &controller.extensions.error {
        body = body.child(ui::error_text(theme, error));
    }
    if controller.extensions.editing.is_none() {
        body = body.child(
            UiBox::new()
                .child(button(
                    theme,
                    "mcp-add".into(),
                    "Add server",
                    ButtonTone::Neutral,
                    false,
                    ViewAction::McpOpen(None),
                    emit,
                ))
                .build(),
        );
    }
    ui::section(
        theme,
        "MCP servers",
        Some("Tools every engine on this host can call."),
        None,
        body.width(Length::Pct(100.0)).build(),
    )
}
