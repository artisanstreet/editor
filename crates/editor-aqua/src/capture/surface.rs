use aqua::render::link::ExternalUpdate;
use aqua::render::lower::Lowering;
use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend};
use aqua_gpu_ref::conformance::render as render_plan;
use aqua_protocol::{GpuBackend, SurfacePixels};

#[derive(Debug, thiserror::Error)]
pub enum SurfaceError {
    #[error("no D3D12 adapter for capture: {message}")]
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
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub adapter: String,
    pub backend: String,
}

pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Pixels {
    pub fn is_opaque(&self) -> bool {
        self.rgba
            .as_chunks::<4>()
            .0
            .iter()
            .all(|[_, _, _, alpha]| *alpha == 255)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), SaveError> {
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

pub fn physical(logical: f64, scale: f64) -> u32 {
    (logical * scale).round().max(1.0) as u32
}

impl Surface {
    pub fn new(
        logical_width: f64,
        logical_height: f64,
        scale: f64,
        fonts: aqua_text::SharedFonts,
        runs: aqua_text::SharedRuns,
    ) -> Result<Self, SurfaceError> {
        let width = physical(logical_width, scale);
        let height = physical(logical_height, scale);
        let device = OffscreenBackend::new(AdapterPreference::MinimumPower)
            .or_else(|_| OffscreenBackend::new(AdapterPreference::Warp))
            .map_err(|error| SurfaceError::NoAdapter {
                message: format!("{error:?}"),
            })?;
        let adapter = match device.is_software() {
            true => "WARP".to_owned(),
            false => "D3D12 hardware adapter".to_owned(),
        };
        Ok(Self {
            device,
            lowering: Lowering::new(fonts, runs).with_frozen_shaders(aqua_protocol::Instant::ZERO),
            latest: None,
            width,
            height,
            scale: scale as f32,
            adapter,
            backend: "D3D11".to_owned(),
        })
    }

    pub fn register_externals<A: aqua::Application>(
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

    pub fn draw(
        &mut self,
        scene: &aqua::prelude::Scene,
        now: aqua_protocol::Instant,
    ) -> Result<(), SurfaceError> {
        self.lowering.at(now);
        let size = aqua_core::Size::new(
            self.width as f32 / self.scale,
            self.height as f32 / self.scale,
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

    pub fn read(&mut self) -> Result<Pixels, SurfaceError> {
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
