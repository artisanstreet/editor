use std::num::NonZeroUsize;
use std::ops::Range;
use std::sync::Arc;

const WINDOW_BEFORE_TARGET: usize = 2_500;
const WINDOW_LINES: usize = 5_000;

#[derive(Clone, Debug)]
pub struct PreviewText {
    full: Arc<str>,
    window: Option<LineWindow>,
}

#[derive(Clone, Debug)]
pub struct LineWindow {
    text: Arc<str>,
    ranges: Arc<[Range<usize>]>,
    start_line: usize,
    total_lines: NonZeroUsize,
}

impl PreviewText {
    #[must_use]
    fn new(text: String, target_line: usize) -> Self {
        let total_lines = text.lines().count();
        let window = NonZeroUsize::new(total_lines).map(|total_lines| {
            let start_line = target_line
                .min(total_lines.get() - 1)
                .saturating_sub(WINDOW_BEFORE_TARGET);
            let mut joined = String::new();
            let mut ranges = Vec::new();
            for (index, line) in text.lines().skip(start_line).take(WINDOW_LINES).enumerate() {
                if index > 0 {
                    joined.push('\n');
                }
                let start = joined.len();
                joined.push_str(line);
                ranges.push(start..joined.len());
            }
            LineWindow {
                text: Arc::from(joined),
                ranges: Arc::from(ranges),
                start_line,
                total_lines,
            }
        });
        Self {
            full: Arc::from(text),
            window,
        }
    }

    #[must_use]
    fn full(&self) -> &Arc<str> {
        &self.full
    }

    #[must_use]
    const fn window(&self) -> Option<&LineWindow> {
        self.window.as_ref()
    }

    #[must_use]
    fn note(&self) -> String {
        match &self.window {
            None => "This file is empty.".to_owned(),
            Some(window) => format!(
                "Lines {}–{} of {}. Long lines are clipped visually. Select All copies these preview lines; Copy file copies the full file.",
                window.start_line + 1,
                window.start_line + window.ranges.len(),
                window.total_lines
            ),
        }
    }
}

impl LineWindow {
    #[must_use]
    fn text(&self) -> &Arc<str> {
        &self.text
    }

    #[must_use]
    fn len(&self) -> usize {
        self.ranges.len()
    }

    #[must_use]
    const fn start_line(&self) -> usize {
        self.start_line
    }

    #[must_use]
    fn range(&self, index: usize) -> Range<usize> {
        self.ranges[index].clone()
    }

    #[must_use]
    fn line(&self, index: usize) -> &str {
        self.text
            .get(self.range(index))
            .expect("invariant: preview line ranges fall on char boundaries")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("no paired Forge can read {path}")]
    Unpaired { path: String },
    #[error("read {path} from Forge")]
    Read {
        path: String,
        #[source]
        source: crate::core::client::CallError,
    },
    #[error("{path} is not UTF-8 text")]
    NotText {
        path: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
    #[error("the Forge {host} that held {path} disconnected before it answered")]
    HostGone {
        path: String,
        host: crate::core::HostId,
    },
}

impl PreviewError {
    #[must_use]
    const fn retryable(&self) -> bool {
        match self {
            Self::Read { .. } | Self::HostGone { .. } => true,
            Self::Unpaired { .. } | Self::NotText { .. } => false,
        }
    }
}

type PreviewSource = (
    crate::core::HostId,
    crate::core::HostInstance,
    crate::core::ForgeClient,
    artisan_api::ThreadId,
);

pub struct ResourcePreview {
    pub(crate) path: String,
    target_line: usize,
    pub(crate) content: Option<Result<PreviewText, PreviewError>>,
    delivery: crate::core::delivery::Delivery<Result<PreviewText, PreviewError>>,
    request: Option<tokio_util::task::AbortOnDropHandle<()>>,
    source: Option<(crate::core::HostId, crate::core::HostInstance)>,
    thread: Option<artisan_api::ThreadId>,
}

impl std::fmt::Debug for ResourcePreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourcePreview").finish_non_exhaustive()
    }
}

impl ResourcePreview {
    #[must_use]
    pub const fn content(&self) -> Option<&Result<PreviewText, PreviewError>> {
        self.content.as_ref()
    }

    pub fn new(path: String, line: Option<u32>, source: Option<PreviewSource>) -> Self {
        let target_line = line.unwrap_or(1).saturating_sub(1) as usize;
        let mut preview = Self {
            path,
            target_line,
            content: None,
            delivery: crate::core::delivery::Delivery::new(NonZeroUsize::MIN),
            request: None,
            source: None,
            thread: None,
        };
        preview.start(source);
        preview
    }

    fn start(&mut self, source: Option<PreviewSource>) {
        self.source = source
            .as_ref()
            .map(|(host, instance, _, _)| (host.clone(), *instance));
        self.thread = source.as_ref().map(|(_, _, _, thread)| thread.clone());
        let Some((_, _, client, thread)) = source else {
            self.request = None;
            self.content = Some(Err(PreviewError::Unpaired {
                path: self.path.clone(),
            }));
            return;
        };
        self.content = None;
        let fetch_path = self.path.clone();
        let target_line = self.target_line;
        let sender = self.delivery.sender();
        self.request = Some(tokio_util::task::AbortOnDropHandle::new(
            crate::core::runtime::runtime().spawn(async move {
                let result = match client.resource(&thread, &fetch_path, false, None).await {
                    Ok(bytes) => String::from_utf8(bytes)
                        .map(|text| PreviewText::new(text, target_line))
                        .map_err(|source| PreviewError::NotText {
                            path: fetch_path,
                            source,
                        }),
                    Err(source) => Err(PreviewError::Read {
                        path: fetch_path,
                        source,
                    }),
                };
                if sender.send(result).await.is_err() {
                    tracing::debug!("the source preview closed before its file was read");
                }
            }),
        ));
    }

    pub(crate) fn retry(&mut self, core: &crate::core::Core) -> bool {
        if !self
            .content
            .as_ref()
            .is_some_and(|content| content.as_ref().is_err_and(PreviewError::retryable))
        {
            return false;
        }
        let source = self
            .source
            .as_ref()
            .map(|(host, _)| host)
            .zip(self.thread.clone())
            .and_then(|(host, thread)| {
                core.host(host)
                    .map(|data| (host.clone(), data.instance(), data.client.clone(), thread))
            });
        self.start(source);
        true
    }

    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }

    pub fn drain(&mut self, core: &crate::core::Core) -> bool {
        let gone = self
            .source
            .as_ref()
            .filter(|(host, instance)| core.host_if_instance(host, *instance).is_none())
            .map(|(host, _)| host.clone());
        let delivered = self.delivery.drain().into_iter().next();
        match (gone, delivered) {
            (Some(host), _) if self.content.is_none() => {
                self.request = None;
                self.content = Some(Err(PreviewError::HostGone {
                    path: self.path.clone(),
                    host,
                }));
                true
            }
            (Some(_), _) | (None, None) => false,
            (None, Some(result)) => {
                self.request = None;
                self.content = Some(result);
                true
            }
        }
    }
}
pub mod view;
