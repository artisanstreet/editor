use std::ops::Range;

#[derive(Clone, Debug)]
pub struct PreviewText {
    pub text: String,
    pub start_line: usize,
    pub offsets: Vec<Range<usize>>,
    pub total_lines: usize,
}

impl PreviewText {
    pub fn new(text: String, target_line: usize) -> Self {
        let total_lines = text.lines().count();
        let start_line = target_line
            .min(total_lines.saturating_sub(1))
            .saturating_sub(2_500);
        let base = text.as_ptr() as usize;
        let offsets = text
            .lines()
            .skip(start_line)
            .take(5_000)
            .map(|line| {
                let start = line.as_ptr() as usize - base;
                start..start + line.len()
            })
            .collect();
        Self {
            text,
            start_line,
            offsets,
            total_lines,
        }
    }

    pub fn visible_line(&self, index: usize) -> Option<&str> {
        let text = &self.text[self.offsets.get(index)?.clone()];
        Some(&text[..text.floor_char_boundary(text.len().min(16 * 1024))])
    }

    pub fn note(&self) -> String {
        format!(
            "Lines {}–{} of {}. Long lines are clipped visually. Select All copies these preview lines; Copy file copies the full file.",
            self.start_line + 1,
            self.start_line + self.offsets.len(),
            self.total_lines
        )
    }
}

pub struct ResourcePreview {
    pub path: String,
    pub target_line: usize,
    pub content: Option<Result<PreviewText, String>>,
    delivery: crate::core::delivery::Delivery<Result<PreviewText, String>>,
    request: Option<tokio_util::task::AbortOnDropHandle<()>>,
    source: Option<(crate::core::HostId, crate::core::HostInstance)>,
}

impl ResourcePreview {
    pub fn new(
        path: String,
        line: Option<u32>,
        source: Option<(
            crate::core::HostId,
            crate::core::HostInstance,
            crate::core::ForgeClient,
            artisan_api::ThreadId,
        )>,
    ) -> Self {
        let target_line = line.unwrap_or(1).saturating_sub(1) as usize;
        let delivery = crate::core::delivery::Delivery::new(std::num::NonZeroUsize::MIN);
        let mut content = None;
        let identity = source
            .as_ref()
            .map(|(host, instance, _, _)| (host.clone(), *instance));
        let request = if let Some((_, _, client, thread)) = source {
            let fetch_path = path.clone();
            let sender = delivery.sender();
            Some(tokio_util::task::AbortOnDropHandle::new(
                crate::core::runtime::runtime().spawn(async move {
                    let result = client
                        .resource(&thread, &fetch_path, false)
                        .await
                        .map_err(|error| format!("Couldn't read this resource: {error:#}"))
                        .and_then(|bytes| {
                            String::from_utf8(bytes)
                                .map_err(|_| "This resource is not a UTF-8 text file.".to_owned())
                        })
                        .map(|text| PreviewText::new(text, target_line));
                    let _ = sender.send(result).await;
                }),
            ))
        } else {
            content = Some(Err(
                "This reference has no available paired Forge. You can copy its path.".to_owned(),
            ));
            None
        };
        Self {
            path,
            target_line,
            content,
            delivery,
            request,
            source: identity,
        }
    }

    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }

    pub fn drain(&mut self, core: &crate::core::Core) -> bool {
        let Some(result) = self.delivery.drain().into_iter().next() else {
            return false;
        };
        self.request = None;
        if self
            .source
            .as_ref()
            .is_some_and(|(host, instance)| core.host_if_instance(host, *instance).is_none())
        {
            return false;
        }
        self.content = Some(result);
        true
    }
}
pub mod view;
