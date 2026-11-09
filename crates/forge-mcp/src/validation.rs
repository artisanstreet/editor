use serde_json::Value;

pub fn validate(schema: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > 32 {
        return Err(format!("{path}: input nesting exceeds 32 levels"));
    }
    if let Some(alternatives) = schema.get("oneOf").and_then(Value::as_array)
        && alternatives
            .iter()
            .filter(|schema| validate(schema, value, path, depth + 1).is_ok())
            .count()
            != 1
    {
        return Err(format!(
            "{path}: value must match exactly one supported shape"
        ));
    }
    if schema
        .get("const")
        .is_some_and(|expected| expected != value)
    {
        return Err(format!("{path}: unexpected value"));
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|allowed| !allowed.contains(value))
    {
        return Err(format!("{path}: value is outside the allowed choices"));
    }
    let matches_type = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => true,
    };
    let valid_type = match schema.get("type") {
        Some(Value::String(kind)) => matches_type(kind),
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).any(matches_type),
        _ => true,
    };
    if !valid_type {
        return Err(format!("{path}: incorrect value type"));
    }
    validate_bounds(schema, value, path)?;
    if let Some(object) = value.as_object() {
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(required) {
                return Err(format!("{path}.{required}: required value is missing"));
            }
        }
        for (key, value) in object {
            match schema.get("properties").and_then(|props| props.get(key)) {
                Some(property) => validate(property, value, &format!("{path}.{key}"), depth + 1)?,
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => {
                        return Err(format!("{path}.{key}: unknown argument"));
                    }
                    Some(property @ Value::Object(_)) => {
                        validate(property, value, &format!("{path}.{key}"), depth + 1)?;
                    }
                    _ => {}
                },
            }
        }
    }
    if let (Some(array), Some(items)) = (value.as_array(), schema.get("items")) {
        for (index, value) in array.iter().enumerate() {
            validate(items, value, &format!("{path}[{index}]"), depth + 1)?;
        }
    }
    Ok(())
}

fn validate_bounds(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    for (property, lower) in [("minimum", true), ("maximum", false)] {
        if let (Some(bound), Some(actual)) =
            (schema.get(property).and_then(Value::as_f64), value.as_f64())
            && ((lower && actual < bound) || (!lower && actual > bound))
        {
            return Err(format!("{path}: violates {property} {bound}"));
        }
    }
    for (property, actual, lower) in [
        ("minLength", value.as_str().map(|s| s.chars().count()), true),
        (
            "maxLength",
            value.as_str().map(|s| s.chars().count()),
            false,
        ),
        ("minItems", value.as_array().map(Vec::len), true),
        ("maxItems", value.as_array().map(Vec::len), false),
    ] {
        if let (Some(bound), Some(actual)) = (schema.get(property).and_then(Value::as_u64), actual)
            && ((lower && (actual as u64) < bound) || (!lower && (actual as u64) > bound))
        {
            return Err(format!("{path}: violates {property} {bound}"));
        }
    }
    Ok(())
}
