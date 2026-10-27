use editor_core::Core;
use editor_settings::account::controller;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::theme::ActiveTheme as _;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, Styled as _, Subscription, div, px,
};

pub struct SetupView {
    core: Entity<Core>,
    claim_code: Entity<editor_ui::text_editor::TextEditor>,
    show_claim: bool,
    finish_error: Option<String>,
    setup_flow: bool,
    _claim: Subscription,
    _account: Subscription,
}

impl SetupView {
    /// `setup_flow` renders the NeedsSetup actions (Connect then explicit
    /// Finish setup). Outside the gate, the same inventory offers Connect
    /// and Open instead of re-running setup.
    pub fn new(core: Entity<Core>, setup_flow: bool, cx: &mut Context<Self>) -> Self {
        let claim_code = cx.new(|cx| {
            editor_ui::text_editor::TextEditor::new("", "Code from this Forge", cx).single_line()
        });
        let claim_subscription = cx.subscribe(
            &claim_code,
            |this, _, event: &editor_ui::text_editor::TextEditorEvent, cx| {
                if *event == editor_ui::text_editor::TextEditorEvent::Submitted {
                    this.claim(cx);
                } else {
                    cx.notify();
                }
            },
        );
        let account = core.read(cx).account().clone();
        let account_subscription = cx.subscribe(
            &account,
            |_, _, _: &editor_core::AccountEvent, cx| {
                cx.notify();
            },
        );
        Self {
            core,
            claim_code,
            show_claim: false,
            finish_error: None,
            setup_flow,
            _claim: claim_subscription,
            _account: account_subscription,
        }
    }

    fn claim(&self, cx: &mut Context<Self>) {
        let code = self.claim_code.read(cx).text().to_owned();
        controller::claim_code(&self.core, &code, cx);
        self.claim_code.update(cx, |editor, cx| {
            editor.set_text("", cx);
        });
    }

    fn refresh_binding(&self, cx: &mut Context<Self>) {
        controller::forge_status(&self.core, cx);
    }

    /// Opens the verified live route for one chosen machine. The durable
    /// record stays untouched; finishing setup is the explicit next step.
    fn connect(&self, machine_id: &str, cx: &mut Context<Self>) {
        controller::connect_machine(&self.core, machine_id, cx);
    }

    /// Opens the chosen Forge's new thread. Only offered once ready; the
    /// gate keeps this unreachable while locked.
    fn open(&self, machine_id: &str, cx: &mut Context<Self>) {
        let host = editor_core::HostId(machine_id.to_owned());
        self.core.update(cx, |core, cx| {
            core.navigate(
                editor_core::Screen::NewThread {
                    host,
                    project: None,
                },
                cx,
            );
        });
    }

    /// Persists the durable onboarding record for a machine whose Relay
    /// route is already live. Fails honestly without a live route; the
    /// message shows inline and nothing flips to ready.
    fn finish_setup(&mut self, machine_id: &str, cx: &mut Context<Self>) {
        let id = machine_id.to_owned();
        let result = self
            .core
            .update(cx, |core, cx| core.complete_setup(id, cx));
        self.finish_error = result.err().map(|error| format!("{error:#}"));
        cx.notify();
    }
}

impl Render for SetupView {
    #[expect(
        clippy::too_many_lines,
        reason = "one match over every setup stage builds one element tree"
    )]
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let account = self.core.read(cx).account().clone();
        let (email, account_id, machines, notice, relay_error, binding) = account.read_with(
            cx,
            |account, _| {
                (
                    account
                        .profile()
                        .map(|profile| profile.email.clone())
                        .unwrap_or_default(),
                    account
                        .profile()
                        .map(|profile| profile.account_id.clone())
                        .unwrap_or_default(),
                    account.onboardable_machines(),
                    account.notice().map(str::to_owned),
                    account.relay_error().map(str::to_owned),
                    account.local_binding(),
                )
            },
        );
        let direct: Vec<(String, bool)> = self
            .core
            .read(cx)
            .hosts()
            .map(|(config, host)| (config.name.clone(), host.read(cx).is_live()))
            .collect();
        let mut rows: Vec<AnyElement> = Vec::new();
        rows.push(
            div()
                .text_color(theme.colors.text_muted.to_paint())
                .child(format!(
                    "Signed in as {email}. Connect one of this account's Forges to continue."
                ))
                .into_any_element(),
        );
        for machine in &machines {
            let state = machine.last_seen_at.as_deref().map_or_else(
                || "Enrolled · never connected".to_owned(),
                |seen| format!("Last seen {seen}"),
            );
            let connected = self
                .core
                .read(cx)
                .connected_account_host(&machine.id, cx);
            let completable = account.read(cx).can_complete_setup(&machine.id);
            let id = machine.id.clone();
            let connect_id = SharedString::from(format!("entry-connect-{id}"));
            let finish_id = SharedString::from(format!("entry-finish-{id}"));
            let open_id = SharedString::from(format!("entry-open-{id}"));
            let remove_id = SharedString::from(format!("entry-remove-{id}"));
            let remove_machine = id.clone();
            let connect_machine = id.clone();
            let finish_machine = id.clone();
            let open_machine = id.clone();
            let remove_core = self.core.clone();
            let setup_flow = self.setup_flow;
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .py(px(8.0))
                    .child(
                        div()
                            .text_color(theme.colors.text.to_paint())
                            .child(machine.label.clone()),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child(state),
                    )
                    .children(connected.then(|| {
                        div()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child("Connected — this Forge has a live route.")
                            .into_any_element()
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap(px(8.0))
                            .children((setup_flow || !connected).then(|| {
                                Button::text(connect_id.clone(), theme, "Connect")
                                    .variant(ButtonVariant::Default)
                                    .disabled(!completable || connected)
                                    .debug_selector(connect_id)
                                    .on_activate(cx.listener(move |this, _, _, cx| {
                                        this.connect(&connect_machine, cx);
                                    }))
                                    .into_any_element()
                            }))
                            .children(setup_flow.then(|| {
                                Button::text(finish_id.clone(), theme, "Finish setup")
                                    .variant(ButtonVariant::Outline)
                                    .disabled(!connected)
                                    .debug_selector(finish_id)
                                    .on_activate(cx.listener(move |this, _, _, cx| {
                                        this.finish_setup(&finish_machine, cx);
                                    }))
                                    .into_any_element()
                            }))
                            .children((!setup_flow && connected).then(|| {
                                Button::text(open_id.clone(), theme, "Open")
                                    .variant(ButtonVariant::Outline)
                                    .debug_selector(open_id)
                                    .on_activate(cx.listener(move |this, _, _, cx| {
                                        this.open(&open_machine, cx);
                                    }))
                                    .into_any_element()
                            }))
                            .child(
                                Button::text(remove_id.clone(), theme, "Remove")
                                    .variant(ButtonVariant::Ghost)
                                    .debug_selector(remove_id)
                                    .on_activate(move |_, _, cx| {
                                        controller::remove_machine(
                                            &remove_core,
                                            &remove_machine,
                                            cx,
                                        );
                                    })
                                    .into_any_element(),
                            ),
                    )
                    .into_any_element(),
            );
        }
        if machines.is_empty() {
            rows.push(
                div()
                    .text_color(theme.colors.text_muted.to_paint())
                    .child("No Forges on this account yet. Pair this machine below.")
                    .into_any_element(),
            );
        }
        if !direct.is_empty() {
            let mut direct_list = div().flex().flex_col().gap(px(4.0)).child(
                div()
                    .text_color(theme.colors.text.to_paint())
                    .child("Already on this device"),
            );
            for (name, is_live) in direct {
                direct_list = direct_list.child(
                    div()
                        .text_color(theme.colors.text_muted.to_paint())
                        .child(format!(
                            "{name} · {}",
                            if is_live { "reachable" } else { "offline" }
                        )),
                );
            }
            rows.push(direct_list.into_any_element());
        }
        let bound_here = binding.as_ref().is_some_and(|binding| {
            binding.enrolled
                && binding
                    .account_id
                    .as_deref()
                    .is_some_and(|id| id == account_id)
        });
        let claimed_here = binding
            .as_ref()
            .and_then(|binding| binding.claimed_by.clone())
            .is_some_and(|id| id == account_id);
        let claimed_elsewhere = binding
            .as_ref()
            .and_then(|binding| binding.claimed_by.clone())
            .is_some_and(|id| id != account_id);
        let bound_elsewhere = binding.as_ref().is_some_and(|binding| {
            binding.enrolled
                && binding
                    .account_id
                    .as_deref()
                    .is_some_and(|id| id != account_id)
        });
        let pair_core = self.core.clone();
        if bound_here {
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(theme.colors.text.to_paint())
                            .child("This Forge"),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child(format!("Paired to {email}."))
                            .into_any_element(),
                    )
                    .into_any_element(),
            );
        } else if claimed_here {
            let confirm_core = self.core.clone();
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(theme.colors.text.to_paint())
                            .child("This Forge"),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child(format!("This Forge is claimed for {email}."))
                            .into_any_element(),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .child(
                                Button::text(
                                    "entry-forge-confirm",
                                    theme,
                                    format!("Confirm this Forge into {email}"),
                                )
                                .variant(ButtonVariant::Default)
                                .debug_selector("entry-forge-confirm")
                                .on_activate(move |_, _, cx| {
                                    controller::forge_confirm(&confirm_core, cx);
                                })
                                .into_any_element(),
                            ),
                    )
                    .into_any_element(),
            );
        } else if claimed_elsewhere || bound_elsewhere {
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(theme.colors.text.to_paint())
                            .child("This Forge"),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.destructive_text.to_paint())
                            .child(
                                "This Forge belongs to a different account. Switch accounts below.",
                            )
                            .into_any_element(),
                    )
                    .into_any_element(),
            );
        } else {
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_color(theme.colors.text.to_paint())
                            .child("This Forge"),
                    )
                    .child(
                        div()
                            .text_color(theme.colors.text_muted.to_paint())
                            .child("This Forge isn't paired yet.")
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .child(
                                Button::text("entry-pair", theme, "Pair this machine")
                                    .variant(ButtonVariant::Default)
                                    .debug_selector("entry-pair")
                                    .on_activate(move |_, _, cx| {
                                        controller::enroll_this_forge(&pair_core, cx);
                                    })
                                    .into_any_element(),
                            ),
                    )
                    .into_any_element(),
            );
        }
        let show_claim = self.show_claim;
        rows.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(
                    Button::text(
                        "entry-toggle-claim",
                        theme,
                        if show_claim {
                            "Hide"
                        } else {
                            "Use a code from another machine"
                        },
                    )
                    .variant(ButtonVariant::Ghost)
                    .debug_selector("entry-toggle-claim")
                    .on_activate(cx.listener(|this, _, _, cx| {
                        this.show_claim = !this.show_claim;
                        cx.notify();
                    }))
                    .into_any_element(),
                )
                .into_any_element(),
        );
        if show_claim {
            rows.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id("entry-claim-code")
                            .flex_1()
                            .min_w_0()
                            .child(self.claim_code.clone()),
                    )
                    .child(
                        Button::text("entry-claim", theme, "Claim")
                            .variant(ButtonVariant::Default)
                            .debug_selector("entry-claim")
                            .on_activate(cx.listener(|this, _, _, cx| this.claim(cx)))
                            .into_any_element(),
                    )
                    .into_any_element(),
            );
        }
        if let Some(error) = self.finish_error.clone() {
            rows.push(
                div()
                    .text_color(theme.colors.destructive_text.to_paint())
                    .child(error)
                    .into_any_element(),
            );
        }
        if let Some(notice) = notice {
            rows.push(
                div()
                    .text_color(theme.colors.text_muted.to_paint())
                    .child(notice)
                    .into_any_element(),
            );
        }
        if let Some(error) = relay_error {
            rows.push(
                div()
                    .text_color(theme.colors.destructive_text.to_paint())
                    .child(error)
                    .into_any_element(),
            );
        }
        rows.push(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap(px(8.0))
                .child(
                    Button::text("entry-setup-refresh", theme, "Check again")
                        .variant(ButtonVariant::Ghost)
                        .debug_selector("entry-setup-refresh")
                        .on_activate(cx.listener(|this, _, _, cx| this.refresh_binding(cx)))
                        .into_any_element(),
                )
                .child(
                    Button::text("entry-setup-use-another", theme, "Use another account")
                        .variant(ButtonVariant::Ghost)
                        .debug_selector("entry-setup-use-another")
                        .on_activate(cx.listener(|this, _, _, cx| {
                            controller::sign_out(&this.core, false, cx);
                        }))
                        .into_any_element(),
                )
                .into_any_element(),
        );
        div()
            .id("entry-setup")
            .debug_selector(|| "entry-setup".into())
            .flex()
            .flex_col()
            .children(rows)
    }
}
