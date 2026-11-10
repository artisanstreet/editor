use super::{ChoiceSemantic, ModelAxis, ModelCustomization};

pub fn validate_model_customizations(rules: &[ModelCustomization]) -> Result<(), String> {
    if rules.len() > 1024 {
        return Err("At most 1024 model customization rules are allowed.".into());
    }
    for rule in rules {
        if rule.name.trim().is_empty()
            || rule.native_id.trim().is_empty()
            || rule.name.len() > 256
            || rule.native_id.len() > 256
            || rule
                .label
                .as_ref()
                .is_some_and(|label| label.trim().is_empty() || label.len() > 256)
            || rule
                .scope
                .model
                .as_ref()
                .is_some_and(|model| model.trim().is_empty() || model.len() > 256)
        {
            return Err("Customization names, model IDs, native IDs and labels must be nonempty and at most 256 bytes.".into());
        }
        let allowed = matches!(
            (&rule.axis, &rule.semantic),
            (_, None)
                | (
                    ModelAxis::Thinking
                        | ModelAxis::Activation
                        | ModelAxis::Special
                        | ModelAxis::Variant,
                    Some(
                        ChoiceSemantic::Intensity(_)
                            | ChoiceSemantic::Activation(_)
                            | ChoiceSemantic::Special
                            | ChoiceSemantic::Native,
                    ),
                )
                | (
                    ModelAxis::Speed,
                    Some(ChoiceSemantic::Speed(_) | ChoiceSemantic::Native)
                )
        );
        if !allowed {
            return Err("A customization cannot change an option's execution domain or manufacture a default.".into());
        }
    }
    Ok(())
}
