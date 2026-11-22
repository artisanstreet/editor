pub const MODEL_MENU: &str = "composer-model-scroll";

pub fn key_hash(key: &str) -> aqua_ui::tree::KeyHash {
    thread_local! {
        static HASHES: std::cell::RefCell<std::collections::HashMap<String, aqua_ui::tree::KeyHash>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    HASHES.with(|hashes| {
        let mut hashes = hashes.borrow_mut();
        if let Some(hash) = hashes.get(key) {
            *hash
        } else {
            if hashes.len() >= 65_536 {
                hashes.clear();
            }
            let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
            hashes.insert(key.to_owned(), hash);
            hash
        }
    })
}
