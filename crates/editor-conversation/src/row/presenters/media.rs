//! Image and attachment presentation borrows decoded state and owns no cache.
use super::super::Blob;
use super::{Action, Actions};
use artisan_api::{BlobHash, Item, ItemId, ItemKind};
use editor_ui::{
    button::{Button, ButtonVariant},
    chat_bubble::ChatBubbleSegment,
    media_frame::MediaFrame,
    theme::ArtisanTheme,
};
use gpui::{
    AnyElement, Div, IntoElement, ObjectFit, ParentElement as _, SharedString, Styled as _,
    StyledImage as _, div, img, px,
};
use std::collections::HashMap;
pub(in crate::row) struct MediaPresenter<'a> {
    pub theme: ArtisanTheme,
    pub images: &'a HashMap<BlobHash, Blob>,
    pub actions: Actions,
}
impl MediaPresenter<'_> {
    pub(in crate::row) fn user_attachments(&self, item: &Item) -> Option<Div> {
        let ItemKind::UserMessage { images, .. } = &item.kind else {
            return None;
        };
        if images.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(8.0))
                .max_w(px(ChatBubbleSegment::MAX_WIDTH))
                .children(
                    images
                        .iter()
                        .enumerate()
                        .map(|(index, hash)| self.image_tile(&item.id, index, hash)),
                ),
        )
    }

    fn image_tile(&self, item: &ItemId, index: usize, hash: &BlobHash) -> AnyElement {
        let theme = self.theme;
        let (content, image) = match self.images.get(hash) {
            Some(Blob::Ready(image)) => (
                img(std::sync::Arc::clone(image))
                    .size_full()
                    .rounded(MediaFrame::sent_attachment_content_radius())
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                Some(std::sync::Arc::clone(image)),
            ),
            Some(Blob::Failed(_)) => {
                let retry = self.actions.handler(Action::RetryImage(hash.clone()));
                (
                    div()
                        .size_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .px(px(8.0))
                        .text_size(theme.typography.label_text)
                        .text_color(theme.colors.text_muted.to_paint())
                        .child("Couldn't load")
                        .child(
                            Button::text(format!("retry-sent-{item}-{index}"), theme, "Retry")
                                .variant(ButtonVariant::Ghost)
                                .on_activate(move |_, _, cx| retry(cx)),
                        )
                        .into_any_element(),
                    None,
                )
            }
            _ => (div().size_full().into_any_element(), None),
        };
        let mut frame = MediaFrame::sent_attachment(theme, content)
            .debug_selector(format!("sent-image-{item}-{index}"))
            .on_visible(self.actions.handler(Action::LoadImage(hash.clone())));
        if let Some(image) = image {
            frame = frame.on_activate(
                SharedString::from(format!("sent-image-{item}-{index}")),
                "View attached image",
                editor_ui::image_preview::open_on_click(image),
            );
        }
        frame.into_any_element()
    }

    fn image(&self, hash: &BlobHash) -> AnyElement {
        let theme = self.theme;
        let retry_hash = hash.clone();
        let retry = self.actions.handler(Action::RetryImage(retry_hash));
        let (content, image): (AnyElement, _) = match self.images.get(hash) {
            Some(Blob::Ready(image)) => (
                MediaFrame::image_content(image).into_any_element(),
                Some(std::sync::Arc::clone(image)),
            ),
            Some(Blob::Failed(error)) => (
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.text_muted.to_paint())
                    .flex_col()
                    .child(format!("Couldn't load the image: {error}"))
                    .child(
                        Button::text(format!("retry-image-{hash}"), theme, "Retry")
                            .variant(ButtonVariant::Ghost)
                            .on_activate(move |_, _, cx| retry(cx)),
                    )
                    .into_any_element(),
                None,
            ),
            _ => (
                div()
                    .size_full()
                    .bg(theme.colors.raised.to_paint())
                    .into_any_element(),
                None,
            ),
        };
        let aspect = image.as_ref().map_or(16.0 / 9.0, |image| {
            let size = image.size(0);
            #[expect(
                clippy::cast_precision_loss,
                reason = "decoded image edges are bounded"
            )]
            {
                size.width.0 as f32 / size.height.0.max(1) as f32
            }
        });
        let mut frame = MediaFrame::transcript_image(theme, aspect, content)
            .on_visible(self.actions.handler(Action::LoadImage(hash.clone())));
        if let Some(image) = image {
            frame = frame.on_activate(
                SharedString::from(format!("image-{hash}")),
                "View image",
                editor_ui::image_preview::open_on_click(image),
            );
        }
        div()
            .w_full()
            .max_w(px(ChatBubbleSegment::MAX_WIDTH))
            .child(frame)
            .into_any_element()
    }

    pub(in crate::row) fn image_row(&self, item: &Item) -> AnyElement {
        let ItemKind::Image { blob, .. } = &item.kind else {
            return div().into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(self.image(blob))
            .into_any_element()
    }
}
