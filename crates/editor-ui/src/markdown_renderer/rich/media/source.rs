//! Where a media `src` points, and how its bytes are fetched.
//!
//! A `src` is a URL (`http`/`https`), a `data:` URL, a `file://` URL, or a
//! file path, where `~` names the home folder. Files live where the agent
//! that named them runs: the Forge's machine, which is often not the
//! Editor's (a Windows Editor with a WSL Forge, or a remote Forge). So when
//! the host installs a [`MediaFetcher`], every file is fetched through it
//! and resolved by the Forge's own rules; relative paths mean the thread's
//! project there. Without one (tests, previews), files resolve locally
//! against [`MediaRoot`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine as _;
use gpui::{App, Global, SharedString};

/// The folder relative media paths resolve against when files are read
/// locally: the open project.
#[derive(Clone, Debug, Default)]
pub struct MediaRoot(pub Option<PathBuf>);

impl Global for MediaRoot {}

impl MediaRoot {
    /// Points relative media paths at `root`, or at the working directory
    /// when `None`.
    pub fn set(cx: &mut App, root: Option<PathBuf>) {
        cx.set_global(Self(root));
    }
}

/// One file a reply names, as the host fetches it from where it lives.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MediaRequest {
    /// The host's scope for relative paths: the open thread.
    pub scope: SharedString,
    /// The path as written, decoded from any `file://` URL; relative to
    /// [`Self::from_home`]'s folder unless absolute where the file lives.
    pub path: String,
    /// Whether `path` is relative to the home folder (`~/…`) rather than
    /// the project.
    pub from_home: bool,
}

/// The bytes of one fetched file, or `None` when it cannot be served.
pub type MediaFetch = Pin<Box<dyn Future<Output = Option<Arc<[u8]>>> + Send>>;

/// The host's way to fetch media files from where they live, scoped to the
/// open thread. Installed while a thread is open; cleared otherwise.
#[derive(Clone)]
pub struct MediaFetcher {
    scope: SharedString,
    fetch: Arc<dyn Fn(MediaRequest) -> MediaFetch + Send + Sync>,
}

impl Global for MediaFetcher {}

impl MediaFetcher {
    /// Fetches files for `scope` through `fetch`.
    pub fn new(
        scope: impl Into<SharedString>,
        fetch: impl Fn(MediaRequest) -> MediaFetch + Send + Sync + 'static,
    ) -> Self {
        Self {
            scope: scope.into(),
            fetch: Arc::new(fetch),
        }
    }

    /// Installs `fetcher`, or removes it so files read locally.
    pub fn set(cx: &mut App, fetcher: Option<Self>) {
        match fetcher {
            Some(fetcher) => cx.set_global(fetcher),
            None => {
                if cx.has_global::<Self>() {
                    cx.remove_global::<Self>();
                }
            }
        }
    }

    /// The scope files are fetched for.
    #[must_use]
    pub fn scope(&self) -> &SharedString {
        &self.scope
    }

    /// Starts fetching `request`.
    #[must_use]
    pub fn fetch(&self, request: MediaRequest) -> MediaFetch {
        (self.fetch)(request)
    }
}

/// One block's load of a source, as GPUI's asset cache keys it.
///
/// The cache keeps every load for the life of the app. Keyed by the source
/// alone, a file rewritten under the same name would show its first bytes
/// in every block that ever names it. Keyed by the block too, each block
/// keeps what it loaded, and a later block naming the file loads it afresh.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Owned<T> {
    /// The loading block's id: unique in the transcript, stable across
    /// frames.
    pub(super) owner: SharedString,
    pub(super) source: T,
}

impl<T> Owned<T> {
    /// `source` as the block `owner` loads it.
    pub(super) fn new(owner: &SharedString, source: T) -> Self {
        Self {
            owner: owner.clone(),
            source,
        }
    }
}

/// A `src` as the reply wrote it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Parsed {
    /// An `http`/`https` URL.
    Url(String),
    /// The decoded payload of a `data:` URL.
    Data(Vec<u8>),
    /// A file path, decoded from any `file://` URL.
    File { path: String, from_home: bool },
    /// Nothing loadable: an empty `src`, a malformed `data:` URL, or a
    /// `file://` URL naming another host.
    Invalid,
}

/// Parses `src` without consulting any file system.
pub(super) fn parse(src: &str) -> Parsed {
    let src = src.trim();
    let lower = src.to_ascii_lowercase();
    if src.is_empty() {
        return Parsed::Invalid;
    }
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Parsed::Url(src.to_owned());
    }
    if lower.starts_with("data:") {
        return data_url(&src["data:".len()..]).map_or(Parsed::Invalid, Parsed::Data);
    }
    if lower.starts_with("file://") {
        return file_url(&src["file://".len()..]);
    }
    home_relative(src)
}

/// `~`, `~/…`, and `~\…` name the home folder; everything else is a path.
fn home_relative(path: &str) -> Parsed {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
    };
    match rest {
        Some(rest) => Parsed::File {
            path: rest.to_owned(),
            from_home: true,
        },
        None => Parsed::File {
            path: path.to_owned(),
            from_home: false,
        },
    }
}

/// `file://[localhost]/path`, percent-decoded. A drive path such as
/// `file:///C:/x` becomes `C:/x`; any other host is not this machine.
fn file_url(rest: &str) -> Parsed {
    let (host, path) = match rest.find('/') {
        Some(slash) => rest.split_at(slash),
        None => (rest, ""),
    };
    if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
        return Parsed::Invalid;
    }
    let Some(path) = percent_decode(path) else {
        return Parsed::Invalid;
    };
    let bytes = path.as_bytes();
    let drive =
        bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':';
    let path = if drive { path[1..].to_owned() } else { path };
    if path.is_empty() {
        return Parsed::Invalid;
    }
    Parsed::File {
        path,
        from_home: false,
    }
}

/// The payload of `data:[<mediatype>][;base64],<data>`.
fn data_url(rest: &str) -> Option<Vec<u8>> {
    let (meta, data) = rest.split_once(',')?;
    if meta.to_ascii_lowercase().ends_with(";base64") {
        let compact: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        base64::engine::general_purpose::STANDARD
            .decode(compact.as_bytes())
            .ok()
    } else {
        percent_decode(data).map(String::into_bytes)
    }
}

/// Decodes `%xx` escapes; `None` for a malformed escape or non-UTF-8 result.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Where a `src`'s bytes come from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Located {
    Url(String),
    Data(Vec<u8>),
    /// A file read on this machine.
    Path(PathBuf),
    /// A file fetched from where the open thread's files live.
    Fetched(MediaRequest),
    Invalid,
}

/// Locates `src`: through the host's fetcher when one is installed, else on
/// this machine against [`MediaRoot`] and the home folder.
pub(super) fn locate(src: &str, cx: &App) -> Located {
    let parsed = parse(src);
    if let (Parsed::File { path, from_home }, Some(fetcher)) =
        (&parsed, cx.try_global::<MediaFetcher>())
    {
        return Located::Fetched(MediaRequest {
            scope: fetcher.scope().clone(),
            path: path.clone(),
            from_home: *from_home,
        });
    }
    let root = cx
        .try_global::<MediaRoot>()
        .and_then(|root| root.0.as_deref());
    resolve(parsed, root, home_dir().as_deref())
}

/// This machine's home folder: `HOME`, or `USERPROFILE` on Windows.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Resolves a parsed `src` on this machine against `root` and `home`.
pub(super) fn resolve(parsed: Parsed, root: Option<&Path>, home: Option<&Path>) -> Located {
    match parsed {
        Parsed::Url(url) => Located::Url(url),
        Parsed::Data(bytes) => Located::Data(bytes),
        Parsed::Invalid => Located::Invalid,
        Parsed::File {
            path,
            from_home: true,
        } => home.map_or(Located::Invalid, |home| Located::Path(home.join(path))),
        Parsed::File { path, .. } => {
            let path = PathBuf::from(path);
            match root {
                Some(root) if path.is_relative() => Located::Path(root.join(path)),
                _ => Located::Path(path),
            }
        }
    }
}
