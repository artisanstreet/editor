//! This machine's Forge enrollment, as the installed `ae` reports it, and
//! the actions that move it along: enroll, claim, confirm.

use anyhow::{Context as _, Result, bail};
use artisan_account::network::AccountProfile;
use gpui::Context;
use serde::Deserialize;

use super::{Account, run_http};
use crate::ae::Ae;

/// What `ae account status --json` says, before comparing it to an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AeBinding {
    Unenrolled,
    Pending,
    Claimed {
        account_id: String,
        email: Option<String>,
    },
    Enrolled {
        machine_id: String,
        account_id: String,
    },
}

/// This machine's Forge, seen from the signed-in account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LocalForge {
    /// Not read yet, or not signed in.
    Unknown,
    Unenrolled,
    /// An enrollment is started and waits for its code to be claimed.
    Pending,
    /// Claimed into this account; confirm it on this machine next.
    Claimed {
        email: Option<String>,
    },
    /// Enrolled to this account: the Editor connects to it directly.
    Enrolled {
        machine_id: String,
    },
    /// Enrolled to this account, then removed from it: pair it again.
    Revoked {
        machine_id: String,
    },
    /// Claimed or enrolled by a different account.
    OtherAccount,
}

fn local_forge(binding: &AeBinding, profile: &AccountProfile) -> LocalForge {
    let ours = |account_id: &str| account_id == profile.account_id;
    match binding {
        AeBinding::Unenrolled => LocalForge::Unenrolled,
        AeBinding::Pending => LocalForge::Pending,
        AeBinding::Claimed { account_id, email } if ours(account_id) => LocalForge::Claimed {
            email: email.clone(),
        },
        AeBinding::Enrolled {
            machine_id,
            account_id,
        } if ours(account_id) => {
            let revoked = profile
                .machines
                .iter()
                .any(|machine| &machine.id == machine_id && machine.revoked_at.is_some());
            if revoked {
                LocalForge::Revoked {
                    machine_id: machine_id.clone(),
                }
            } else {
                LocalForge::Enrolled {
                    machine_id: machine_id.clone(),
                }
            }
        }
        AeBinding::Claimed { .. } | AeBinding::Enrolled { .. } => LocalForge::OtherAccount,
    }
}

#[derive(Deserialize)]
struct StatusReport {
    enrolled: bool,
    #[serde(default)]
    machine_id: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

#[derive(Deserialize)]
struct Enrollment {
    code: String,
}

fn parse_status(printed: &str) -> Result<AeBinding> {
    let status: StatusReport =
        serde_json::from_str(printed).context("read what `ae account status` printed")?;
    Ok(
        match (
            status.enrolled,
            status.machine_id,
            status.account_id,
            status.status.as_deref(),
        ) {
            (true, Some(machine_id), Some(account_id), _) => AeBinding::Enrolled {
                machine_id,
                account_id,
            },
            (false, _, Some(account_id), Some("claimed" | "confirmed")) => AeBinding::Claimed {
                account_id,
                email: status.email,
            },
            (false, _, _, Some("pending")) => AeBinding::Pending,
            (false, _, _, Some("none" | "expired" | "voided")) => AeBinding::Unenrolled,
            _ => bail!("`ae account status` reported a state this Editor doesn't know: {printed}"),
        },
    )
}

async fn read_status(ae: &Ae, core: &str) -> Result<AeBinding> {
    parse_status(
        &ae.run(&["account", "--core", core, "status", "--json"])
            .await?,
    )
}

impl Account {
    #[must_use]
    pub fn local_forge(&self) -> LocalForge {
        match (&self.binding, self.profile()) {
            (Some(binding), Some(profile)) => local_forge(binding, profile),
            _ => LocalForge::Unknown,
        }
    }

    /// Re-reads this machine's enrollment from `ae`.
    pub fn refresh_local_forge(&mut self, cx: &mut Context<Self>) {
        let (ae, core) = (self.services.ae.clone(), self.base_url.clone());
        self.forge = Some(cx.spawn(async move |this, cx| {
            let read = read_status(&ae, &core).await;
            // Err: the Account is gone.
            this.update(cx, |this, cx| this.set_binding(read, cx)).ok();
        }));
    }

    fn set_binding(&mut self, read: Result<AeBinding>, cx: &mut Context<Self>) {
        self.forge = None;
        match read {
            Ok(binding) => {
                self.binding = Some(binding);
                Self::changed(cx);
            }
            // The last binding stays: one failed `ae` run doesn't unpair the Forge.
            Err(error) => self.report(&error, cx),
        }
    }

    /// Starts an enrollment of this machine's Forge with `ae` and claims its
    /// code into this account; confirming stays an explicit step.
    pub fn enroll_this_forge(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let replace = matches!(self.local_forge(), LocalForge::Revoked { .. });
        let (ae, core_url, http) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.http.clone(),
        );
        self.last_error = None;
        self.forge = Some(cx.spawn(async move |this, cx| {
            let enrolled = async {
                let mut args = vec!["account", "--core", &core_url, "enroll", "--json"];
                if replace {
                    args.push("--replace");
                }
                let enrollment: Enrollment = serde_json::from_str(&ae.run(&args).await?)
                    .context("read what `ae account enroll` printed")?;
                let code = artisan_account::normalise_code(&enrollment.code);
                run_http(async move { http.claim_enrollment(&session, &code).await }).await?;
                read_status(&ae, &core_url).await
            }
            .await
            .context("enroll this machine's Forge");
            // Err: the Account is gone.
            this.update(cx, |this, cx| this.set_binding(enrolled, cx))
                .ok();
        }));
    }

    /// Claims a code another machine's `ae account enroll` printed.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the code comes straight from a text field"
    )]
    pub fn claim_code(&mut self, code: String, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let code = artisan_account::normalise_code(&code);
        let (ae, core_url, http) = (
            self.services.ae.clone(),
            self.base_url.clone(),
            self.http.clone(),
        );
        self.last_error = None;
        self.forge = Some(cx.spawn(async move |this, cx| {
            let claimed = async {
                run_http(async move { http.claim_enrollment(&session, &code).await }).await?;
                read_status(&ae, &core_url).await
            }
            .await
            .context("claim the Forge's code");
            // Err: the Account is gone.
            this.update(cx, |this, cx| this.set_binding(claimed, cx))
                .ok();
        }));
    }

    /// Confirms on this machine that its Forge joins the signed-in account.
    pub fn confirm_forge(&mut self, cx: &mut Context<Self>) {
        let Some(account_id) = self.profile().map(|profile| profile.account_id.clone()) else {
            return;
        };
        let (ae, core) = (self.services.ae.clone(), self.base_url.clone());
        self.last_error = None;
        self.forge = Some(cx.spawn(async move |this, cx| {
            let confirmed = async {
                ae.run(&[
                    "account",
                    "--core",
                    &core,
                    "confirm",
                    "--yes",
                    "--account-id",
                    &account_id,
                ])
                .await?;
                read_status(&ae, &core).await
            }
            .await
            .context("confirm this machine's Forge");
            // Err: the Account is gone.
            this.update(cx, |this, cx| {
                let ok = confirmed.is_ok();
                this.set_binding(confirmed, cx);
                if ok {
                    this.sync(cx);
                }
            })
            .ok();
        }));
    }
}
