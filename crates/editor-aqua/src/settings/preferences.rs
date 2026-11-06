use serde_json::{Map, Value};

#[derive(Clone, Debug)]
pub struct SettingRequest {
    pub key: &'static str,
    pub value: Value,
    previous: Option<Value>,
}

pub fn begin(settings: &mut Map<String, Value>, key: &'static str, value: Value) -> SettingRequest {
    let previous = settings.insert(key.to_owned(), value.clone());
    SettingRequest {
        key,
        value,
        previous,
    }
}

pub fn rollback(settings: &mut Map<String, Value>, request: SettingRequest) {
    match request.previous {
        Some(previous) => {
            settings.insert(request.key.to_owned(), previous);
        }
        None => {
            settings.remove(request.key);
        }
    }
}
