use crate::core::{
    EditorFiles,
    access::{self, OnboardingRecord},
};

pub fn save_onboarding_for_tests(
    files: &EditorFiles,
    base_url: &str,
    account_id: &str,
    machine_id: &str,
) -> anyhow::Result<()> {
    access::save_onboarding(
        &files.dir,
        &OnboardingRecord {
            account_id: account_id.to_owned(),
            base_url: base_url.to_owned(),
            machine_id: machine_id.to_owned(),
        },
    )
}
