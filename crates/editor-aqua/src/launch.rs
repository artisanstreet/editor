use crate::core::EditorFiles;
use crate::shell::{application::ShellApplication, host::ShellResources, root::RootViews};

fn fixture_configuration() -> anyhow::Result<(std::path::PathBuf, f64, f64)> {
    let directory = std::env::var_os("ARTISAN_AQUA_FIXTURE_DIR")
        .ok_or_else(|| anyhow::anyhow!("qualification requires ARTISAN_AQUA_FIXTURE_DIR"))?;
    Ok((
        directory.into(),
        std::env::var("ARTISAN_AQUA_WIDTH")?.parse()?,
        std::env::var("ARTISAN_AQUA_HEIGHT")?.parse()?,
    ))
}

fn launch_dimensions() -> (f64, f64) {
    let size = crate::shell::layout::launch_size(None);
    (f64::from(size.width), f64::from(size.height))
}

#[cfg(feature = "qualification")]
fn prepare_qualification() -> Option<anyhow::Result<()>> {
    use crate::qualification::preparation;
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [mode, path] if mode == "--qualify-prepare" => {
            Some(preparation::prepare(std::path::Path::new(path)))
        }
        [mode, path, digest] if mode == "--qualify-restore" => Some(
            digest
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("seed SHA-256 must be UTF-8"))
                .and_then(|digest| preparation::restore(std::path::Path::new(path), digest)),
        ),
        _ if arguments
            .iter()
            .any(|argument| argument == "--qualify-prepare" || argument == "--qualify-restore") =>
        {
            Some(Err(anyhow::anyhow!(
                "qualification preparation requires --qualify-prepare REQUEST or --qualify-restore SEED SHA256"
            )))
        }
        _ => None,
    }
}

pub fn launch() -> anyhow::Result<()> {
    #[cfg(windows)]
    aqua::native::startup_mark("main_enter");
    #[cfg(windows)]
    match aqua::prelude::handle_launch() {
        aqua::prelude::Launch::Handled => return Ok(()),
        aqua::prelude::Launch::Cold(activation) => {
            crate::shell::native_notifications::record_cold_launch(activation.response)
        }
        aqua::prelude::Launch::Ordinary => {}
    }
    #[cfg(feature = "qualification")]
    if let Some(result) = prepare_qualification() {
        return result;
    }
    #[cfg(feature = "capture")]
    if crate::capture::cli::run_from_environment()? {
        return Ok(());
    }
    #[cfg(windows)]
    aqua::native::startup_mark("launch_handled");
    #[cfg(windows)]
    aqua::native::prewarm_gpu();
    let source_encoded = std::env::args()
        .skip(1)
        .any(|arg| arg == "--source-encoded");
    let qualifying = std::env::args().any(|argument| argument == "--qualify-editor");
    anyhow::ensure!(
        !qualifying
            || (!std::env::args()
                .skip(1)
                .any(|argument| crate::core::Screen::from_link(&argument).is_some())
                && std::env::var_os("ARTISAN_EDITOR_SCRIPT").is_none()
                && std::env::var_os("ARTISAN_EDITOR_CAPTURE").is_none()),
        "qualification does not accept an external route or legacy capture script"
    );
    #[cfg(not(feature = "qualification"))]
    anyhow::ensure!(!qualifying, "qualification feature is not enabled");
    #[cfg(feature = "qualification")]
    let bootstrap = if qualifying {
        let path = std::env::var_os("ARTISAN_EDITOR_QUALIFICATION")
            .ok_or_else(|| anyhow::anyhow!("qualification request path is required"))?;
        Some(crate::qualification::bootstrap::Bootstrap::load(
            std::path::Path::new(&path),
        )?)
    } else {
        None
    };
    #[cfg(feature = "qualification")]
    let (directory, width, height) = match &bootstrap {
        Some(bootstrap) => {
            let (width, height) = bootstrap.dimensions();
            (bootstrap.editor_directory().to_owned(), width, height)
        }
        None if source_encoded => fixture_configuration()?,
        None => {
            let (width, height) = launch_dimensions();
            (EditorFiles::default_location().dir, width, height)
        }
    };
    #[cfg(not(feature = "qualification"))]
    let (directory, width, height) = if source_encoded {
        fixture_configuration()?
    } else {
        let (width, height) = launch_dimensions();
        (EditorFiles::default_location().dir, width, height)
    };
    anyhow::ensure!(
        width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0,
        "qualification window dimensions must be positive"
    );
    let fit_launch = !qualifying && !source_encoded;
    if !qualifying {
        crate::logging::init();
    }
    let mut config = aqua::AquaConfig::default();
    config.window.title = "Artisan Editor".to_owned();
    config.window.inner_size.width = width;
    config.window.inner_size.height = height;
    let mut minimum = config.window.inner_size;
    minimum.width = 480.0;
    minimum.height = 420.0;
    config.window.min_size = Some(minimum);
    config.window.titlebar = aqua::config::Titlebar::Overlay;
    #[cfg(windows)]
    aqua::native::startup_mark("config_ready");
    let runtime = aqua::Aqua::new(config).map_err(anyhow::Error::msg)?;
    #[cfg(windows)]
    aqua::native::startup_mark("runtime_ready");
    let link = std::env::args()
        .skip(1)
        .find_map(|argument| crate::core::Screen::from_link(&argument));
    let resources = ShellResources::for_runtime(&runtime, 1.0)
        .ok_or_else(|| anyhow::anyhow!("could not configure Editor layout"))?;
    let resources = resources.with_color_profile(aqua_core::ColorProfile::GpuiWindowsEncoded);
    #[cfg(windows)]
    aqua::native::startup_mark("resources_ready");
    #[cfg(feature = "qualification")]
    let app = match &bootstrap {
        Some(bootstrap) => {
            ShellApplication::for_qualification(bootstrap.core()?, resources, RootViews::default())
        }
        None => ShellApplication::opening(
            EditorFiles { dir: directory },
            resources,
            RootViews::default(),
            link,
        ),
    };
    #[cfg(not(feature = "qualification"))]
    let app = ShellApplication::opening(
        EditorFiles { dir: directory },
        resources,
        RootViews::default(),
        link,
    );
    let app = if fit_launch {
        app.fitting_launch_to_display()
    } else {
        app
    };
    #[cfg(windows)]
    aqua::native::startup_mark("app_ready");
    let result = runtime.run(app).map_err(anyhow::Error::msg);
    #[cfg(feature = "qualification")]
    drop(bootstrap);
    result
}
