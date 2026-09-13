//! Public host summaries only. Credential custody and scanning stay on worker threads.
use super::*;
use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

#[derive(Default)]
struct Catalog {
    entries: Vec<(String, PathBuf)>,
    details: HashMap<PathBuf, (String, HostPresentation)>,
}

fn catalog() -> &'static RwLock<Catalog> {
    static CATALOG: OnceLock<RwLock<Catalog>> = OnceLock::new();
    CATALOG.get_or_init(Default::default)
}

pub(super) fn entries() -> Vec<(String, PathBuf)> {
    catalog().read().unwrap().entries.clone()
}

pub(crate) fn label(home: Option<&Path>) -> String {
    home.map_or_else(
        || "This computer".into(),
        |home| {
            catalog()
                .read()
                .unwrap()
                .details
                .get(home)
                .map_or_else(|| "Unavailable host".into(), |(name, _)| name.clone())
        },
    )
}

pub(super) fn presentation(home: &Path) -> Option<HostPresentation> {
    catalog()
        .read()
        .unwrap()
        .details
        .get(home)
        .map(|(_, detail)| detail.clone())
}

pub(crate) fn same_host(left: Option<&Path>, right: Option<&Path>) -> bool {
    if left == right {
        return true;
    }
    let cache = catalog().read().unwrap();
    match (
        left.and_then(|p| cache.details.get(p)),
        right.and_then(|p| cache.details.get(p)),
    ) {
        (Some((_, a)), Some((_, b))) => a.avatar_seed == b.avatar_seed,
        _ => false,
    }
}

/// Call only on a background executor. Never hold the cache lock during IO.
pub(crate) fn refresh(selected: Option<&Path>) {
    let Ok(entries) = hosts::list() else {
        return;
    };
    let mut paths: Vec<_> = entries.iter().map(|(_, path)| path.clone()).collect();
    if let Some(path) = selected {
        paths.push(path.to_owned());
    }
    paths.sort();
    paths.dedup();
    let mut details = HashMap::new();
    for home in paths {
        if let Ok(host) = hosts::read_private(&home, "host.json")
            .and_then(|bytes| hosts::HostInvitation::decode(&bytes))
        {
            details.insert(
                home.clone(),
                (host.name.clone(), super::read_presentation(Some(&home))),
            );
        }
    }
    let mut cache = catalog().write().unwrap();
    cache.entries = entries;
    // Keep identities for retained sessions using older invitation incarnations.
    cache.details.extend(details);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cached_identity_survives_incarnation_changes_without_files() {
        let old = PathBuf::from("/test/catalog-old");
        let new = PathBuf::from("/test/catalog-new");
        {
            let mut cache = catalog().write().unwrap();
            for path in [&old, &new] {
                cache.details.insert(
                    path.clone(),
                    (
                        "Ubuntu".into(),
                        HostPresentation {
                            wsl_distribution: None,
                            subtitle: "192.0.2.1".into(),
                            avatar_seed: "same-certificate".into(),
                        },
                    ),
                );
            }
        }
        assert!(same_host(Some(&old), Some(&new)));
        assert_eq!(label(Some(&old)), "Ubuntu");
        assert_eq!(presentation(&new).unwrap().subtitle, "192.0.2.1");
        assert!(!same_host(Some(&old), Some(Path::new("/missing"))));
        let mut cache = catalog().write().unwrap();
        cache.details.remove(&old);
        cache.details.remove(&new);
    }
}
