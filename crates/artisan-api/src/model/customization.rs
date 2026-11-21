use super::{ChoiceSemantic, ModelAxis, ModelCustomization};

#[derive(Debug, thiserror::Error)]
pub enum ModelCustomizationsError {
    #[error("decode the model customizations")]
    Decode(#[source] serde_json::Error),
    #[error("model customization version {version} is not supported; use version 1")]
    Version { version: u32 },
    #[error("{count} model customization rules were given; at most 1024 are allowed")]
    TooManyRules { count: usize },
    #[error(
        "customization {rule:?}: names, model IDs, native IDs and labels must be nonempty and at most 256 bytes"
    )]
    BadName { rule: String },
    #[error(
        "customization {rule:?} cannot change an option's execution domain or manufacture a default"
    )]
    ExecutionDomain { rule: String },
}

pub(crate) fn validate_model_customizations(
    rules: &[ModelCustomization],
) -> Result<(), ModelCustomizationsError> {
    if rules.len() > 1024 {
        return Err(ModelCustomizationsError::TooManyRules { count: rules.len() });
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
            return Err(ModelCustomizationsError::BadName {
                rule: rule.name.chars().take(256).collect(),
            });
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
            return Err(ModelCustomizationsError::ExecutionDomain {
                rule: rule.name.clone(),
            });
        }
    }
    Ok(())
}
