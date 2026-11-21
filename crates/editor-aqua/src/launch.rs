use crate::core::{DataDirError, EditorFiles};
use crate::shell::{application::ShellApplication, host::ShellResources, root::RootViews};

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("find the Editor's data folder")]
    DataDir(#[source] DataDirError),
    #[error("start logging")]
    Logging(#[source] diagnostics::SubscribeError),
    #[error(
        "start logging, and the temporary log failed too: {}",
        diagnostics::chain(record)
    )]
    Unlogged {
        #[source]
        source: diagnostics::SubscribeError,
        record: crate::logging::RecordError,
    },
    #[error("check whether this program installs or updates the Editor")]
    Setup(#[source] crate::setup::BeginError),
    #[cfg(feature = "qualification")]
    #[error("prepare a qualification seed")]
    Prepare(#[source] crate::qualification::preparation::PrepareError),
    #[cfg(feature = "qualification")]
    #[error("restore a qualification seed")]
    Restore(#[source] crate::qualification::preparation::RestoreError),
    #[cfg(feature = "qualification")]
    #[error("the seed SHA-256 is not UTF-8")]
    SeedDigest,
    #[cfg(feature = "qualification")]
    #[error(
        "qualification preparation takes --qualify-prepare REQUEST or --qualify-restore SEED SHA256"
    )]
    QualifyArguments,
    #[cfg(feature = "capture")]
    #[error("run a capture")]
    Capture(#[source] crate::capture::cli::CliError),
    #[error("qualification does not accept an external route or legacy capture script")]
    QualificationRoute,
    #[cfg(not(feature = "qualification"))]
    #[error("this build cannot qualify: the qualification feature is off")]
    QualificationDisabled,
    #[cfg(feature = "qualification")]
    #[error("ARTISAN_EDITOR_QUALIFICATION names no qualification request")]
    QualificationRequest,
    #[cfg(feature = "qualification")]
    #[error("bootstrap qualification")]
    Bootstrap(#[source] crate::qualification::bootstrap::BootstrapError),
    #[cfg(feature = "qualification")]
    #[error("open the qualification Editor's core")]
    QualificationCore(#[source] crate::qualification::sign_in::OpenSignedInError),
    #[error("ARTISAN_AQUA_FIXTURE_DIR names no fixture directory")]
    FixtureDir,
    #[error("read {name}")]
    MissingDimension {
        name: &'static str,
        #[source]
        source: std::env::VarError,
    },
    #[error("{name}={value} is not a number")]
    Dimension {
        name: &'static str,
        value: String,
        #[source]
        source: std::num::ParseFloatError,
    },
    #[error("the window size {width}x{height} is not positive")]
    WindowSize { width: f64, height: f64 },
    #[error("start the window system: {message}")]
    Start { message: String },
    #[error("configure the Editor layout")]
    Layout,
    #[error("run the Editor: {message}")]
    Run { message: String },
    #[error(
        "could not finish saving drafts: {failed} failed, {timed_out} timed out, {kept_locally} kept on this device"
    )]
    Drafts {
        failed: usize,
        timed_out: usize,
        kept_locally: usize,
    },
    #[error("could not finish saving settings: {failed} saves failed, {timed_out} timed out")]
    Settings { failed: usize, timed_out: usize },
}

fn dimension(name: &'static str) -> Result<f64, LaunchError> {
    let value =
        std::env::var(name).map_err(|source| LaunchError::MissingDimension { name, source })?;
    value.parse().map_err(|source| LaunchError::Dimension {
        name,
        value,
        source,
    })
}

fn fixture_configuration() -> Result<(std::path::PathBuf, f64, f64), LaunchError> {
    let directory = std::env::var_os("ARTISAN_AQUA_FIXTURE_DIR").ok_or(LaunchError::FixtureDir)?;
    Ok((
        directory.into(),
        dimension("ARTISAN_AQUA_WIDTH")?,
        dimension("ARTISAN_AQUA_HEIGHT")?,
    ))
}

fn launch_dimensions() -> (f64, f64) {
    let size = crate::shell::layout::launch_size(None);
    (f64::from(size.width), f64::from(size.height))
}

#[cfg(feature = "qualification")]
fn prepare_qualification() -> Option<Result<(), LaunchError>> {
    use crate::qualification::preparation;
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    match arguments.as_slice() {
        [mode, path] if mode == "--qualify-prepare" => {
            Some(preparation::prepare(std::path::Path::new(path)).map_err(LaunchError::Prepare))
        }
        [mode, path, digest] if mode == "--qualify-restore" => Some(
            digest
                .to_str()
                .ok_or(LaunchError::SeedDigest)
                .and_then(|digest| {
                    preparation::restore(std::path::Path::new(path), digest)
                        .map_err(LaunchError::Restore)
                }),
        ),
        _ if arguments
            .iter()
            .any(|argument| argument == "--qualify-prepare" || argument == "--qualify-restore") =>
        {
            Some(Err(LaunchError::QualifyArguments))
        }
        _ => None,
    }
}

pub fn launch() -> Result<(), LaunchError> {
    #[cfg(windows)]
    aqua::native::startup_mark("main_enter");
    let qualifying = std::env::args().any(|argument| argument == "--qualify-editor");
    if !qualifying {
        start_logging()?;
    }
    #[cfg(windows)]
    match aqua::prelude::handle_launch() {
        aqua::prelude::Launch::Handled => return Ok(()),
        aqua::prelude::Launch::Cold(activation) => {
            crate::shell::native_notifications::record_cold_launch(activation.response);
        }
        aqua::prelude::Launch::Ordinary => {}
    }
    let launched = run(qualifying);
    if let Err(error) = &launched {
        logged(error);
    }
    launched
}

fn logged(error: &LaunchError) {
    tracing::error!(
        error = %::diagnostics::chain(error),
        "the Editor could not launch"
    );
}

fn start_logging() -> Result<(), LaunchError> {
    let files = EditorFiles::default_location();
    let recent = match crate::logging::init(files.as_ref().ok().map(|files| files.dir.as_path())) {
        Ok(recent) => recent,
        Err(source) => {
            return Err(match crate::logging::record_in_fallback(&source) {
                Ok(()) => LaunchError::Logging(source),
                Err(record) => LaunchError::Unlogged { source, record },
            });
        }
    };
    match files {
        Ok(files) => {
            diagnostics::CrashReporter {
                app: artisan_paths::App::Editor,
                version: concat!(env!("CARGO_PKG_VERSION"), " (", env!("EDITOR_COMMIT"), ")"),
                recent,
                data: files.dir,
            }
            .install();
            Ok(())
        }
        Err(source) => {
            let error = LaunchError::DataDir(source);
            logged(&error);
            Err(error)
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one launch sequence: setup, qualification, capture, links, then the window loop"
)]
fn run(qualifying: bool) -> Result<(), LaunchError> {
    if matches!(
        crate::setup::begin().map_err(LaunchError::Setup)?,
        crate::setup::Outcome::Finished
    ) {
        return Ok(());
    }
    #[cfg(feature = "qualification")]
    if let Some(result) = prepare_qualification() {
        return result;
    }
    #[cfg(feature = "capture")]
    if crate::capture::cli::run_from_environment().map_err(LaunchError::Capture)? {
        return Ok(());
    }
    #[cfg(windows)]
    aqua::native::startup_mark("launch_handled");
    #[cfg(windows)]
    aqua::native::prewarm_gpu();
    let source_encoded = std::env::args()
        .skip(1)
        .any(|arg| arg == "--source-encoded");
    if qualifying
        && (std::env::args().skip(1).any(|argument| {
            !matches!(
                crate::core::Screen::from_link(&argument),
                Err(crate::core::DeepLinkError::NotALink)
            )
        }) || std::env::var_os("ARTISAN_EDITOR_SCRIPT").is_some()
            || std::env::var_os("ARTISAN_EDITOR_CAPTURE").is_some())
    {
        return Err(LaunchError::QualificationRoute);
    }
    #[cfg(not(feature = "qualification"))]
    if qualifying {
        return Err(LaunchError::QualificationDisabled);
    }
    #[cfg(feature = "qualification")]
    let bootstrap = if qualifying {
        let path = std::env::var_os("ARTISAN_EDITOR_QUALIFICATION")
            .ok_or(LaunchError::QualificationRequest)?;
        Some(
            crate::qualification::bootstrap::Bootstrap::load(std::path::Path::new(&path))
                .map_err(LaunchError::Bootstrap)?,
        )
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
            (
                EditorFiles::default_location()
                    .map_err(LaunchError::DataDir)?
                    .dir,
                width,
                height,
            )
        }
    };
    #[cfg(not(feature = "qualification"))]
    let (directory, width, height) = if source_encoded {
        fixture_configuration()?
    } else {
        let (width, height) = launch_dimensions();
        (
            EditorFiles::default_location()
                .map_err(LaunchError::DataDir)?
                .dir,
            width,
            height,
        )
    };
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(LaunchError::WindowSize { width, height });
    }
    let fit_launch = !qualifying && !source_encoded;
    let mut config = aqua::AquaConfig::default();
    crate::shell::host::WINDOW_NAME.clone_into(&mut config.window.title);
    config.window.inner_size.width = width;
    config.window.inner_size.height = height;
    let mut minimum = config.window.inner_size;
    minimum.width = f64::from(crate::shell::layout::MIN_WINDOW_WIDTH);
    minimum.height = f64::from(crate::shell::layout::MIN_WINDOW_HEIGHT);
    config.window.min_size = Some(minimum);
    config.window.titlebar = aqua::config::Titlebar::Overlay;
    #[cfg(windows)]
    aqua::native::startup_mark("config_ready");
    let runtime = aqua::Aqua::new(config).map_err(|message| LaunchError::Start { message })?;
    #[cfg(windows)]
    aqua::native::startup_mark("runtime_ready");
    let link = std::env::args()
        .skip(1)
        .find_map(|argument| match crate::core::Screen::from_link(&argument) {
            Ok(screen) => Some(screen),
            Err(crate::core::DeepLinkError::NotALink) => None,
            Err(error) => {
                tracing::warn!(error = %::diagnostics::chain(&error), "ignored a link the Editor was opened with");
                None
            }
        });
    let resources = ShellResources::for_runtime(&runtime, 1.0).ok_or(LaunchError::Layout)?;
    let resources = resources.with_color_profile(aqua_core::ColorProfile::GpuiWindowsEncoded);
    #[cfg(windows)]
    aqua::native::startup_mark("resources_ready");
    #[cfg(feature = "qualification")]
    let app = match &bootstrap {
        Some(bootstrap) => ShellApplication::for_qualification(
            bootstrap.core().map_err(LaunchError::QualificationCore)?,
            resources,
            RootViews::default(),
        ),
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
    let result = runtime
        .run(app)
        .map_err(|message| LaunchError::Run { message });
    let (drafts, settings) = crate::core::runtime::runtime().block_on(async {
        tokio::join!(
            crate::composer::persistence::drain_pending(std::time::Duration::from_secs(5)),
            crate::core::drain_settings(std::time::Duration::from_secs(5)),
        )
    });
    let settings = match settings {
        crate::core::SettingsShutdown::Saved => Ok(()),
        crate::core::SettingsShutdown::Incomplete { failed, timed_out } => {
            tracing::error!(
                failed,
                timed_out,
                "Editor closed before every settings change was saved"
            );
            Err(LaunchError::Settings { failed, timed_out })
        }
    };
    let shutdown = match drafts {
        crate::composer::persistence::ShutdownOutcome::Saved => Ok(()),
        crate::composer::persistence::ShutdownOutcome::Incomplete {
            failed,
            timed_out,
            kept_locally,
        } if kept_locally == failed + timed_out => {
            tracing::warn!(
                failed,
                timed_out,
                kept_locally,
                "Editor closed before Forge saved every draft; they are kept on this device until it can"
            );
            Ok(())
        }
        crate::composer::persistence::ShutdownOutcome::Incomplete {
            failed,
            timed_out,
            kept_locally,
        } => {
            tracing::error!(
                failed,
                timed_out,
                kept_locally,
                "Editor closed before all drafts could be saved or kept on this device"
            );
            Err(LaunchError::Drafts {
                failed,
                timed_out,
                kept_locally,
            })
        }
    };
    #[cfg(feature = "qualification")]
    drop(bootstrap);
    result.and(shutdown).and(settings)
}
