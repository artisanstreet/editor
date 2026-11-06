mod view;
pub(crate) use view::{Action, ImageState};

use super::SizeHint;
use crate::ui::media::source::MediaSource;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(crate) struct ImageBlock {
    images: Vec<Picture>,
    hint: SizeHint,
}
impl ImageBlock {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_vec};
        std::mem::size_of::<Self>()
            + retained_vec(&self.images)
            + self
                .images
                .iter()
                .map(|image| {
                    image.src.resident_bytes()
                        + retained_optional(image.alt.as_ref())
                        + retained_optional(image.caption.as_ref())
                })
                .sum::<usize>()
    }
}

#[derive(Debug, Deserialize)]
struct Spec {
    #[serde(default)]
    src: Option<MediaSource>,
    #[serde(default)]
    alt: Option<String>,
    #[serde(default, alias = "paths")]
    images: Vec<PictureInput>,
    #[serde(flatten)]
    hint: SizeHint,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PictureInput {
    Path(MediaSource),
    Detailed(Picture),
}

impl From<PictureInput> for Picture {
    fn from(input: PictureInput) -> Self {
        match input {
            PictureInput::Path(src) => Self {
                src,
                alt: None,
                caption: None,
            },
            PictureInput::Detailed(picture) => picture,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Picture {
    src: MediaSource,
    #[serde(default)]
    alt: Option<String>,
    #[serde(default)]
    caption: Option<String>,
}

impl TryFrom<Spec> for ImageBlock {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        let mut images: Vec<Picture> = spec.images.into_iter().map(Picture::from).collect();
        if let Some(src) = spec.src {
            images.insert(
                0,
                Picture {
                    src,
                    alt: spec.alt,
                    caption: None,
                },
            );
        }
        images.retain(|image| !image.src.trim().is_empty());
        if images.len() > 16 {
            return Err("image gallery preview supports at most 16 images".into());
        }
        if images.is_empty() {
            return Err("an image needs a `src` or `images`".to_owned());
        }
        Ok(Self {
            images,
            hint: spec.hint,
        })
    }
}

fn gallery_rows(mut count: usize) -> Vec<u16> {
    let mut rows = Vec::new();
    while count > 0 {
        let columns = match count {
            1 => 1,
            2 | 4 => 2,
            _ if count.is_multiple_of(3) => 3,
            _ => 2,
        };
        rows.push(columns);
        count -= usize::from(columns);
    }
    rows
}
