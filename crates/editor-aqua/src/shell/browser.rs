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

const GECKO: [&str; 6] = ["firefox", "librewolf", "waterfox", "zen", "floorp", "mullvadbrowser"];
const QUERY_LIMIT: usize = 2000;

#[must_use]
pub fn program_of(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next()?,
        None => {
            let lower = command.to_ascii_lowercase();
            let end = lower.find(".exe").map_or(command.len(), |at| at + 4);
            &command[..end]
        }
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
    let index = named.or(default).or((!installed.is_empty()).then_some(0))?;
    installed.into_iter().nth(index)
}

impl Browser {
    fn family(&self) -> Family {
        let path = self.program.to_string_lossy().to_ascii_lowercase();
        let file = path.rsplit(['\\', '/']).next().unwrap_or_default();
        let stem = file.strip_suffix(".exe").unwrap_or(file);
        match GECKO.contains(&stem) {
            true => Family::Gecko,
            false => Family::Chromium,
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

    pub fn search(&self, text: &str) -> std::io::Result<()> {
        std::process::Command::new(&self.program)
            .args(self.search_arguments(text))
            .spawn()
            .map(drop)
    }
}

#[cfg(not(windows))]
#[must_use]
pub fn resolve(_preferred: Option<&str>) -> Option<Browser> {
    None
}

#[cfg(windows)]
#[must_use]
pub fn resolve(preferred: Option<&str>) -> Option<Browser> {
    native::resolve(preferred)
}

#[must_use]
pub fn icon(program: &Path) -> Option<aqua::prelude::FileIcon> {
    aqua::prelude::file_icon(program, 32)
}

#[cfg(windows)]
mod native {
    use super::{Browser, pick, program_of};
    use windows_registry::{CLASSES_ROOT, CURRENT_USER, Key, LOCAL_MACHINE};

    const CLIENTS: &str = r"Software\Clients\StartMenuInternet";
    const CHOICE: &str =
        r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice";

    fn registered(clients: &Key, id: &str) -> Option<Browser> {
        let key = clients.open(id).ok()?;
        let capabilities = key.open("Capabilities").ok();
        let name = capabilities
            .as_ref()
            .and_then(|capabilities| capabilities.get_string("ApplicationName").ok())
            .or_else(|| key.get_string("").ok())
            .filter(|name| !name.trim().is_empty())?;
        let program = program_of(&key.open(r"shell\open\command").ok()?.get_string("").ok()?)?;
        let https_handler = capabilities
            .and_then(|capabilities| capabilities.open("URLAssociations").ok())
            .and_then(|associations| associations.get_string("https").ok());
        Some(Browser {
            name,
            program,
            https_handler,
        })
    }

    fn installed() -> Vec<Browser> {
        [CURRENT_USER, LOCAL_MACHINE]
            .into_iter()
            .filter_map(|root| root.open(CLIENTS).ok())
            .flat_map(|clients| {
                let ids: Vec<String> = clients
                    .keys()
                    .map(|keys| keys.collect())
                    .unwrap_or_default();
                ids.into_iter()
                    .filter_map(|id| registered(&clients, &id))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    fn handler(id: &str) -> Option<Browser> {
        let key = CLASSES_ROOT.open(id).ok()?;
        let program = program_of(&key.open(r"shell\open\command").ok()?.get_string("").ok()?)?;
        let name = key
            .open("Application")
            .ok()
            .and_then(|application| application.get_string("ApplicationName").ok())
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

    pub fn resolve(preferred: Option<&str>) -> Option<Browser> {
        let default = CURRENT_USER
            .open(CHOICE)
            .ok()
            .and_then(|choice| choice.get_string("ProgId").ok());
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
