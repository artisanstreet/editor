mod view;
#[cfg(feature = "test-support")]
pub use view::AudioFixture;
pub(crate) use view::AudioState;

use crate::ui::media::source::MediaSource;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(crate) struct AudioBlock {
    tracks: Vec<Track>,
}
impl AudioBlock {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_vec};
        std::mem::size_of::<Self>()
            + retained_vec(&self.tracks)
            + self
                .tracks
                .iter()
                .map(|track| track.src.resident_bytes() + retained_optional(track.name.as_ref()))
                .sum::<usize>()
    }
}

#[derive(Debug, Deserialize)]
struct Spec {
    #[serde(default)]
    src: Option<MediaSource>,
    #[serde(default, alias = "label")]
    name: Option<String>,
    #[serde(default)]
    tracks: Vec<Track>,
}

#[derive(Clone, Debug, Deserialize)]
struct Track {
    src: MediaSource,
    #[serde(default, alias = "label")]
    name: Option<String>,
}

impl Track {
    fn shown_name(&self) -> String {
        let named = self
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let file = || {
            let src = self.src.trim().trim_end_matches('/');
            src.rsplit(['/', '\\']).next().unwrap_or(src).to_owned()
        };
        String::from(named.map_or_else(file, str::to_owned))
    }
}

impl TryFrom<Spec> for AudioBlock {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        let mut tracks = spec.tracks;
        if let Some(src) = spec.src {
            tracks.insert(
                0,
                Track {
                    src,
                    name: spec.name,
                },
            );
        }
        tracks.retain(|track| !track.src.trim().is_empty());
        if tracks.len() > 16 {
            return Err("audio preview supports at most 16 tracks".into());
        }
        if tracks.is_empty() {
            return Err("audio needs a `src` or `tracks`".to_owned());
        }
        Ok(Self { tracks })
    }
}
