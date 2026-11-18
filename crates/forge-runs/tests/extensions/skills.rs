use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn a_damaged_disabled_skills_setting_keeps_skills_out_instead_of_enabling_them() {
    let fixture = fixture(Arc::new(|_turn, _io| Box::pin(async { Outcome::done() }))).await;
    let hub = &fixture.hub;
    write_skill(
        &hub.extensions().skills_dir(),
        "review",
        "Reviews the diff.",
    );
    hub.extensions()
        .set_skill_enabled("local.review", false)
        .await
        .unwrap();
    hub.store()
        .set_setting(
            DISABLED_SKILLS_KEY.to_owned(),
            serde_json::json!({ "local.review": true }),
        )
        .await
        .unwrap();

    let refused = hub
        .extensions()
        .reconcile_skills()
        .await
        .expect_err("a damaged setting is reported, not read as no skills turned off");
    assert!(
        refused.chain().any(|cause| cause
            .downcast_ref::<extensions::DisabledSkillsError>()
            .is_some()),
        "the failure names the setting: {refused:#}"
    );
    assert_eq!(
        claude_skills(hub),
        ["artisan-editor -> artisan-editor"],
        "the skill the user turned off stays out of the engine"
    );
}
