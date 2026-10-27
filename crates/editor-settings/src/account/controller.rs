use editor_core::Core;
use gpui::{App, Entity};

pub fn request_link(core: &Entity<Core>, email: &str, cx: &mut App) {
    let email = email.to_owned();
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, |account, cx| account.request_link(email, cx));
    });
}

pub fn dismiss_error(core: &Entity<Core>, cx: &mut App) {
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, editor_core::Account::dismiss_error);
    });
}

pub fn split_second_factor(code: &str) -> (Option<String>, Option<String>) {
    let code = code.trim().to_owned();
    if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
        (Some(code), None)
    } else if code.is_empty() {
        (None, None)
    } else {
        (None, Some(code))
    }
}

pub fn finish_sign_in(core: &Entity<Core>, input: &str, cx: &mut App) {
    let (totp, recovery) = split_second_factor(input);
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, |account, cx| account.finish_sign_in(totp, recovery, cx));
    });
}

pub fn sign_out(core: &Entity<Core>, revoke_all: bool, cx: &mut App) {
    core.update(cx, |core, cx| {
        core.account().update(cx, |account, cx| {
            if revoke_all {
                account.sign_out_everywhere(cx);
            } else {
                account.sign_out(cx);
            }
        });
    });
}

pub fn refresh(core: &Entity<Core>, cx: &mut App) {
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, editor_core::Account::refresh);
    });
}

pub fn claim_code(core: &Entity<Core>, raw: &str, cx: &mut App) {
    let normalized = raw.to_owned();
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, |account, cx| account.claim_code(normalized, cx));
    });
}

pub fn enroll_this_forge(core: &Entity<Core>, cx: &mut App) {
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, editor_core::Account::enroll_this_forge);
    });
}

pub fn forge_status(core: &Entity<Core>, cx: &mut App) {
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, editor_core::Account::forge_status);
    });
}

pub fn forge_confirm(core: &Entity<Core>, cx: &mut App) {
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, editor_core::Account::forge_confirm);
    });
}

pub fn connect_machine(core: &Entity<Core>, machine_id: &str, cx: &mut App) {
    let id = machine_id.to_owned();
    let account = core.read_with(cx, |core, _| core.account().clone());
    account.update(cx, |account, cx| {
        account.connect_machine(id, core, cx);
    });
}

pub fn remove_machine(core: &Entity<Core>, machine_id: &str, cx: &mut App) {
    let id = machine_id.to_owned();
    core.update(cx, |core, cx| {
        core.account()
            .update(cx, |account, cx| account.remove_machine(id, cx));
    });
}

pub fn account_identity(core: &Entity<Core>, cx: &App) -> Option<(String, String, String)> {
    core.read(cx).account().read(cx).profile().map(|profile| {
        (
            profile.account_id.clone(),
            profile.email.clone(),
            profile.display_name.clone(),
        )
    })
}
