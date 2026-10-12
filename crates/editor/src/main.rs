//! The Artisan Editor: one window holding the sections. Each section owns
//! its state; this crate only lays them out, opens the window and handles
//! what belongs to no section (titlebar, palette, notices, first pairing).

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod context_meter;
mod home;
mod notifications;
mod palette;
#[cfg(feature = "capture")]
mod script;
mod workspace;

use editor_core::Screen;
use gpui::{
    App, AppContext as _, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, px,
    size,
};
use workspace::Workspace;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

gpui::actions!(
    artisan,
    [
        Quit,
        OpenPalette,
        ToggleSidebar,
        ToggleInspector,
        GoBack,
        OpenSettings,
        NewThread
    ]
);

/// The log rolls over to `editor.log.1` past this size, at start.
const LOG_ROLL_BYTES: u64 = 8 * 1024 * 1024;

/// The Editor's log: `editor.log` in its data folder, as well as stderr,
/// which nobody sees when it is started from Explorer.
fn log_file() -> Option<std::fs::File> {
    let dir = editor_core::hosts::EditorFiles::default_location().dir;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("editor.log");
    if std::fs::metadata(&path).is_ok_and(|meta| meta.len() > LOG_ROLL_BYTES) {
        let _ = std::fs::rename(&path, dir.join("editor.log.1"));
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
}

fn main() {
    use tracing_subscriber::fmt::writer::MakeWriterExt as _;
    let file = log_file().map(std::sync::Mutex::new);
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "info,gpui=warn".into())
    };
    match file {
        Some(file) => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_writer(std::io::stderr.and(file))
            .with_ansi(false)
            .init(),
        None => tracing_subscriber::fmt()
            .with_env_filter(filter())
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .init(),
    }
    // `artisan-editor artisan://thread/<host>/<thread>` opens that screen.
    let link = std::env::args()
        .skip(1)
        .find_map(|argument| Screen::from_link(&argument));

    gpui_platform::application()
        .with_assets(editor_ui::asset_seam::CatalogAssetSource)
        .run(move |cx: &mut App| {
            if let Err(error) = editor_ui::fonts::register_bundled_fonts(cx) {
                tracing::warn!("using system fonts; the bundled ones didn't load: {error}");
            }
            bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let (core, failure) = editor_core::init(cx);
            let failure = failure
                .map(|error| format!("Couldn't read this device's Artisan files: {error:#}"));
            if let Some(screen) = link.clone() {
                core.update(cx, |core, cx| core.navigate(screen, cx));
            }
            cx.set_app_identity("com.artisanstreet.editor", "Artisan");
            let notified_core = core.downgrade();
            cx.on_system_notification_response(move |response, cx| {
                let (Some((host, thread)), Some(core)) = (
                    notifications::parse_tag(&response.tag),
                    notified_core.upgrade(),
                ) else {
                    return;
                };
                core.update(cx, |core, cx| {
                    core.navigate(Screen::Thread { host, thread }, cx)
                });
                if let Some(window) = cx.windows().first() {
                    let _ = window.update(cx, |_, window, _| window.activate_window());
                }
                cx.activate(true);
            });

            let launch = size(px(1440.0), px(900.0));
            let launch = cx.primary_display().map_or(launch, |display| {
                let visible = display.visible_bounds().size;
                size(
                    launch.width.min(visible.width),
                    launch.height.min(visible.height),
                )
            });
            #[allow(unused_mut)]
            let mut options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, launch, cx))),
                window_min_size: Some(size(px(720.0), px(480.0))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Artisan".into()),
                    appears_transparent: true,
                    ..TitlebarOptions::default()
                }),
                inactive_frame_interval: None,
                ..WindowOptions::default()
            };
            #[cfg(feature = "capture")]
            let scripted = script::path();
            #[cfg(feature = "capture")]
            if scripted.is_some() {
                script::window_options(&mut options);
            }
            let opened = cx.open_window(options, move |window, cx| {
                cx.new(|cx| Workspace::new(core, failure, window, cx))
            });
            match opened {
                Ok(handle) => {
                    #[cfg(feature = "capture")]
                    if let Some(path) = scripted {
                        script::run(path, handle, cx);
                        return;
                    }
                    #[cfg(feature = "capture")]
                    capture(handle, cx);
                    #[cfg(not(feature = "capture"))]
                    let _ = handle;
                    cx.activate(true);
                }
                Err(error) => {
                    tracing::error!("couldn't open the window: {error:#}");
                    cx.quit();
                }
            }
        });
}

fn bind_keys(cx: &mut App) {
    editor_ui::text_editor::TextEditor::bind_actions(cx);
    let context = Some(workspace::KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-k", OpenPalette, context),
        KeyBinding::new("cmd-k", OpenPalette, context),
        KeyBinding::new("ctrl-b", ToggleSidebar, context),
        KeyBinding::new("cmd-b", ToggleSidebar, context),
        KeyBinding::new("ctrl-alt-b", ToggleInspector, context),
        KeyBinding::new("cmd-alt-b", ToggleInspector, context),
        KeyBinding::new("alt-left", GoBack, context),
        KeyBinding::new("ctrl-,", OpenSettings, context),
        KeyBinding::new("cmd-,", OpenSettings, context),
        KeyBinding::new("ctrl-n", NewThread, context),
        KeyBinding::new("cmd-n", NewThread, context),
    ]);
    palette::bind_keys(cx);
}

#[cfg(feature = "capture")]
fn capture(handle: gpui::WindowHandle<Workspace>, cx: &mut App) {
    let Some(path) = std::env::var_os("ARTISAN_EDITOR_CAPTURE") else {
        return;
    };
    let delay = std::env::var("ARTISAN_EDITOR_CAPTURE_DELAY")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12);
    tracing::info!(delay, "capturing the window soon");
    cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(std::time::Duration::from_secs(delay))
            .await;
        tracing::info!("capturing the window");
        let saved = handle.update(cx, |_, window, _| {
            window
                .render_to_image()
                .and_then(|image| Ok(image.save(&path)?))
        });
        match saved {
            Ok(Ok(())) => tracing::info!(
                "saved the window to {}",
                std::path::Path::new(&path).display()
            ),
            Ok(Err(error)) => tracing::error!("couldn't capture the window: {error:#}"),
            Err(error) => tracing::error!("couldn't capture the window: {error:#}"),
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}
