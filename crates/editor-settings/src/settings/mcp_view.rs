use std::rc::Rc;

use aqua_ui::{element::Element, style::Length};
use artisan_api::{McpAuthStatus, McpServer};

use super::{
    controller::{Gear, SettingsController},
    ui,
    view::ViewAction,
};
use editor_core::{Core, HostData, McpSignIn, McpSignInPhase, Problem};
use editor_ui::{
    carved::{
        Carve,
        glyph::{Icon, glyph},
    },
    kit::ButtonTone,
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
    ui::button(theme, id, label, tone, disabled, move || {
        emit(action.clone());
    })
}

fn controls(
    theme: &ArtisanTheme,
    name: &str,
    status: Option<McpAuthStatus>,
    sign_in: Option<&McpSignIn>,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Vec<Element> {
    let make = |kind: &str, label: &str, tone, disabled, action| {
        button(
            theme,
            format!("mcp-{kind}-{name}"),
            label,
            tone,
            disabled,
            action,
            Rc::clone(emit),
        )
    };
    match sign_in.map(|sign_in| (sign_in.phase(), sign_in.authorize_url().map(str::to_owned))) {
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
        Some((McpSignInPhase::Starting | McpSignInPhase::Waiting, _)) => vec![
            make(
                "busy",
                "Opening…",
                ButtonTone::Neutral,
                true,
                ViewAction::McpEdited,
            ),
            make(
                "cancel-sign-in",
                "Cancel",
                ButtonTone::Quiet,
                false,
                ViewAction::McpCancelSignIn(name.into()),
            ),
        ],
        Some((McpSignInPhase::Finishing, _)) => vec![
            make(
                "busy",
                "Signing in…",
                ButtonTone::Neutral,
                true,
                ViewAction::McpEdited,
            ),
            make(
                "cancel-sign-in",
                "Cancel",
                ButtonTone::Quiet,
                false,
                ViewAction::McpCancelSignIn(name.into()),
            ),
        ],
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
            Some(McpAuthStatus::NeedsToken) | None => Vec::new(),
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
        ("Program", "npx"),
        (
            "Arguments (JSON array)",
            "[\"-y\", \"@modelcontextprotocol/server-github\"]",
        ),
        ("Server URL (HTTP)", "https://example.com/mcp"),
        ("Environment (JSON object)", "{\"GITHUB_TOKEN\": \"…\"}"),
        (
            "Headers (JSON object, HTTP only)",
            "{\"Authorization\": \"Bearer …\"}",
        ),
    ];
    let mut cells: Vec<Element> = Vec::new();
    for (index, (label, placeholder)) in fields.into_iter().enumerate() {
        let send = Rc::clone(&emit);
        let editor = EditorController::from_shared(
            Rc::clone(&controller.mcp_editors[index]),
            move |event| match event {
                EditorEvent::Changed => send(ViewAction::McpEdited),
                EditorEvent::PresentationChanged => send(ViewAction::McpPresentationChanged),
                EditorEvent::SelectionChanged { .. }
                | EditorEvent::Submitted
                | EditorEvent::List(_)
                | EditorEvent::PastedFiles { .. } => {}
            },
        );
        cells.push(ui::field(
            theme,
            label,
            ui::input(
                theme,
                editor.element_with_notice_keyed(
                    format!("settings-mcp-field-{index}"),
                    Mode::SingleLine,
                    theme,
                    placeholder,
                    true,
                    false,
                ),
            ),
        ));
    }
    let mut rows = vec![ui::label(
        theme,
        match controller.extensions.editing {
            Some(super::extensions::McpEditing::Existing(_)) => "Edit MCP Server",
            Some(super::extensions::McpEditing::New) | None => "New MCP Server",
        },
    )];
    let mut cells = cells.into_iter();
    while let Some(left) = cells.next() {
        rows.push(
            ui::inline(14.0)
                .width(Length::Pct(100.0))
                .child(left)
                .children(cells.next())
                .build(),
        );
    }
    let saving = controller.extensions.saving;
    rows.push(
        ui::actions()
            .child(button(
                theme,
                "mcp-cancel".into(),
                "Cancel",
                ButtonTone::Quiet,
                saving,
                ViewAction::McpCancel,
                Rc::clone(&emit),
            ))
            .child(button(
                theme,
                "mcp-save".into(),
                if saving { "Saving…" } else { "Save" },
                ButtonTone::Primary,
                saving,
                ViewAction::McpSave,
                emit,
            ))
            .build(),
    );
    ui::form_block(theme, rows)
}

fn sign_in_failure(
    theme: &ArtisanTheme,
    server: &str,
    sign_in: Option<&McpSignIn>,
) -> Option<Element> {
    match sign_in?.phase() {
        McpSignInPhase::Failed(problem) => Some(ui::block(
            ui::problem(theme, format!("mcp-sign-in-error-{server}"), problem).build(),
        )),
        McpSignInPhase::Starting | McpSignInPhase::Waiting | McpSignInPhase::Finishing => None,
    }
}

pub(crate) fn toggle(
    controller: &SettingsController,
    host: &HostData,
    server: &McpServer,
    theme: &ArtisanTheme,
    emit: &Rc<dyn Fn(ViewAction)>,
) -> Element {
    let instance = host.instance();
    let send = Rc::clone(emit);
    let previous = server.clone();
    editor_ui::switch::Switch::new(
        format!("settings-mcp-enabled-{}", server.name),
        *theme,
        !server.disabled,
        server.name.clone(),
    )
    .recessed()
    .disabled(controller.extensions.saving || controller.extensions.editing.is_some())
    .on_change(move |enabled| send(ViewAction::McpToggle(instance, previous.clone(), enabled)))
    .build()
}

pub(crate) fn section(
    controller: &SettingsController,
    core: &Core,
    host: &HostData,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    let servers: Result<Vec<McpServer>, Problem> = host
        .mcp_servers()
        .map(<[McpServer]>::to_vec)
        .map_err(|error| Problem::caused_by("Couldn't read the MCP servers.", error));
    let auth = host.settings.mcp_auth();
    let mut rows = Vec::new();
    if let Err(problem) = &servers {
        rows.push(ui::block(
            ui::problem(theme, "mcp-servers-error", problem).build(),
        ));
    }
    if let Err(error) = auth {
        rows.push(ui::block(
            ui::problem(
                theme,
                "mcp-auth-error",
                &Problem::caused_by("Couldn't read which MCP servers are signed in.", error),
            )
            .build(),
        ));
    }
    let carve = Carve::of(theme);
    for server in servers.unwrap_or_default() {
        let status = match auth {
            Ok(auth) => auth.get(&server.name).copied(),
            Err(_shown_above) => None,
        };
        let sign_in = core.mcp_sign_in(&host.id, &server.name);
        let failure = sign_in_failure(theme, &server.name, sign_in);
        let mark = glyph(
            if server.url.is_some() {
                Icon::World
            } else {
                Icon::Terminal
            },
            20.0,
            carve.ink_dim,
        );
        let gear = Gear::Mcp(server.clone());
        let send = Rc::clone(&emit);
        let open = gear.clone();
        let controls = ui::actions()
            .children(controls(theme, &server.name, status, sign_in, &emit))
            .child(toggle(controller, host, &server, theme, &emit))
            .child(ui::gear(
                theme,
                gear.key(),
                format!("{} options", server.name),
                move || send(ViewAction::Gear(Some(open.clone()))),
            ))
            .build();
        rows.push(ui::machine_row(
            Some(ui::tile(mark)),
            ui::label(theme, server.name.clone()),
            Some(controls),
            false,
        ));
        if matches!(status, Some(McpAuthStatus::NeedsToken)) {
            rows.push(ui::note(
                theme,
                "Edit the server to add an Authorization header.",
            ));
        }
        rows.extend(failure);
    }
    if rows.is_empty() && controller.extensions.editing.is_none() {
        rows.push(ui::row(theme, "No MCP servers configured", "", None));
    }
    if controller.extensions.editing.is_some() {
        rows.push(form(controller, theme, Rc::clone(&emit)));
    }
    let mut body = ui::stack_gap(10.0)
        .width(Length::Pct(100.0))
        .child(ui::card(theme, rows));
    if let Some(problem) = &controller.extensions.error {
        body = body.child(ui::problem(theme, "mcp-error", problem).build());
    }
    let editing = controller.extensions.editing.is_some();
    let add = button(
        theme,
        "mcp-add".into(),
        if editing {
            "Cancel"
        } else {
            "Add new MCP server"
        },
        if editing {
            ButtonTone::Neutral
        } else {
            ButtonTone::Primary
        },
        controller.extensions.saving,
        if editing {
            ViewAction::McpCancel
        } else {
            ViewAction::McpOpen(None)
        },
        emit,
    );
    ui::section(theme, "MCP Servers", Some(add), body.build())
}
