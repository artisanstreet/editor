use super::{BlobAction, RealizeRowError, RowFactory, Service};
use artisan_api::ItemKind;
use editor_ui::kit::{Button, ButtonTone, Space, Spaced, TextRole};
use std::rc::Rc;

pub(crate) fn image_caption(
    text: &str,
    theme: &editor_ui::theme::ArtisanTheme,
) -> aqua_ui::element::Element {
    use aqua_ui::style::Length;
    aqua_ui::element::Box::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(
            TextRole::Label
                .text(theme, text.to_owned())
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .wrap(true)
                .build(),
        )
        .build()
}

impl RowFactory {
    pub(super) fn user_attachments(
        &self,
        owner: &super::super::row_cache::RowOwner,
        item: &artisan_api::Item,
    ) -> Result<Option<aqua_ui::element::Element>, RealizeRowError> {
        use aqua_ui::element::Box as UiBox;
        use aqua_ui::style::{Length, Style};
        let ItemKind::UserMessage { images, .. } = &item.kind else {
            return Ok(None);
        };
        if images.is_empty() {
            return Ok(None);
        }
        let mut row = UiBox::new().layout("row").spaced(Space::S8).style(
            Style::new()
                .wrap(true)
                .max_width(Length::Px(
                    editor_ui::chat_bubble::ChatBubbleSegment::MAX_WIDTH,
                ))
                .justify(aqua_ui::JustifyContent::END),
        );
        for (index, hash) in images.iter().enumerate() {
            row = row.child(self.blob_frame(owner, item, hash, Some(index), 96.0)?);
        }
        Ok(Some(row.build()))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one frame per blob state, from loading through failed retry to the opened image"
    )]
    pub(super) fn blob_frame(
        &self,
        owner: &super::super::row_cache::RowOwner,
        item: &artisan_api::Item,
        hash: &artisan_api::BlobHash,
        attachment: Option<usize>,
        width: f32,
    ) -> Result<aqua_ui::element::Element, RealizeRowError> {
        use aqua_core::image::ImageFit;
        use aqua_ui::element::{Box as UiBox, Media};
        use aqua_ui::style::{Length, Style};
        use editor_core::host::BlobImage;
        use editor_ui::media_frame::MediaFrame;
        let resources = self
            .blob_images
            .as_ref()
            .ok_or(RealizeRowError::Missing(Service::Images))?;
        let state = resources.images.lookup(hash);
        if !matches!(&state, Some(BlobImage::Ready(_))) {
            self.unsettled.set(true);
        }
        let image = match &state {
            Some(BlobImage::Ready(image)) => Some((**image).clone()),
            _ => None,
        };
        let now = self.now;
        let arrival = owner.motion.borrow_mut().arrival(
            format!("image-{hash}-{attachment:?}"),
            image.is_some(),
            self.reduced_motion,
            now,
        );
        let content = match (&state, &image) {
            (_, Some(image)) if attachment.is_some() => Media::new(image.clone())
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .fit(ImageFit::Cover)
                .style(MediaFrame::sent_attachment_content_style())
                .build(),
            (_, Some(image)) => MediaFrame::image_content(image.clone()),
            (Some(BlobImage::Failed(error)), _) => {
                let emit = Rc::clone(&resources.emit);
                let active = Rc::clone(&self.active);
                let id = item.id.clone();
                let seq = item.seq;
                let retry_hash = hash.clone();
                let retry_id = attachment.map_or_else(
                    || format!("retry-image-{hash}"),
                    |index| format!("retry-sent-{}-{index}", item.id),
                );
                let retry = Button::text(retry_id, self.theme, ButtonTone::Neutral, "Try again")
                    .on_activate(move |_| {
                        if active.get() {
                            emit(
                                id.clone(),
                                seq,
                                BlobAction::Load {
                                    hash: retry_hash.clone(),
                                    retry: true,
                                },
                            );
                        }
                    })
                    .build();
                let message = if attachment.is_some() {
                    "Couldn't load".to_owned()
                } else {
                    format!("Couldn't load the image: {error}")
                };
                let role = match attachment {
                    Some(_) => TextRole::Caption,
                    None => TextRole::Label,
                };
                let text = role
                    .text(&self.theme, message)
                    .gpui_source_color(self.theme.colors.text_muted.to_reference_text_color())
                    .build();
                let mut column = UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0))
                    .style(
                        Style::new()
                            .align(aqua_ui::AlignItems::CENTER)
                            .justify(aqua_ui::JustifyContent::CENTER),
                    );
                if attachment.is_some() {
                    column = column.padded(Space::S8);
                }
                column.child(text).child(retry).build()
            }
            _ if attachment.is_none() => MediaFrame::placeholder(&self.theme),
            _ => UiBox::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0))
                .build(),
        };
        let content = match arrival {
            Some(progress) => super::super::motion::arrived(content, progress),
            None => content,
        };
        let aspect = image
            .as_ref()
            .and_then(|image| match image.status() {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "image dimensions are bounded by the decoder"
                )]
                aqua::images::ImageStatus::Ready { width, height } => {
                    Some(width as f32 / height.max(1) as f32)
                }
                aqua::images::ImageStatus::Pending
                | aqua::images::ImageStatus::Failed(_)
                | aqua::images::ImageStatus::Cancelled => None,
            })
            .unwrap_or(16.0 / 9.0);
        let mut frame = if attachment.is_some() {
            MediaFrame::sent_attachment(self.theme, content)
        } else {
            MediaFrame::transcript_image(self.theme, aspect, width, content)
        };
        let emit = Rc::clone(&resources.emit);
        let active = Rc::clone(&self.active);
        let id = item.id.clone();
        let seq = item.seq;
        let visible_hash = hash.clone();
        frame = frame.on_visible(move || {
            if active.get() {
                emit(
                    id.clone(),
                    seq,
                    BlobAction::Load {
                        hash: visible_hash.clone(),
                        retry: false,
                    },
                );
            }
        });
        if let Some(index) = attachment {
            frame = frame.debug_selector(format!("sent-image-{}-{index}", item.id));
        }
        if let Some(image) = image {
            let opener_key = attachment.map_or_else(
                || format!("image-{hash}"),
                |index| format!("sent-image-{}-{index}", item.id),
            );
            let emit = Rc::clone(&resources.emit);
            let active = Rc::clone(&self.active);
            let id = item.id.clone();
            let open_hash = hash.clone();
            let key = opener_key.clone();
            frame = frame.on_activate(
                opener_key,
                if attachment.is_some() {
                    "View attached image"
                } else {
                    "View image"
                },
                move |_| {
                    if active.get() {
                        emit(
                            id.clone(),
                            seq,
                            BlobAction::Open {
                                hash: open_hash.clone(),
                                image: image.clone(),
                                opener_key: key.clone(),
                            },
                        );
                    }
                },
            );
        }
        let frame = frame.build();
        if attachment.is_some() {
            Ok(frame)
        } else {
            Ok(UiBox::new()
                .width(Length::Pct(100.0))
                .style(Style::new().max_width(Length::Px(
                    editor_ui::chat_bubble::ChatBubbleSegment::MAX_WIDTH,
                )))
                .child(frame)
                .build())
        }
    }
}
