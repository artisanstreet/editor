use std::collections::BTreeMap;
use std::fmt::Write as _;

const CARGO_RULE: &str =
    "\n================================================================================\n";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Licence {
    pub name: &'static str,
    pub text: &'static str,
}

#[derive(Debug, PartialEq, Eq)]
struct Entry {
    name: String,
    text: String,
}

enum Section<'a> {
    Bundled { name: String, text: &'a str },
    Crates(&'a str),
    Attestation,
}

fn section<'a>(label: &str, text: &'a str) -> Section<'a> {
    let file = label.rsplit('/').next().unwrap_or(label);
    let named = |name: &str| Section::Bundled {
        name: name.to_owned(),
        text,
    };
    match label {
        "notices/editor.txt" | "notices/ae.txt" | "notices/forge.txt" => Section::Crates(text),
        "LICENSE.md" => named("Artisan Editor"),
        "crates/editor-assets/licenses/aqua-BSD.txt" => named("Aqua"),
        "crates/editor-assets/licenses/aqua-taffy-MIT.txt" => named("Taffy"),
        "crates/editor-assets/licenses/jetbrains-2023-LICENSE.md" => named("JetBrains file icons"),
        "crates/editor-assets/licenses/lobe-icons-MIT.txt" => named("Lobe Icons"),
        "crates/editor-assets/licenses/simple-icons-CC0.txt" => named("Simple Icons"),
        "crates/editor-assets/licenses/spline-sans-OFL.txt" => named("Spline Sans"),
        "crates/editor-assets/licenses/spline-sans-mono-OFL.txt" => named("Spline Sans Mono"),
        "crates/editor-assets/licenses/svgl-svelte-LICENSE.txt" => named("SVGL"),
        "crates/editor-assets/licenses/tabler-MIT.txt" => named("Tabler Icons"),
        "vendor/highlighting/NOTICE.txt" => named("Syntax highlighting grammars"),
        "vendor/mermaid-rs-renderer/LICENSE" => named("mermaid-rs-renderer"),
        "notices/fff-MIT.txt" => named("fff"),
        _ if file.ends_with("NOTES.md")
            || file.ends_with("README.md")
            || std::path::Path::new(file)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json")) =>
        {
            Section::Attestation
        }
        _ => named(file.split('.').next().unwrap_or(file)),
    }
}

fn sections(notices: &str) -> Vec<(&str, &str)> {
    let mut found: Vec<(&str, usize, usize)> = Vec::new();
    let mut offset = 0;
    for line in notices.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(label) = bare
            .strip_prefix("==== ")
            .and_then(|rest| rest.strip_suffix(" ===="))
        {
            if let Some(last) = found.last_mut() {
                last.2 = offset;
            }
            found.push((label, offset + line.len(), notices.len()));
        }
        offset += line.len();
    }
    found
        .into_iter()
        .map(|(label, start, end)| {
            let text = notices
                .get(start..end)
                .expect("invariant: notice sections start and end on line breaks");
            (label, text.trim())
        })
        .collect()
}

fn crates(text: &str, into: &mut BTreeMap<String, Vec<(String, String)>>) {
    for block in text.split(CARGO_RULE).skip(1) {
        let Some((heading, rest)) = block.split_once('\n') else {
            continue;
        };
        let Some((users, body)) = rest
            .trim_start()
            .strip_prefix("Used by:\n")
            .and_then(|rest| rest.split_once("\n\n"))
        else {
            continue;
        };
        let licence = format!("{}\n\n{}", heading.trim(), body.trim());
        for user in users.lines() {
            let Some(user) = user.strip_prefix("- ") else {
                continue;
            };
            let mut words = user.split_whitespace();
            if let (Some(name), Some(version)) = (words.next(), words.next()) {
                let versions = into.entry(name.to_owned()).or_default();
                if !versions.iter().any(|(seen, _)| seen == version) {
                    versions.push((version.to_owned(), licence.clone()));
                }
            }
        }
    }
}

fn catalogue(notices: &str) -> Vec<Entry> {
    let mut bundled: BTreeMap<String, String> = BTreeMap::new();
    let mut cargo: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (label, text) in sections(notices) {
        match section(label, text) {
            Section::Bundled { name, text } => {
                bundled.entry(name).or_insert_with(|| text.to_owned());
            }
            Section::Crates(text) => crates(text, &mut cargo),
            Section::Attestation => {}
        }
    }
    let mut licences: Vec<Entry> = bundled
        .into_iter()
        .map(|(name, text)| Entry { name, text })
        .collect();
    for (name, versions) in cargo {
        let mut texts: Vec<(Vec<String>, String)> = Vec::new();
        for (version, text) in versions {
            match texts.iter_mut().find(|(_, seen)| *seen == text) {
                Some((shared, _)) => shared.push(version),
                None => texts.push((vec![version], text)),
            }
        }
        let split = texts.len() > 1;
        licences.extend(texts.into_iter().map(|(versions, text)| Entry {
            name: if split {
                format!("{name} {}", versions.join(", "))
            } else {
                name.clone()
            },
            text,
        }));
    }
    licences.sort_by_cached_key(|licence| licence.name.to_lowercase());
    licences
}

fn source(entries: &[Entry]) -> String {
    let mut texts: Vec<&str> = Vec::new();
    let mut rows = String::new();
    for entry in entries {
        let index = texts
            .iter()
            .position(|text| *text == entry.text)
            .unwrap_or_else(|| {
                texts.push(&entry.text);
                texts.len() - 1
            });
        writeln!(
            rows,
            "        ::artisan_payload::licences::Licence {{ name: {:?}, text: TEXT_{index} }},",
            entry.name
        )
        .expect("invariant: writing to a String cannot fail");
    }
    let mut source = String::from("{\n");
    for (index, text) in texts.iter().enumerate() {
        writeln!(source, "    const TEXT_{index}: &str = {text:?};")
            .expect("invariant: writing to a String cannot fail");
    }
    source.push_str("    &[\n");
    source.push_str(&rows);
    source.push_str("    ]\n}\n");
    source
}

fn compile(notices: &str) -> String {
    source(&catalogue(notices))
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedLicencesError {
    #[error("OUT_DIR is not set")]
    OutDirUnset,
    #[error("write the licence table {}", path.display())]
    Write {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

pub fn embed(name: &str, notices: &str) -> Result<(), EmbedLicencesError> {
    let path = std::path::PathBuf::from(
        std::env::var_os("OUT_DIR").ok_or(EmbedLicencesError::OutDirUnset)?,
    )
    .join(name);
    std::fs::write(&path, compile(notices))
        .map_err(|source| EmbedLicencesError::Write { path, source })
}

#[macro_export]
macro_rules! licences {
    ($name:literal) => {
        include!(concat!(env!("OUT_DIR"), "/", $name))
    };
}

#[cfg(test)]
#[path = "../tests/payload/licences.rs"]
mod tests;
