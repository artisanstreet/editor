mod cmd;

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use globset::GlobMatcher;
use regex::Regex;
use serde_json::Value;

use super::files::{self, ReadFileError};
use super::tools::Call;

const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
const MAX_SHELL_DEPTH: usize = 6;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allow,
    Ask,
    Deny(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Grammar {
    #[cfg_attr(not(windows), default)]
    Posix,
    #[cfg_attr(windows, default)]
    Cmd,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadSettingsError {
    #[error("could not read the Claude Code settings file {}", file.display())]
    Unreadable {
        file: PathBuf,
        #[source]
        source: ReadFileError,
    },
    #[error("the Claude Code settings file {} is not valid JSON", file.display())]
    NotJson {
        file: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("the Claude Code settings file {} has a malformed `{field}`", file.display())]
    Malformed { file: PathBuf, field: &'static str },
}

#[derive(Debug, thiserror::Error)]
pub enum ReadRefusal {
    #[error("the Claude Code permission rule {rule} withholds it")]
    Rule { rule: String },
    #[error("a Claude Code settings file failed to load, so no file is read on the agent's behalf")]
    Unsettled(#[source] Arc<LoadSettingsError>),
}

#[derive(Debug, Clone, Default)]
pub struct Rules {
    grammar: Grammar,
    broken: Vec<Arc<LoadSettingsError>>,
    allow: Vec<Rule>,
    ask: Vec<Rule>,
    deny: Vec<Rule>,
    accept_edits: bool,
    home: PathBuf,
    cwd: PathBuf,
    root: PathBuf,
}

#[derive(Debug, Clone)]
struct Rule {
    tool: String,
    spec: Spec,
    written: String,
}

#[derive(Debug, Clone)]
enum Spec {
    Any,
    Exact(Vec<String>),
    Prefix(Vec<String>),
    Pattern(Regex),
    Path {
        bases: Vec<PathBuf>,
        glob: GlobMatcher,
    },
    Domain {
        domain: String,
        subdomains_only: bool,
    },
    Never,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum List {
    Allow,
    Ask,
    Deny,
}

struct Anchors<'a> {
    user_home: &'a Path,
    root: &'a Path,
    cwd: &'a Path,
}

impl Rules {
    #[must_use]
    pub fn load(home: &Path, cwd: &Path) -> Self {
        Self::load_for(home, cwd, Grammar::default())
    }

    #[must_use]
    pub(crate) fn load_for(home: &Path, cwd: &Path, grammar: Grammar) -> Self {
        let mut project: Vec<_> = cwd.ancestors().take_while(|dir| *dir != home).collect();
        if let Some(root) = project.iter().position(|dir| dir.join(".git").exists()) {
            project.truncate(root + 1);
        } else {
            project.truncate(1);
        }
        let root = project
            .last()
            .map_or_else(|| cwd.to_path_buf(), |dir| dir.to_path_buf());
        let mut rules = Self {
            grammar,
            home: home.to_path_buf(),
            cwd: cwd.to_path_buf(),
            root: root.clone(),
            ..Self::default()
        };
        let user_home = std::env::home_dir().unwrap_or_else(|| home.to_path_buf());
        let mut files = vec![(home.join(".claude").join("settings.json"), root)];
        for dir in project.into_iter().rev() {
            files.push((dir.join(".claude").join("settings.json"), dir.to_path_buf()));
            files.push((
                dir.join(".claude").join("settings.local.json"),
                dir.to_path_buf(),
            ));
        }
        for (file, root) in files {
            let anchors = Anchors {
                user_home: &user_home,
                root: &root,
                cwd,
            };
            if let Err(error) = rules.read_file(&file, &anchors) {
                let logged: &dyn std::error::Error = &error;
                tracing::warn!(
                    error = logged,
                    "a Claude Code settings file failed to load; tools are refused until it is repaired"
                );
                rules.broken.push(Arc::new(error));
            }
        }
        rules
    }

    fn read_file(&mut self, path: &Path, anchors: &Anchors<'_>) -> Result<(), LoadSettingsError> {
        let bytes = match files::read_bounded(path, MAX_SETTINGS_BYTES) {
            Ok(bytes) => bytes,
            Err(ReadFileError::Open { source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(source) => {
                return Err(LoadSettingsError::Unreadable {
                    file: path.to_path_buf(),
                    source,
                });
            }
        };
        let value = serde_json::from_slice::<Value>(&bytes).map_err(|source| {
            LoadSettingsError::NotJson {
                file: path.to_path_buf(),
                source,
            }
        })?;
        let malformed = |field| LoadSettingsError::Malformed {
            file: path.to_path_buf(),
            field,
        };
        let settings = value.as_object().ok_or_else(|| malformed("settings"))?;
        let Some(permissions) = settings.get("permissions") else {
            return Ok(());
        };
        let permissions = permissions
            .as_object()
            .ok_or_else(|| malformed("permissions"))?;
        let mode = match permissions.get("defaultMode") {
            None => None,
            Some(mode) => Some(
                mode.as_str()
                    .ok_or_else(|| malformed("permissions.defaultMode"))?,
            ),
        };
        let allow =
            rule_texts(permissions.get("allow")).ok_or_else(|| malformed("permissions.allow"))?;
        let ask = rule_texts(permissions.get("ask")).ok_or_else(|| malformed("permissions.ask"))?;
        let deny =
            rule_texts(permissions.get("deny")).ok_or_else(|| malformed("permissions.deny"))?;
        if mode == Some("acceptEdits") {
            self.accept_edits = true;
        }
        let grammar = self.grammar;
        push(&mut self.allow, &allow, List::Allow, anchors, path, grammar);
        push(&mut self.ask, &ask, List::Ask, anchors, path, grammar);
        push(&mut self.deny, &deny, List::Deny, anchors, path, grammar);
        Ok(())
    }

    #[must_use]
    pub(crate) fn broken(&self) -> Option<&LoadSettingsError> {
        self.broken.first().map(AsRef::as_ref)
    }

    #[must_use]
    pub(crate) const fn accept_edits(&self) -> bool {
        self.accept_edits
    }

    #[must_use]
    pub(crate) fn verdict(&self, call: &Call) -> Option<Verdict> {
        let facts = self.facts(call);
        if let Some(rule) = self.deny.iter().find(|rule| rule.touches(&facts)) {
            return Some(Verdict::Deny(format!(
                "A Claude Code permission rule denies this: {}. Do not retry it; ask the user or take another way.",
                rule.written
            )));
        }
        if self.ask.iter().any(|rule| rule.touches(&facts)) {
            return Some(Verdict::Ask);
        }
        if self.allows(&facts) {
            return Some(Verdict::Allow);
        }
        None
    }

    fn allows(&self, facts: &Facts<'_>) -> bool {
        let named: Vec<&Rule> = self
            .allow
            .iter()
            .filter(|rule| rule.names(facts.tool))
            .collect();
        if named.iter().any(|rule| matches!(rule.spec, Spec::Any)) {
            return true;
        }
        if let Some(shell) = &facts.shell {
            return shell.complete
                && !shell.commands.is_empty()
                && shell
                    .commands
                    .iter()
                    .all(|command| named.iter().any(|rule| rule.fits(command)));
        }
        named.iter().any(|rule| rule.vouches(facts))
    }

    #[must_use]
    pub(crate) fn protects(&self, call: &Call) -> bool {
        changes_files(&call.name)
            && call_path(call)
                .is_some_and(|raw| self.candidates(raw).iter().any(|path| self.protected(path)))
    }

    fn protected(&self, path: &Path) -> bool {
        let names: Vec<String> = path
            .components()
            .filter_map(|component| match component {
                Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
                Component::Prefix(_)
                | Component::RootDir
                | Component::CurDir
                | Component::ParentDir => None,
            })
            .collect();
        names.iter().any(|name| name == ".claude")
            || names.last().is_some_and(|name| name == ".mcp.json")
            || (!self.home.as_os_str().is_empty()
                && forms(&self.home).iter().any(|home| path.starts_with(home)))
    }

    #[must_use]
    pub(crate) fn inside_root(&self, call: &Call) -> bool {
        if self.root.as_os_str().is_empty() {
            return false;
        }
        let roots = forms(&self.root);
        call_path(call).is_some_and(|raw| {
            self.candidates(raw)
                .iter()
                .all(|path| roots.iter().any(|root| path.starts_with(root)))
        })
    }

    #[must_use]
    pub(crate) fn hides(&self, tool: &str, path: &Path) -> bool {
        let mut rules = self
            .deny
            .iter()
            .chain(&self.ask)
            .filter(|rule| matches!(rule.spec, Spec::Path { .. }) && rule.names(tool))
            .peekable();
        if rules.peek().is_none() {
            return false;
        }
        let facts = Facts {
            tool,
            shell: None,
            paths: forms(path),
            host: None,
        };
        rules.any(|rule| rule.touches(&facts))
    }

    pub fn may_read(&self, path: &Path) -> Result<(), ReadRefusal> {
        if let Some(broken) = self.broken.first() {
            return Err(ReadRefusal::Unsettled(Arc::clone(broken)));
        }
        let facts = Facts {
            tool: "Read",
            shell: None,
            paths: forms(path),
            host: None,
        };
        match self
            .deny
            .iter()
            .chain(&self.ask)
            .find(|rule| rule.touches(&facts))
        {
            Some(rule) => Err(ReadRefusal::Rule {
                rule: rule.written.clone(),
            }),
            None => Ok(()),
        }
    }

    fn facts<'a>(&self, call: &'a Call) -> Facts<'a> {
        let tool = call.name.as_str();
        Facts {
            tool,
            shell: (tool == "Bash").then(|| {
                Shell::parse(
                    call.input["command"].as_str().unwrap_or_default(),
                    self.grammar,
                )
            }),
            paths: call_path(call)
                .map(|raw| self.candidates(raw))
                .unwrap_or_default(),
            host: (tool == "WebFetch")
                .then(|| url_host(call.input["url"].as_str().unwrap_or_default()))
                .flatten(),
        }
    }

    fn candidates(&self, raw: &str) -> Vec<PathBuf> {
        let path = Path::new(raw);
        if path.is_absolute() {
            forms(path)
        } else {
            forms(&self.cwd.join(path))
        }
    }
}

fn rule_texts(list: Option<&Value>) -> Option<Vec<&str>> {
    match list {
        None => Some(Vec::new()),
        Some(list) => list.as_array()?.iter().map(Value::as_str).collect(),
    }
}

fn push(
    rules: &mut Vec<Rule>,
    list: &[&str],
    kind: List,
    anchors: &Anchors<'_>,
    file: &Path,
    grammar: Grammar,
) {
    rules.extend(
        list.iter()
            .filter_map(|rule| parse(rule, kind, anchors, file, grammar)),
    );
}

#[must_use]
pub(super) fn url_host(url: &str) -> Option<String> {
    let url = reqwest::Url::parse(url.trim()).ok()?;
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

#[must_use]
pub(super) fn changes_files(tool: &str) -> bool {
    matches!(tool, "Edit" | "Write" | "MultiEdit" | "NotebookEdit")
}

fn call_path(call: &Call) -> Option<&str> {
    let input = &call.input;
    let text = |key: &str| input[key].as_str().filter(|path| !path.trim().is_empty());
    match call.name.as_str() {
        "Read" | "Write" | "Edit" | "MultiEdit" => text("file_path").or_else(|| text("path")),
        "NotebookEdit" => text("notebook_path"),
        "Grep" | "Glob" => Some(text("path").unwrap_or(".")),
        _ => None,
    }
}

fn forms(path: &Path) -> Vec<PathBuf> {
    let lexical = normalize(path);
    let real = path
        .ancestors()
        .find_map(|ancestor| {
            let real = std::fs::canonicalize(ancestor).ok()?;
            let rest = path.strip_prefix(ancestor).ok()?;
            Some(normalize(&real.join(rest)))
        })
        .unwrap_or_else(|| lexical.clone());
    if real == lexical {
        vec![lexical]
    } else {
        vec![lexical, real]
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            other @ (Component::Prefix(_) | Component::RootDir | Component::Normal(_)) => {
                out.push(other);
            }
        }
    }
    out
}

struct Facts<'a> {
    tool: &'a str,
    shell: Option<Shell>,
    paths: Vec<PathBuf>,
    host: Option<String>,
}

impl Rule {
    fn names(&self, tool: &str) -> bool {
        if self.tool.starts_with("mcp__") {
            let rule = self.tool.strip_suffix("__*").unwrap_or(&self.tool);
            return tool == rule
                || tool
                    .strip_prefix(rule)
                    .is_some_and(|rest| rest.starts_with("__"));
        }
        match self.tool.as_str() {
            "Read" => matches!(tool, "Read" | "Grep" | "Glob"),
            "Edit" => changes_files(tool),
            rule => tool == rule,
        }
    }

    fn touches(&self, facts: &Facts<'_>) -> bool {
        if !self.names(facts.tool) {
            return false;
        }
        match &self.spec {
            Spec::Any => true,
            Spec::Never => false,
            Spec::Exact(_) | Spec::Prefix(_) | Spec::Pattern(_) => {
                facts.shell.as_ref().is_some_and(|shell| {
                    shell
                        .commands
                        .iter()
                        .chain(&shell.aliases)
                        .any(|command| program_forms(command).iter().any(|form| self.fits(form)))
                })
            }
            Spec::Path { .. } => facts.paths.iter().any(|path| self.covers_path(path)),
            Spec::Domain { .. } => facts
                .host
                .as_deref()
                .is_some_and(|host| self.covers_host(host)),
        }
    }

    fn vouches(&self, facts: &Facts<'_>) -> bool {
        match &self.spec {
            Spec::Any => true,
            Spec::Path { .. } => {
                !facts.paths.is_empty() && facts.paths.iter().all(|path| self.covers_path(path))
            }
            Spec::Domain { .. } => facts
                .host
                .as_deref()
                .is_some_and(|host| self.covers_host(host)),
            Spec::Exact(_) | Spec::Prefix(_) | Spec::Pattern(_) | Spec::Never => false,
        }
    }

    fn fits(&self, words: &[String]) -> bool {
        match &self.spec {
            Spec::Any => true,
            Spec::Exact(rule) => words == rule.as_slice(),
            Spec::Prefix(rule) => words.starts_with(rule),
            Spec::Pattern(pattern) => pattern.is_match(&words.join(" ")),
            Spec::Path { .. } | Spec::Domain { .. } | Spec::Never => false,
        }
    }

    fn covers_path(&self, path: &Path) -> bool {
        let Spec::Path { bases, glob } = &self.spec else {
            return false;
        };
        bases.iter().any(|base| {
            path.strip_prefix(base).is_ok_and(|rest| {
                let parts: Vec<String> = rest
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                (1..=parts.len()).any(|end| glob.is_match(parts[..end].join("/")))
            })
        })
    }

    fn covers_host(&self, host: &str) -> bool {
        let Spec::Domain {
            domain,
            subdomains_only,
        } = &self.spec
        else {
            return false;
        };
        let below = host
            .strip_suffix(domain.as_str())
            .is_some_and(|rest| rest.ends_with('.'));
        below || (!subdomains_only && host == domain)
    }
}

fn parse(
    text: &str,
    kind: List,
    anchors: &Anchors<'_>,
    file: &Path,
    grammar: Grammar,
) -> Option<Rule> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let (tool, spec) = match text.split_once('(') {
        Some((tool, rest)) => (tool.trim(), Some(rest.strip_suffix(')').map(str::trim))),
        None => (text, None),
    };
    if tool.is_empty() || tool.len() > 128 {
        return None;
    }
    let spec = match (tool, spec) {
        (_, None) => Some(Spec::Any),
        ("Bash", Some(Some(spec))) => bash_spec(spec, grammar),
        (
            "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" | "Glob" | "Grep",
            Some(Some(spec)),
        ) => path_spec(spec, anchors),
        ("WebFetch", Some(Some(spec))) => domain_spec(spec),
        _ => None,
    };
    let spec = match (spec, kind) {
        (Some(spec), _) => spec,
        (None, List::Allow) => Spec::Never,
        (None, List::Ask | List::Deny) => {
            tracing::warn!(
                rule = text,
                file = %file.display(),
                "a Claude Code permission rule this transport cannot read covers the whole tool"
            );
            Spec::Any
        }
    };
    Some(Rule {
        tool: tool.to_owned(),
        spec,
        written: text.to_owned(),
    })
}

fn bash_spec(spec: &str, grammar: Grammar) -> Option<Spec> {
    if let Some(prefix) = spec.strip_suffix(":*") {
        return one_command(prefix, grammar).map(Spec::Prefix);
    }
    if let Some(prefix) = spec
        .strip_suffix(" *")
        .filter(|prefix| !prefix.contains('*'))
    {
        return one_command(prefix, grammar).map(Spec::Prefix);
    }
    if spec.contains('*') {
        let spec = spec.split_whitespace().collect::<Vec<_>>().join(" ");
        let pattern = spec
            .split('*')
            .map(regex::escape)
            .collect::<Vec<_>>()
            .join(".*");
        return Regex::new(&format!("^{pattern}$")).ok().map(Spec::Pattern);
    }
    one_command(spec, grammar).map(Spec::Exact)
}

fn one_command(text: &str, grammar: Grammar) -> Option<Vec<String>> {
    let shell = Shell::parse(text, grammar);
    match shell.commands.as_slice() {
        [command] if shell.complete => Some(command.clone()),
        _ => None,
    }
}

fn path_spec(spec: &str, anchors: &Anchors<'_>) -> Option<Spec> {
    let (base, pattern, anchored) = spec.strip_prefix("//").map_or_else(
        || {
            if spec == "~" {
                (anchors.user_home.to_path_buf(), "**", true)
            } else if let Some(rest) = spec.strip_prefix("~/") {
                (anchors.user_home.to_path_buf(), rest, true)
            } else if let Some(rest) = spec.strip_prefix('/') {
                (anchors.root.to_path_buf(), rest, true)
            } else if let Some(rest) = spec.strip_prefix("./") {
                (anchors.cwd.to_path_buf(), rest, true)
            } else {
                let anchored = spec.trim_end_matches('/').contains('/');
                (anchors.cwd.to_path_buf(), spec, anchored)
            }
        },
        absolute,
    );
    let pattern = pattern.trim_end_matches('/');
    let pattern = match (pattern.is_empty(), anchored) {
        (true, _) => "**".to_owned(),
        (false, true) => pattern.to_owned(),
        (false, false) => format!("**/{pattern}"),
    };
    let glob = globset::GlobBuilder::new(&pattern)
        .literal_separator(true)
        .case_insensitive(cfg!(windows))
        .build()
        .ok()?
        .compile_matcher();
    Some(Spec::Path {
        bases: forms(&base),
        glob,
    })
}

fn absolute(rest: &str) -> (PathBuf, &str, bool) {
    if cfg!(windows) {
        let mut chars = rest.chars();
        if let (Some(drive), Some('/')) = (chars.next(), chars.next())
            && drive.is_ascii_alphabetic()
        {
            return (
                PathBuf::from(format!("{drive}:\\")),
                rest.split_at(2).1,
                true,
            );
        }
    }
    (PathBuf::from("/"), rest, true)
}

fn domain_spec(spec: &str) -> Option<Spec> {
    let domain = spec
        .strip_prefix("domain:")?
        .trim()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let (domain, subdomains_only) = domain.strip_prefix("*.").map_or_else(
        || (domain.trim_start_matches('.').to_owned(), false),
        |rest| (rest.to_owned(), true),
    );
    if domain.is_empty() || domain.contains(['*', '/', ':', ' ', '@']) {
        return None;
    }
    Some(Spec::Domain {
        domain,
        subdomains_only,
    })
}

const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "command", "builtin", "exec", "nohup", "time", "nice", "ionice",
    "timeout", "stdbuf", "xargs", "setsid", "unbuffer",
];
const VALUE_OPTIONS: &[&str] = &[
    "-u", "-g", "-n", "-c", "-p", "-C", "-I", "-P", "-L", "-d", "-E", "-s", "-k", "-a", "-i", "-o",
    "-e",
];
const SHELLS: &[&str] = &["sh", "bash", "zsh", "dash", "ksh", "ash", "fish"];
const KEYWORDS: &[&str] = &[
    "!", "{", "if", "then", "else", "elif", "do", "while", "until",
];
const CLOSERS: &[&str] = &["}", "fi", "done", "esac"];

fn program_forms(words: &[String]) -> Vec<Vec<String>> {
    let mut forms = vec![words.to_vec()];
    let mut current = words.to_vec();
    while let Some(program) = current.first() {
        let bare = program_name(program);
        if bare != *program {
            current[0] = bare;
            forms.push(current.clone());
        }
        match unwrapped(&current) {
            Some(inner) if !inner.is_empty() && inner.len() < current.len() => {
                forms.push(inner.clone());
                current = inner;
            }
            _ => break,
        }
    }
    forms
}

fn program_name(program: &str) -> String {
    let name = program.rsplit(['/', '\\']).next().unwrap_or(program);
    let split = name
        .len()
        .checked_sub(4)
        .filter(|cut| *cut > 0)
        .and_then(|cut| Some((name.get(..cut)?, name.get(cut..)?)));
    match split {
        Some((stem, extension)) if extension.eq_ignore_ascii_case(".exe") => stem.to_owned(),
        Some(_) | None => name.to_owned(),
    }
}

fn unwrapped(words: &[String]) -> Option<Vec<String>> {
    let program = words.first()?.as_str();
    if !WRAPPERS.contains(&program) {
        return None;
    }
    let mut rest = &words[1..];
    while let Some(word) = rest.first() {
        if word == "--" {
            rest = &rest[1..];
            break;
        }
        if word.starts_with('-') && word.len() > 1 {
            let takes = VALUE_OPTIONS.contains(&word.as_str()) && rest.len() > 1;
            rest = &rest[if takes { 2 } else { 1 }..];
        } else if program == "env" && is_assignment(word) {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    if program == "timeout"
        && rest
            .first()
            .is_some_and(|word| word.starts_with(|c: char| c.is_ascii_digit()))
    {
        rest = &rest[1..];
    }
    Some(rest.to_vec())
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[derive(Debug, Default)]
struct Shell {
    commands: Vec<Vec<String>>,
    aliases: Vec<Vec<String>>,
    complete: bool,
}

impl Shell {
    fn parse(text: &str, grammar: Grammar) -> Self {
        let mut shell = Self {
            commands: Vec::new(),
            aliases: Vec::new(),
            complete: true,
        };
        match grammar {
            Grammar::Posix => parse_into(text, 0, &mut shell),
            Grammar::Cmd => cmd::parse(text, 0, &mut shell),
        }
        shell
    }
}

fn parse_into(text: &str, depth: usize, out: &mut Shell) {
    if depth > MAX_SHELL_DEPTH {
        out.complete = false;
        return;
    }
    let mut lexer = Lexer {
        chars: text.chars().collect(),
        at: 0,
        depth,
        out,
        words: Vec::new(),
        word: String::new(),
        started: false,
        dynamic: false,
        target: false,
        heredocs: Vec::new(),
        heredocs_from: 0,
    };
    lexer.run();
}

struct Word {
    text: String,
    dynamic: bool,
}

struct Heredoc {
    delimiter: String,
    strip_tabs: bool,
    quoted: bool,
    shell: bool,
}

struct Lexer<'a> {
    chars: Vec<char>,
    at: usize,
    depth: usize,
    out: &'a mut Shell,
    words: Vec<Word>,
    word: String,
    started: bool,
    dynamic: bool,
    target: bool,
    heredocs: Vec<Heredoc>,
    heredocs_from: usize,
}

impl Lexer<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.at + ahead).copied()
    }

    fn push(&mut self, c: char) {
        self.word.push(c);
        self.started = true;
    }

    fn run(&mut self) {
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => {
                    self.at += 1;
                    match self.peek(0) {
                        Some('\n') => self.at += 1,
                        Some(next) => {
                            self.push(next);
                            self.at += 1;
                        }
                        None => {}
                    }
                }
                '\'' => self.single_quoted(),
                '"' => self.double_quoted(),
                '$' => self.dollar(false),
                '`' => self.backtick(),
                '#' if !self.started => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.at += 1;
                    }
                }
                ' ' | '\t' | '\r' => {
                    self.end_word();
                    self.at += 1;
                }
                '\n' => {
                    self.end_command();
                    self.at += 1;
                    self.heredoc_bodies();
                }
                ';' | '(' | ')' => {
                    self.end_command();
                    self.at += 1;
                }
                '|' => {
                    self.end_command();
                    self.at += 1;
                    if matches!(self.peek(0), Some('|' | '&')) {
                        self.at += 1;
                    }
                }
                '&' => match self.peek(1) {
                    Some('>') => self.redirect(),
                    Some('&') => {
                        self.end_command();
                        self.at += 2;
                    }
                    _ => {
                        self.end_command();
                        self.at += 1;
                    }
                },
                '<' | '>' if self.peek(1) == Some('(') => {
                    self.substitution(self.at + 1, "<(…)");
                }
                '<' | '>' => self.redirect(),
                _ => {
                    self.push(c);
                    self.at += 1;
                }
            }
        }
        self.end_command();
        if !self.heredocs.is_empty() {
            self.heredoc_bodies();
        }
    }

    fn single_quoted(&mut self) {
        self.at += 1;
        self.started = true;
        loop {
            match self.peek(0) {
                None => {
                    self.out.complete = false;
                    return;
                }
                Some('\'') => {
                    self.at += 1;
                    return;
                }
                Some(c) => {
                    self.push(c);
                    self.at += 1;
                }
            }
        }
    }

    fn double_quoted(&mut self) {
        self.at += 1;
        self.started = true;
        loop {
            match self.peek(0) {
                None => {
                    self.out.complete = false;
                    return;
                }
                Some('"') => {
                    self.at += 1;
                    return;
                }
                Some('\\') => match self.peek(1) {
                    Some(next @ ('"' | '\\' | '$' | '`')) => {
                        self.push(next);
                        self.at += 2;
                    }
                    Some('\n') => self.at += 2,
                    _ => {
                        self.push('\\');
                        self.at += 1;
                    }
                },
                Some('$') => self.dollar(true),
                Some('`') => self.backtick(),
                Some(c) => {
                    self.push(c);
                    self.at += 1;
                }
            }
        }
    }

    fn dollar(&mut self, quoted: bool) {
        match self.peek(1) {
            Some('(') if self.peek(2) == Some('(') => {
                let Some(end) = self.closing(self.at + 1) else {
                    self.unreadable();
                    return;
                };
                self.opaque(end);
            }
            Some('(') => self.substitution(self.at + 1, "$(…)"),
            Some('{') => {
                let Some(end) = self.chars[self.at + 2..]
                    .iter()
                    .position(|c| *c == '}')
                    .map(|end| self.at + 2 + end)
                else {
                    self.unreadable();
                    return;
                };
                self.opaque(end);
            }
            Some('\'') if !quoted => self.ansi_quoted(),
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                self.push('$');
                self.at += 1;
                while self
                    .peek(0)
                    .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    let c = self.chars[self.at];
                    self.push(c);
                    self.at += 1;
                }
                self.dynamic = true;
            }
            Some(c) if c.is_ascii_digit() || "@*#?$!-".contains(c) => {
                self.push('$');
                self.push(c);
                self.at += 2;
                self.dynamic = true;
            }
            _ => {
                self.push('$');
                self.at += 1;
            }
        }
    }

    fn opaque(&mut self, end: usize) {
        let text: String = self.chars[self.at..=end].iter().collect();
        let (_, after_first) = text.split_at(text.chars().next().map_or(0, char::len_utf8));
        if after_first.contains("$(") || text.contains('`') || after_first.contains("<(") {
            self.out.complete = false;
        }
        self.word.push_str(&text);
        self.started = true;
        self.dynamic = true;
        self.at = end + 1;
    }

    fn ansi_quoted(&mut self) {
        self.at += 2;
        self.started = true;
        loop {
            match self.peek(0) {
                None => {
                    self.out.complete = false;
                    return;
                }
                Some('\'') => {
                    self.at += 1;
                    return;
                }
                Some('\\') => {
                    let plain = match self.peek(1) {
                        Some('n') => Some('\n'),
                        Some('t') => Some('\t'),
                        Some(c @ ('\\' | '\'' | '"')) => Some(c),
                        _ => None,
                    };
                    if let Some(c) = plain {
                        self.push(c);
                    } else {
                        self.push('\\');
                        self.dynamic = true;
                        if let Some(c) = self.peek(1) {
                            self.push(c);
                        }
                    }
                    self.at += 2;
                }
                Some(c) => {
                    self.push(c);
                    self.at += 1;
                }
            }
        }
    }

    fn substitution(&mut self, open: usize, shown: &str) {
        let Some(end) = self.closing(open) else {
            self.unreadable();
            return;
        };
        let inner: String = self.chars[open + 1..end].iter().collect();
        parse_into(&inner, self.depth + 1, self.out);
        self.word.push_str(shown);
        self.started = true;
        self.dynamic = true;
        self.at = end + 1;
    }

    fn backtick(&mut self) {
        let mut inner = String::new();
        let mut at = self.at + 1;
        loop {
            match self.chars.get(at) {
                None => {
                    self.unreadable();
                    return;
                }
                Some('`') => break,
                Some('\\') if matches!(self.chars.get(at + 1), Some('`' | '\\' | '$')) => {
                    inner.push(self.chars[at + 1]);
                    at += 2;
                }
                Some(c) => {
                    inner.push(*c);
                    at += 1;
                }
            }
        }
        parse_into(&inner, self.depth + 1, self.out);
        self.word.push_str("`…`");
        self.started = true;
        self.dynamic = true;
        self.at = at + 1;
    }

    fn closing(&self, open: usize) -> Option<usize> {
        let mut depth = 0usize;
        let mut quote: Option<char> = None;
        let mut at = open;
        while let Some(&c) = self.chars.get(at) {
            match quote {
                Some('\'') => {
                    if c == '\'' {
                        quote = None;
                    }
                }
                Some(_) => match c {
                    '\\' => at += 1,
                    '"' => quote = None,
                    _ => {}
                },
                None => match c {
                    '\\' => at += 1,
                    '\'' | '"' => quote = Some(c),
                    '(' => depth += 1,
                    ')' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            return Some(at);
                        }
                    }
                    _ => {}
                },
            }
            at += 1;
        }
        None
    }

    fn unreadable(&mut self) {
        self.out.complete = false;
        self.at = self.chars.len();
    }

    fn redirect(&mut self) {
        if self.started
            && !self.dynamic
            && !self.word.is_empty()
            && self.word.chars().all(|c| c.is_ascii_digit())
        {
            self.word.clear();
            self.started = false;
        } else {
            self.end_word();
        }
        let ahead: String = self.chars[self.at..self.chars.len().min(self.at + 3)]
            .iter()
            .collect();
        let op = [
            "<<<", "<<-", "&>>", "<<", ">>", ">&", "<&", ">|", "<>", "&>", "<", ">",
        ]
        .into_iter()
        .find(|op| ahead.starts_with(op))
        .unwrap_or(">");
        self.at += op.len();
        match op {
            "<<" | "<<-" => self.heredoc(op == "<<-"),
            _ => self.target = true,
        }
    }

    fn heredoc(&mut self, strip_tabs: bool) {
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.at += 1;
        }
        let mut delimiter = String::new();
        let mut quoted = false;
        while let Some(c) = self.peek(0) {
            if c.is_whitespace() || ";|&<>()".contains(c) {
                break;
            }
            if matches!(c, '\'' | '"' | '\\') {
                quoted = true;
            } else {
                delimiter.push(c);
            }
            self.at += 1;
        }
        if delimiter.is_empty() {
            self.out.complete = false;
            return;
        }
        self.heredocs.push(Heredoc {
            delimiter,
            strip_tabs,
            quoted,
            shell: false,
        });
    }

    fn heredoc_bodies(&mut self) {
        for heredoc in std::mem::take(&mut self.heredocs) {
            let mut body = String::new();
            while self.at < self.chars.len() {
                let end = self.chars[self.at..]
                    .iter()
                    .position(|c| *c == '\n')
                    .map_or(self.chars.len(), |end| self.at + end);
                let line: String = self.chars[self.at..end].iter().collect();
                self.at = (end + 1).min(self.chars.len());
                let bare = if heredoc.strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    &line
                };
                if bare == heredoc.delimiter {
                    break;
                }
                body.push_str(&line);
                body.push('\n');
            }
            if heredoc.shell {
                parse_into(&body, self.depth + 1, self.out);
            } else if !heredoc.quoted && (body.contains("$(") || body.contains('`')) {
                self.out.complete = false;
            }
        }
        self.heredocs_from = 0;
    }

    fn end_word(&mut self) {
        if self.started {
            let word = Word {
                text: std::mem::take(&mut self.word),
                dynamic: self.dynamic,
            };
            if self.target {
                self.target = false;
            } else {
                self.words.push(word);
            }
        }
        self.word.clear();
        self.started = false;
        self.dynamic = false;
    }

    fn end_command(&mut self) {
        self.end_word();
        if self.target {
            self.target = false;
            self.out.complete = false;
        }
        let words = std::mem::take(&mut self.words);
        let shell = simple(&words, self.depth, self.out);
        for heredoc in &mut self.heredocs[self.heredocs_from..] {
            heredoc.shell = shell;
        }
        self.heredocs_from = self.heredocs.len();
    }
}

fn simple(words: &[Word], depth: usize, out: &mut Shell) -> bool {
    let Some(start) = words
        .iter()
        .position(|word| !(is_assignment(&word.text) || KEYWORDS.contains(&word.text.as_str())))
    else {
        return false;
    };
    let words = &words[start..];
    let first = words[0].text.as_str();
    if words.len() == 1 && CLOSERS.contains(&first) {
        return false;
    }
    if matches!(first, "for" | "case" | "select" | "function" | "coproc") {
        out.complete = false;
        return false;
    }
    if words[0].dynamic {
        out.complete = false;
    }
    let texts: Vec<String> = words.iter().map(|word| word.text.clone()).collect();
    let shell = nested(&texts, depth, out);
    out.commands.push(texts);
    shell
}

fn nested(words: &[String], depth: usize, out: &mut Shell) -> bool {
    for form in program_forms(words) {
        let Some(program) = form.first() else {
            continue;
        };
        let program = program_name(program);
        if SHELLS.contains(&program.as_str()) {
            if let Some(script) = shell_script(&form) {
                parse_into(script, depth + 1, out);
            }
            return true;
        }
        if program == "eval" {
            parse_into(&form[1..].join(" "), depth + 1, out);
            return false;
        }
        if program == "find" {
            let mut rest = &form[1..];
            while let Some(at) = rest
                .iter()
                .position(|word| matches!(word.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
            {
                let after = &rest[at + 1..];
                let end = after
                    .iter()
                    .position(|word| word == ";" || word == "+")
                    .unwrap_or(after.len());
                let command: Vec<Word> = after[..end]
                    .iter()
                    .map(|word| Word {
                        text: word.clone(),
                        dynamic: false,
                    })
                    .collect();
                simple(&command, depth + 1, out);
                rest = &after[end.min(after.len())..];
                if rest.is_empty() {
                    break;
                }
                rest = &rest[1..];
            }
            return false;
        }
    }
    false
}

fn shell_script(form: &[String]) -> Option<&str> {
    let flag = form.iter().skip(1).position(|word| {
        word.strip_prefix('-').is_some_and(|flags| {
            !flags.is_empty()
                && !flags.starts_with('-')
                && flags.chars().all(|c| c.is_ascii_alphabetic())
                && flags.contains('c')
        })
    })? + 1;
    form[flag + 1..]
        .iter()
        .find(|word| !word.starts_with('-'))
        .map(String::as_str)
}

#[cfg(test)]
#[path = "../../tests/claudeai/rules/unit.rs"]
mod tests;
