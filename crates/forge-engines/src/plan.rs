pub(crate) fn chatgpt(kind: &str) -> Option<&'static str> {
    match kind {
        "free" => Some("ChatGPT Free"),
        "go" => Some("ChatGPT Go"),
        "plus" => Some("ChatGPT Plus"),
        "prolite" => Some("ChatGPT Pro 100"),
        "pro" => Some("ChatGPT Pro 200"),
        "promax" => Some("ChatGPT Pro 500"),
        "self_serve_business_prolite" => Some("ChatGPT Business Premium"),
        "team" | "business" | "self_serve_business_usage_based" => Some("ChatGPT Business"),
        "enterprise" | "ent26" | "enterprise_cbp_automation" | "enterprise_cbp_usage_based" => {
            Some("ChatGPT Enterprise")
        }
        "edu" | "edu_plus" | "edu_pro" => Some("ChatGPT Edu"),
        _ => None,
    }
}

pub(crate) fn claude(subscription: &str, tier: Option<&str>) -> Option<&'static str> {
    let twenty_fold = tier.is_some_and(|tier| tier.contains("20x"));
    match subscription.to_ascii_lowercase().as_str() {
        "max" if twenty_fold => Some("Claude Ultra"),
        "max" => Some("Claude Max"),
        "ultra" => Some("Claude Ultra"),
        "pro" => Some("Claude Pro"),
        "team" => Some("Claude Team"),
        "enterprise" => Some("Claude Enterprise"),
        "free" => Some("Claude Free"),
        _ => None,
    }
}

#[cfg(test)]
#[path = "../tests/plan/unit.rs"]
mod tests;
