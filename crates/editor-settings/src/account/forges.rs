//! The account's Forges and this machine's own: setup before the workspace
//! opens, Home when no Forge is open, and the Account page afterwards.

use editor_core::{
    Account, AccountEvent, AccountMachine, Core, CoreEvent, EntryGate, HostId, LocalForge, Screen,
};
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme};
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, Window, div, px,
};

pub struct ForgesView {
    core: Entity<Core>,
    code: Entity<TextEditor>,
    /// The machine "Use this Forge" is connecting to; setup finishes once
    /// its route is live.
    finishing: Option<String>,
    error: Option<String>,
    _subscriptions: [Subscription; 3],
}

impl ForgesView {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let code_field =
            cx.new(|cx| TextEditor::new("", "Code from another machine", cx).single_line());
        let account = core.read(cx).account().clone();
        let subscriptions = [
            cx.subscribe(&code_field, |this, _, event: &TextEditorEvent, cx| {
                if *event == TextEditorEvent::Submitted {
                    this.claim(cx);
                }
            }),
            cx.subscribe(&account, |_, _, _: &AccountEvent, cx| cx.notify()),
            cx.subscribe(&core, |this, _, event: &CoreEvent, cx| {
                if matches!(event, CoreEvent::HostsChanged | CoreEvent::AccessChanged) {
                    this.finish_when_live(cx);
                    cx.notify();
                }
            }),
        ];
        Self {
            core,
            code: code_field,
            finishing: None,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    fn account(
        &self,
        cx: &mut Context<Self>,
        action: impl FnOnce(&mut Account, &mut Context<Account>),
    ) {
        let account = self.core.read(cx).account().clone();
        account.update(cx, action);
    }

    fn claim(&mut self, cx: &mut Context<Self>) {
        let code = self.code.read(cx).text().to_owned();
        self.account(cx, |account, cx| account.claim_code(code, cx));
        self.code.update(cx, |editor, cx| editor.set_text("", cx));
    }

    fn use_forge(&mut self, machine_id: String, cx: &mut Context<Self>) {
        self.error = None;
        if self.core.read(cx).machine_live(&machine_id, cx) {
            self.finish(machine_id, cx);
        } else {
            self.core
                .update(cx, |core, cx| core.connect_machine(&machine_id, cx));
            self.finishing = Some(machine_id);
        }
        cx.notify();
    }

    fn finish_when_live(&mut self, cx: &mut Context<Self>) {
        if let Some(machine_id) = self
            .finishing
            .take_if(|id| self.core.read(cx).machine_live(id, cx))
        {
            self.finish(machine_id, cx);
        }
    }

    fn finish(&mut self, machine_id: String, cx: &mut Context<Self>) {
        self.error = self
            .core
            .update(cx, |core, cx| core.complete_setup(machine_id, cx))
            .err()
            .map(|error| format!("{error:#}"));
    }

    fn connect(&self, machine_id: &str, cx: &mut Context<Self>) {
        self.core
            .update(cx, |core, cx| core.connect_machine(machine_id, cx));
    }

    fn open(&self, machine_id: String, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| {
            core.navigate(
                Screen::NewThread {
                    host: HostId(machine_id),
                    project: None,
                },
                cx,
            );
        });
    }

    fn button(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        variant: ButtonVariant,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> Button {
        let id = id.into();
        Button::text(id.clone(), *theme, label)
            .variant(variant)
            .debug_selector(id)
            .on_activate(cx.listener(move |this, _, _, cx| action(this, cx)))
    }

    fn machine_row(
        machine: &AccountMachine,
        live: bool,
        setup: bool,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let seen = machine.last_seen_at.as_deref().map_or_else(
            || "Never connected".to_owned(),
            |seen| format!("Last seen {seen}"),
        );
        let id = machine.id.clone();
        let primary = match (setup, live) {
            (true, _) => {
                let id = id.clone();
                Self::button(
                    format!("forges-use-{}", machine.id),
                    "Use this Forge",
                    ButtonVariant::Default,
                    theme,
                    cx,
                    move |this, cx| this.use_forge(id.clone(), cx),
                )
            }
            (false, true) => {
                let id = id.clone();
                Self::button(
                    format!("forges-open-{}", machine.id),
                    "Open",
                    ButtonVariant::Outline,
                    theme,
                    cx,
                    move |this, cx| this.open(id.clone(), cx),
                )
            }
            (false, false) => {
                let id = id.clone();
                Self::button(
                    format!("forges-connect-{}", machine.id),
                    "Connect",
                    ButtonVariant::Default,
                    theme,
                    cx,
                    move |this, cx| this.connect(&id, cx),
                )
            }
        };
        let remove = Self::button(
            format!("forges-remove-{}", machine.id),
            "Remove",
            ButtonVariant::Ghost,
            theme,
            cx,
            move |this, cx| {
                let id = id.clone();
                this.account(cx, |account, cx| account.remove_machine(id, cx));
            },
        );
        line(theme, machine.label.clone(), seen)
            .child(actions().child(primary).child(remove))
            .into_any_element()
    }

    fn this_machine(
        local: LocalForge,
        email: &str,
        theme: &ArtisanTheme,
        cx: &Context<Self>,
    ) -> AnyElement {
        let pair = |label: &'static str| {
            Self::button(
                "forges-pair",
                label,
                ButtonVariant::Default,
                theme,
                cx,
                |this, cx| {
                    this.account(cx, Account::enroll_this_forge);
                },
            )
        };
        let (detail, action) = match local {
            LocalForge::Unknown => ("Its Forge hasn't answered yet.".to_owned(), None),
            LocalForge::Unenrolled => (
                "Its Forge isn't on the account.".to_owned(),
                Some(pair("Pair this machine")),
            ),
            LocalForge::Pending => (
                "Its Forge waits for its enrollment code to be claimed below.".to_owned(),
                None,
            ),
            LocalForge::Claimed { email: claimed } => (
                format!(
                    "Its Forge is claimed for {}.",
                    claimed.as_deref().unwrap_or(email)
                ),
                Some(Self::button(
                    "forges-confirm",
                    "Confirm",
                    ButtonVariant::Default,
                    theme,
                    cx,
                    |this, cx| this.account(cx, Account::confirm_forge),
                )),
            ),
            LocalForge::Enrolled { .. } => (
                "Its Forge is on the account and connects directly.".to_owned(),
                None,
            ),
            LocalForge::Revoked { .. } => (
                "Its Forge was removed from the account.".to_owned(),
                Some(pair("Pair it again")),
            ),
            LocalForge::OtherAccount => ("Its Forge belongs to another account.".to_owned(), None),
        };
        let check = Self::button(
            "forges-check",
            "Check again",
            ButtonVariant::Ghost,
            theme,
            cx,
            |this, cx| this.account(cx, Account::refresh_local_forge),
        );
        line(theme, "This machine", detail)
            .child(actions().child(check).children(action))
            .into_any_element()
    }

    fn claim_row(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> AnyElement {
        line(
            theme,
            "Another machine",
            "Claim the code its `ae account enroll` printed.",
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("forges-code")
                        .flex_1()
                        .min_w_0()
                        .child(self.code.clone()),
                )
                .child(Self::button(
                    "forges-claim",
                    "Claim",
                    ButtonVariant::Default,
                    theme,
                    cx,
                    Self::claim,
                )),
        )
        .into_any_element()
    }
}

impl Render for ForgesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &{ *cx.theme() };
        let core = self.core.read(cx);
        let setup = matches!(core.entry_gate(cx), EntryGate::NeedsSetup { .. });
        let account = core.account().read(cx);
        let email = account
            .profile()
            .map(|profile| profile.email.clone())
            .unwrap_or_default();
        let local = account.local_forge();
        let error = self
            .error
            .clone()
            .or_else(|| account.last_error().map(str::to_owned));
        let machines: Vec<(AccountMachine, bool)> = account
            .machines()
            .into_iter()
            .map(|machine| {
                let live = core.machine_live(&machine.id, cx);
                (machine, live)
            })
            .collect();

        let mut rows: Vec<AnyElement> = machines
            .iter()
            .map(|(machine, live)| Self::machine_row(machine, *live, setup, theme, cx))
            .collect();
        if rows.is_empty() {
            rows.push(
                muted(theme, "No Forges on this account yet.")
                    .py(px(8.0))
                    .into_any_element(),
            );
        }
        rows.push(Self::this_machine(local, &email, theme, cx));
        rows.push(self.claim_row(theme, cx));
        rows.extend(error.map(|error| {
            div()
                .text_color(theme.colors.destructive_text.to_paint())
                .child(error)
                .into_any_element()
        }));
        if setup {
            rows.push(
                actions()
                    .child(Self::button(
                        "forges-use-another",
                        "Use another account",
                        ButtonVariant::Ghost,
                        theme,
                        cx,
                        |this, cx| this.account(cx, Account::sign_out),
                    ))
                    .into_any_element(),
            );
        }
        div()
            .id("forges")
            .debug_selector(|| "forges".to_owned())
            .flex()
            .flex_col()
            .gap(px(8.0))
            .children(rows)
    }
}

fn muted(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_color(theme.colors.text_muted.to_paint())
        .child(text.into())
}

fn line(
    theme: &ArtisanTheme,
    title: impl Into<SharedString>,
    detail: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .py(px(8.0))
        .child(
            div()
                .text_color(theme.colors.text.to_paint())
                .child(title.into()),
        )
        .child(muted(theme, detail))
}

fn actions() -> Div {
    div().flex().flex_row().justify_end().gap(px(8.0))
}
