use aqua::render::link::ExternalUpdate;
use aqua::render::lower::Lowering;
use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend};
use aqua_gpu_ref::conformance::render as render_plan;
use aqua_protocol::{GpuBackend, SurfacePixels};

#[derive(Debug, thiserror::Error)]
pub enum SurfaceError {
    #[error("the WARP reference renderer is unavailable for capture: {message}")]
    NoAdapter { message: String },
    #[error("lower the scene: {message}")]
    Lower { message: String },
    #[error("upload the scene: {message}")]
    Upload { message: String },
    #[error("draw the scene: {message}")]
    Draw { message: String },
    #[error("nothing has been drawn yet")]
    NothingDrawn,
    #[error("the readback holds {actual} bytes, not {expected}")]
    ReadbackSize { expected: usize, actual: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("a {width}x{height} capture cannot hold {bytes} bytes, so {} was not saved", path.display())]
    Dimensions {
        path: std::path::PathBuf,
        width: u32,
        height: u32,
        bytes: usize,
    },
    #[error("create {}", path.display())]
    Directory {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("save {}", path.display())]
    Encode {
        path: std::path::PathBuf,
        #[source]
        source: image::ImageError,
    },
}

pub struct Surface {
    device: OffscreenBackend,
    lowering: Lowering,
    latest: Option<SurfacePixels>,
    width: u32,
    height: u32,
    pub(crate) scale: f32,
    pub(crate) adapter: String,
    pub(crate) backend: String,
}

impl std::fmt::Debug for Surface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Surface")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("scale", &self.scale)
            .field("adapter", &self.adapter)
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

pub struct Pixels {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
}

impl std::fmt::Debug for Pixels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pixels")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl Pixels {
    pub(crate) fn is_opaque(&self) -> bool {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|[_, _, _, alpha]| *alpha == 255)
    }

    pub(crate) fn save(&self, path: &std::path::Path) -> Result<(), SaveError> {
        let image = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .ok_or_else(|| SaveError::Dimensions {
                path: path.to_owned(),
                width: self.width,
                height: self.height,
                bytes: self.rgba.len(),
            })?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| SaveError::Directory {
                path: parent.to_owned(),
                source,
            })?;
        }
        image
            .save_with_format(path, image::ImageFormat::Png)
            .map_err(|source| SaveError::Encode {
                path: path.to_owned(),
                source,
            })
    }
}

fn physical(logical: f64, scale: f64) -> u32 {
    crate::measure::saturate_pixels((logical * scale).round().max(1.0))
}

impl Surface {
    pub(crate) fn new(
        logical_width: f64,
        logical_height: f64,
        scale: f64,
        fonts: aqua_text::SharedFonts,
        runs: aqua_text::SharedRuns,
    ) -> Result<Self, SurfaceError> {
        let width = physical(logical_width, scale);
        let height = physical(logical_height, scale);
        let device = OffscreenBackend::new(AdapterPreference::Warp).map_err(|error| {
            SurfaceError::NoAdapter {
                message: format!("{error:?}"),
            }
        })?;
        Ok(Self {
            device,
            lowering: Lowering::new(fonts, runs).with_frozen_shaders(aqua_protocol::Instant::ZERO),
            latest: None,
            width,
            height,
            scale: crate::measure::narrow(scale),
            adapter: "WARP".to_owned(),
            backend: "D3D11".to_owned(),
        })
    }

    pub(crate) fn register_externals<A: aqua::Application>(
        &mut self,
        app: &mut A,
        window: &aqua::prelude::ObservedWindowState,
        now: aqua::Instant,
    ) {
        for update in aqua::run_headless_externals(app, window, now) {
            match update {
                ExternalUpdate::Image {
                    layer,
                    width,
                    height,
                    rgba,
                } => self.lowering.register_image(layer, width, height, &rgba),
                ExternalUpdate::Release { layer } => self.lowering.release_image(layer),
                ExternalUpdate::Transition(_) | ExternalUpdate::TextRaster(_) => {}
            }
        }
    }

    pub(crate) fn draw(
        &mut self,
        scene: &aqua::prelude::Scene,
        now: aqua_protocol::Instant,
    ) -> Result<(), SurfaceError> {
        self.lowering.at(now);
        let size = aqua_core::Size::new(
            crate::measure::exact(self.width) / self.scale,
            crate::measure::exact(self.height) / self.scale,
        );
        let lowered = self
            .lowering
            .lower(
                scene,
                size,
                self.scale,
                self.device.capabilities().plan_limits(),
            )
            .map_err(|error| SurfaceError::Lower {
                message: format!("{error:?}"),
            })?;
        self.device
            .upload(lowered.uploads)
            .map_err(|error| SurfaceError::Upload {
                message: format!("{error:?}"),
            })?;
        self.latest = Some(
            render_plan(&mut self.device, &lowered.plan).map_err(|error| SurfaceError::Draw {
                message: format!("{error:?}"),
            })?,
        );
        Ok(())
    }

    pub(crate) fn read(&self) -> Result<Pixels, SurfaceError> {
        let pixels = self.latest.clone().ok_or(SurfaceError::NothingDrawn)?;
        let expected = self.width as usize * self.height as usize * 4;
        if pixels.rgba8.len() != expected {
            return Err(SurfaceError::ReadbackSize {
                expected,
                actual: pixels.rgba8.len(),
            });
        }
        Ok(Pixels {
            width: self.width,
            height: self.height,
            rgba: pixels.rgba8,
        })
    }
}
