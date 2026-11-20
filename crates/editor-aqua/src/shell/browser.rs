use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browser {
    pub name: String,
    pub program: PathBuf,
    pub https_handler: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    Chromium,
    Gecko,
}

const GECKO: [&str; 6] = [
    "firefox",
    "librewolf",
    "waterfox",
    "zen",
    "floorp",
    "mullvadbrowser",
];
const QUERY_LIMIT: usize = 2000;

#[must_use]
pub fn program_of(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let program = if let Some(rest) = command.strip_prefix('"') {
        rest.split('"').next()?
    } else {
        let lower = command.to_ascii_lowercase();
        let end = lower.find(".exe").map_or(command.len(), |at| at + 4);
        command
            .get(..end)
            .expect("invariant: ASCII lowercasing keeps the .exe offset on a char boundary")
    };
    (!program.trim().is_empty()).then(|| PathBuf::from(program.trim()))
}

#[must_use]
pub fn pick(
    installed: Vec<Browser>,
    preferred: Option<&str>,
    default_handler: Option<&str>,
) -> Option<Browser> {
    let named = preferred.and_then(|name| {
        installed
            .iter()
            .position(|browser| browser.name.eq_ignore_ascii_case(name.trim()))
    });
    let default = default_handler.and_then(|handler| {
        installed.iter().position(|browser| {
            browser
                .https_handler
                .as_deref()
                .is_some_and(|own| own.eq_ignore_ascii_case(handler))
        })
    });
    let index = named
        .or(default)
        .or_else(|| (!installed.is_empty()).then_some(0))?;
    installed.into_iter().nth(index)
}

impl Browser {
    fn family(&self) -> Family {
        let path = self.program.to_string_lossy().to_ascii_lowercase();
        let file = path
            .rsplit_once(['\\', '/'])
            .map_or(path.as_str(), |(_, file)| file);
        let stem = file.strip_suffix(".exe").unwrap_or(file);
        if GECKO.contains(&stem) {
            Family::Gecko
        } else {
            Family::Chromium
        }
    }

    #[must_use]
    pub fn search_arguments(&self, text: &str) -> Vec<String> {
        let query: String = text
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(QUERY_LIMIT)
            .collect();
        match self.family() {
            Family::Chromium => vec![format!("? {query}")],
            Family::Gecko => vec!["-search".to_owned(), query],
        }
    }

    pub(crate) fn search(&self, text: &str) -> std::io::Result<()> {
        std::process::Command::new(&self.program)
            .args(self.search_arguments(text))
            .spawn()
            .map(drop)
    }
}

#[cfg(not(windows))]
#[must_use]
pub(crate) fn resolve(_preferred: Option<&str>) -> Option<Browser> {
    None
}

#[cfg(windows)]
#[must_use]
pub(crate) fn resolve(preferred: Option<&str>) -> Option<Browser> {
    native::resolve(preferred)
}

#[must_use]
pub(crate) fn icon(program: &Path) -> Option<aqua::prelude::FileIcon> {
    aqua::prelude::file_icon(program, 32)
}

#[cfg(windows)]
mod native {
    use super::{Browser, pick, program_of};
    use windows_registry::{CLASSES_ROOT, CURRENT_USER, Key, LOCAL_MACHINE};

    const CLIENTS: &str = r"Software\Clients\StartMenuInternet";
    const CHOICE: &str =
        r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice";
    const NOT_FOUND: i32 = 0x8007_0002_u32.cast_signed();

    fn present<T>(result: windows_registry::Result<T>, entry: &str) -> Option<T> {
        match result {
            Ok(value) => Some(value),
            Err(error) if error.code().0 == NOT_FOUND => None,
            Err(error) => {
                tracing::warn!(%error, entry, "skipped a browser registry entry it could not read");
                None
            }
        }
    }

    fn command(key: &Key, id: &str) -> Option<std::path::PathBuf> {
        let open = present(key.open(r"shell\open\command"), id)?;
        program_of(&present(open.get_string(""), id)?)
    }

    fn registered(clients: &Key, id: &str) -> Option<Browser> {
        let key = present(clients.open(id), id)?;
        let capabilities = present(key.open("Capabilities"), id);
        let name = capabilities
            .as_ref()
            .and_then(|capabilities| present(capabilities.get_string("ApplicationName"), id))
            .or_else(|| present(key.get_string(""), id))
            .filter(|name| !name.trim().is_empty())?;
        let program = command(&key, id)?;
        let https_handler = capabilities
            .and_then(|capabilities| present(capabilities.open("URLAssociations"), id))
            .and_then(|associations| present(associations.get_string("https"), id));
        Some(Browser {
            name,
            program,
            https_handler,
        })
    }

    fn installed() -> Vec<Browser> {
        [CURRENT_USER, LOCAL_MACHINE]
            .into_iter()
            .filter_map(|root| present(root.open(CLIENTS), CLIENTS))
            .flat_map(|clients| {
                present(clients.keys(), CLIENTS)
                    .into_iter()
                    .flatten()
                    .filter_map(|id| registered(&clients, &id))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn handler(id: &str) -> Option<Browser> {
        let key = present(CLASSES_ROOT.open(id), id)?;
        let program = command(&key, id)?;
        let name = present(key.open("Application"), id)
            .and_then(|application| present(application.get_string("ApplicationName"), id))
            .or_else(|| {
                program
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
            })?;
        Some(Browser {
            name,
            program,
            https_handler: Some(id.to_owned()),
        })
    }

    pub(super) fn resolve(preferred: Option<&str>) -> Option<Browser> {
        let default = present(CURRENT_USER.open(CHOICE), CHOICE)
            .and_then(|choice| present(choice.get_string("ProgId"), CHOICE));
        let mut browsers = installed();
        let known = default.as_deref().is_some_and(|id| {
            browsers.iter().any(|browser| {
                browser
                    .https_handler
                    .as_deref()
                    .is_some_and(|own| own.eq_ignore_ascii_case(id))
            })
        });
        if !known && let Some(browser) = default.as_deref().and_then(handler) {
            browsers.push(browser);
        }
        pick(browsers, preferred, default.as_deref())
    }
}
