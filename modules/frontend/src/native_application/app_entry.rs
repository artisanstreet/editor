//! Application entry, window launch, and shutdown plumbing for the native
//! [`run`] boundary.
//!
//! Extracted verbatim from `native_application.rs` during the phase-5 module
//! split; the test-facing action binding was widened to `pub(super)`.

use super::*;

pub(super) fn bind_native_actions(cx: &mut App) {
    NativeComposer::bind_actions(cx);
    NativeCommandMenu::bind_actions(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("ctrl-shift-f12", ToggleFrameCounter, None),
        KeyBinding::new("cmd-k", OpenCommandMenu, Some(NATIVE_KEY_CONTEXT)),
        KeyBinding::new("ctrl-k", OpenCommandMenu, Some(NATIVE_KEY_CONTEXT)),
        KeyBinding::new("tab", NextTabStop, Some(NATIVE_KEY_CONTEXT)),
        KeyBinding::new("shift-tab", PreviousTabStop, Some(NATIVE_KEY_CONTEXT)),
    ]);
}

fn request_app_shutdown(
    cx: &mut App,
    service: Option<Arc<NativeTransportService>>,
    shutdown_started: &Arc<AtomicBool>,
) {
    if shutdown_started.swap(true, Ordering::AcqRel) {
        return;
    }
    let task = cx.spawn(async move |cx| {
        if let Some(service) = service {
            let timer_executor = cx.background_executor().clone();
            loop {
                if service.is_finished() {
                    let _ = service.join();
                    break;
                }
                let _ = service.request_shutdown();
                while matches!(service.try_recv(), Ok(Some(_))) {}
                timer_executor.timer(POLL_INTERVAL).await;
            }
        }
        let () = cx.update(|cx| cx.quit());
    });
    task.detach();
}

fn prepare_application_shutdown(
    view: &Rc<RefCell<Option<Entity<NativeApplication>>>>,
    cx: &mut App,
) {
    let view = view.borrow().clone();
    if let Some(view) = view {
        view.update(cx, NativeApplication::prepare_shutdown);
    }
}

/// Logs which GPU renderer backs the opened window.
///
/// Compile-time half of the renderer guard: `Window::gpu_context` only
/// exists when the `gpui/wgpu-surfaces` feature rides the workspace
/// `gpui_platform/wgpu` chain, so dropping that feature fails the build here
/// instead of silently running the DirectX backend. The runtime half is the
/// `eprintln!` below plus `gpui_wgpu`'s own `Selected GPU adapter` log line.
#[cfg(target_os = "windows")]
fn report_renderer(window: &mut Window) {
    let wgpu_active = window.gpu_context().is_some();
    let device = window
        .gpu_specs()
        .map_or_else(|| String::from("<unknown>"), |specs| specs.device_name);
    eprintln!("artisan editor renderer: wgpu active = {wgpu_active}, device = {device}");
}

/// Non-Windows builds have no wgpu-shipping contract to guard.
#[cfg(not(target_os = "windows"))]
fn report_renderer(_window: &mut Window) {}

/// Launches the real native application window.
#[must_use]
pub fn run() -> ExitCode {
    let service = NativeTransportService::spawn().ok().map(Arc::new);
    let shutdown_started = Arc::new(AtomicBool::new(false));
    let launched = Rc::new(Cell::new(false));
    let launch_flag = Rc::clone(&launched);
    let application_view = Rc::new(RefCell::new(None));

    gpui_platform::application()
        .with_assets(artisan_ui::asset_seam::CatalogAssetSource)
        .run(move |cx: &mut App| {
            // Register the vendored legacy typefaces before any window opens;
            // on failure keep running on system faces (typed, not swallowed).
            if let Err(error) = artisan_ui::fonts::register_bundled_fonts(cx) {
                eprintln!("bundled font registration failed, using system faces: {error}");
            }

            bind_native_actions(cx);

            let service_for_action = service.clone();
            let shutdown_for_action = Arc::clone(&shutdown_started);
            let view_for_action = Rc::clone(&application_view);
            cx.on_action(move |_: &Quit, cx| {
                prepare_application_shutdown(&view_for_action, cx);
                request_app_shutdown(cx, service_for_action.clone(), &shutdown_for_action);
            });

            let service_for_close = service.clone();
            let shutdown_for_close = Arc::clone(&shutdown_started);
            let view_for_close = Rc::clone(&application_view);
            cx.on_window_closed(move |cx, _window_id| {
                if cx.windows().is_empty() {
                    prepare_application_shutdown(&view_for_close, cx);
                    request_app_shutdown(cx, service_for_close.clone(), &shutdown_for_close);
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT)), cx);
            let service_for_view = service.clone();
            let view_for_registration = Rc::clone(&application_view);
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(WINDOW_TITLE.into()),
                        // CE keeps native resizing; desktop_shell supplies caption hit areas.
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    crate::native_frame_rate::initialize(window, cx);
                    let view =
                        cx.new(|view_cx| NativeApplication::new(service_for_view, window, view_cx));
                    view_for_registration.borrow_mut().replace(view.clone());
                    view.update(cx, NativeApplication::start_polling);
                    window.set_debug_frame_overlay_mode(gpui::DebugFrameOverlayMode::FrameRate);
                    frame_capture::start(window);
                    view
                },
            );

            if opened.is_ok() {
                launch_flag.set(true);
                if let Ok(handle) = &opened {
                    let _ = handle.update(cx, |_, window, _| report_renderer(window));
                }
                cx.activate(true);
            } else {
                request_app_shutdown(cx, service.clone(), &shutdown_started);
            }
        });

    if launched.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
