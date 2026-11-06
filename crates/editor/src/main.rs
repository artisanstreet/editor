#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

#[cfg(not(windows))]
mod context_meter;
#[cfg(not(windows))]
mod entry;
#[cfg(feature = "dhat-heap")]
#[cfg(not(windows))]
mod heap_profile;
#[cfg(not(windows))]
mod home;
#[cfg(not(windows))]
mod logging;
#[cfg(not(windows))]
mod notifications;
#[cfg(not(windows))]
mod palette;
#[cfg(not(windows))]
mod repository_context;
#[cfg(not(windows))]
mod resource_preview;
#[cfg(feature = "capture")]
#[cfg(not(windows))]
mod script;
#[cfg(not(windows))]
mod workspace;

#[cfg(not(windows))]
use actions::{GoBack, NewThread, OpenPalette, OpenSettings, Quit, ToggleInspector, ToggleSidebar};
#[cfg(not(windows))]
use editor_core::{Core, Screen};
#[cfg(not(windows))]
use entry::EntryRoot;
#[cfg(not(windows))]
use gpui::{
    App, AppContext as _, Bounds, Entity, KeyBinding, Pixels, Size, TitlebarOptions, WindowBounds,
    WindowOptions, px, size,
};

#[cfg(not(windows))]
mod actions {
    #![expect(
        clippy::derive_partial_eq_without_eq,
        reason = "gpui's actions! macro chooses the derives"
    )]
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
}

#[cfg(not(windows))]
const LAUNCH_WINDOW_HEIGHT: f32 = 900.0;
#[cfg(not(windows))]
const MIN_WINDOW_WIDTH: f32 = 480.0;
#[cfg(not(windows))]
const MIN_WINDOW_HEIGHT: f32 = 420.0;

#[cfg(not(windows))]
fn launch_window_geometry(cx: &App) -> (Bounds<Pixels>, Size<Pixels>) {
    let min_size = size(px(MIN_WINDOW_WIDTH), px(MIN_WINDOW_HEIGHT));
    let mut launch = size(
        px(2.0f32.mul_add(workspace::COLUMN_WIDTH, workspace::CHAT_MIN_WIDTH)),
        px(LAUNCH_WINDOW_HEIGHT),
    );
    if let Some(display) = cx.primary_display() {
        let visible = display.visible_bounds().size;
        launch.width = launch.width.min(visible.width).max(min_size.width);
        launch.height = launch.height.min(visible.height).max(min_size.height);
    }
    (Bounds::centered(None, launch, cx), min_size)
}

#[cfg(not(windows))]
fn init_logging() {
    use tracing_subscriber::fmt::writer::MakeWriterExt as _;
    let file = logging::file().map(std::sync::Mutex::new);
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            "info,gpui=warn,gpui_ce_windows::directx_devices=info,gpui_ce_windows::platform=info"
                .into()
        })
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
}

#[cfg(not(windows))]
fn follow_notification(
    core: &Entity<Core>,
    response: &gpui::SystemNotificationResponse,
    cx: &mut App,
) {
    let Some((host, thread)) = notifications::parse_tag(&response.tag) else {
        return;
    };
    core.update(cx, |core, cx| {
        core.navigate(Screen::Thread { host, thread }, cx);
    });
}

#[cfg(not(windows))]
fn main() {
    init_logging();
    #[cfg(feature = "dhat-heap")]
    heap_profile::start();
    let link = std::env::args()
        .skip(1)
        .find_map(|argument| Screen::from_link(&argument));

    gpui_platform::application()
        .with_assets(editor_ui::asset_seam::CatalogAssetSource)
        .run(move |cx: &mut App| {
            cx.set_http_client(editor_core::media_http::client());
            if let Err(error) = editor_ui::fonts::register_bundled_fonts(cx) {
                tracing::warn!("using system fonts; the bundled ones didn't load: {error}");
            }
            editor_ui::theme::install(editor_ui::theme::ThemeMode::Dark, cx);
            editor_sources::init(cx);
            bind_keys(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            #[cfg(feature = "dhat-heap")]
            cx.on_app_quit(|_| {
                heap_profile::finish();
                async {}
            })
            .detach();
            cx.on_window_closed(|cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();

            let (core, failure) = editor_core::init(cx);
            let failure = failure
                .map(|error| format!("Couldn't read this device's Artisan files: {error:#}"));
            if let Some(screen) = link {
                core.update(cx, |core, cx| core.navigate(screen, cx));
            }
            cx.set_app_identity("com.artisanstreet.editor", "Artisan Editor");
            let notified_core = core.downgrade();
            cx.on_system_notification_response(move |response, cx| {
                if let Some(core) = notified_core.upgrade() {
                    follow_notification(&core, &response, cx);
                }
                if let Some(window) = cx.windows().first() {
                    window
                        .update(cx, |_, window, _| window.activate_window())
                        .ok();
                }
                cx.activate(true);
            });

            let (launch, min_size) = launch_window_geometry(cx);
            #[cfg_attr(
                not(feature = "capture"),
                expect(unused_mut, reason = "only the capture feature adjusts the options")
            )]
            let mut options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(launch)),
                window_min_size: Some(min_size),
                titlebar: Some(TitlebarOptions {
                    title: Some("Artisan Editor".into()),
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
                cx.new(|cx| EntryRoot::new(core, failure, window, cx))
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
    #[cfg(feature = "dhat-heap")]
    heap_profile::finish();
}

#[cfg(not(windows))]
fn bind_keys(cx: &mut App) {
    editor_ui::focus_navigation::bind_actions(cx);
    editor_ui::text_editor::TextEditor::bind_actions(cx);
    editor_ui::selectable_text::bind_copy(cx);
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
#[cfg(not(windows))]
fn capture(handle: gpui::WindowHandle<EntryRoot>, cx: &mut App) {
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

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    editor_aqua::launch()
}
