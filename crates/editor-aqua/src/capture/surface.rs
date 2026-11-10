use anyhow::{Context, Result, anyhow, ensure};
use aqua::render::lower::Lowering;
use aqua::render::link::ExternalUpdate;
use aqua_gpu_d3d11::scene::{AdapterPreference, OffscreenBackend};
use aqua_gpu_ref::conformance::render as render_plan;
use aqua_protocol::{GpuBackend, SurfacePixels};

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
        self.rgba.chunks_exact(4).all(|pixel| pixel[3] == 255)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let image = image::RgbaImage::from_raw(self.width, self.height, self.rgba.clone())
            .context("capture buffer does not match its dimensions")?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        image
            .save_with_format(path, image::ImageFormat::Png)
            .with_context(|| format!("save {}", path.display()))
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
    ) -> Result<Self> {
        let width = physical(logical_width, scale);
        let height = physical(logical_height, scale);
        let device = OffscreenBackend::new(AdapterPreference::MinimumPower)
            .or_else(|_| OffscreenBackend::new(AdapterPreference::Warp))
            .map_err(|error| anyhow!("no D3D12 adapter for capture: {error:?}"))?;
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

    pub fn draw(&mut self, scene: &aqua::prelude::Scene, now: aqua_protocol::Instant) -> Result<()> {
        self.lowering.at(now);
        let size = aqua_core::Size::new(
            self.width as f32 / self.scale,
            self.height as f32 / self.scale,
        );
        let lowered = self
            .lowering
            .lower(scene, size, self.scale, self.device.capabilities().plan_limits())
            .map_err(|error| anyhow!("capture lowering failed: {error:?}"))?;
        self.device
            .upload(lowered.uploads)
            .map_err(|error| anyhow!("capture upload failed: {error:?}"))?;
        self.latest = Some(
            render_plan(&mut self.device, &lowered.plan)
                .map_err(|error| anyhow!("capture draw failed: {error:?}"))?,
        );
        Ok(())
    }

    pub fn read(&mut self) -> Result<Pixels> {
        let pixels = self.latest.clone().context("nothing has been drawn yet")?;
        ensure!(
            pixels.rgba8.len() == self.width as usize * self.height as usize * 4,
            "readback has an unexpected size"
        );
        Ok(Pixels {
            width: self.width,
            height: self.height,
            rgba: pixels.rgba8,
        })
    }
}
