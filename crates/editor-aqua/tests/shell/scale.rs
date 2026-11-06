use aqua::prelude::{AquaWindowId, InputEvent, ObservedWindowState, WindowEvent, WindowOptions};
use aqua::{AppCtx, Application, Aqua, AquaConfig, UiApp};
use aqua_ui::element::{Box as UiBox, Element, Text};
use editor_aqua::core::{Core, EditorFiles};
use editor_aqua::shell::{
    application::{SectionViews, ShellApplication},
    host::{Shell, ShellResources},
};
use std::{cell::RefCell, rc::Rc};

struct Probe {
    scale_was_already_applied: Rc<RefCell<Vec<(f64, bool)>>>,
    runtime: Vec<(f32, aqua_core::Size)>,
}

impl SectionViews for Probe {
    fn build(
        &mut self,
        _: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> anyhow::Result<Element> {
        self.runtime.push((ui.scale(), ui.viewport()));
        let changed = ui
            .tree_mut()
            .set_gpui_layout_scale(Some(window.scale_factor as f32));
        self.scale_was_already_applied
            .borrow_mut()
            .push((window.scale_factor, !changed));
        Ok(UiBox::new()
            .child(Text::new("scale").width(80.0).height(40.0).build())
            .build())
    }

    fn input(
        &mut self,
        _: &mut Shell,
        _: &mut UiApp,
        _: &mut AppCtx<'_>,
        _: AquaWindowId,
        _: &InputEvent,
        _: bool,
    ) -> bool {
        false
    }

    fn wake(&mut self, _: &mut Shell, _: &mut UiApp, _: &mut AppCtx<'_>, _: AquaWindowId) -> bool {
        false
    }
}

#[test]
fn scale_change_reaches_layout_before_the_rebuild() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let seen = Rc::new(RefCell::new(Vec::new()));
    let views = Probe {
        scale_was_already_applied: seen.clone(),
        runtime: Vec::new(),
    };
    let mut app = ShellApplication::new(files.clone(), ShellResources::new(1.0).unwrap(), views);
    let mut core = Core::for_tests(files).unwrap();
    core.set_image_assets(app.shell.images().clone());
    app.shell.core = core;
    app.shell.entry.sync(&app.shell.core);
    let mut aqua = Aqua::new(AquaConfig {
        headless: true,
        ..AquaConfig::default()
    })
    .unwrap();
    let id = AquaWindowId::new(3).unwrap();
    let size = WindowOptions::default().inner_size;
    aqua.add_headless_window(ObservedWindowState::new(id, "test".into(), size));
    aqua.dispatch_start(&mut app, id);
    seen.borrow_mut().clear();
    aqua.dispatch_window_event(&mut app, id, &WindowEvent::ScaleChanged(id, 1.75));
    let seen = seen.borrow();
    assert!(!seen.is_empty(), "scale change did not rebuild");
    assert_eq!(seen.last(), Some(&(1.75, true)));
}

#[test]
fn startup_scale_drives_physical_accessibility_bounds_through_resize_and_dpi_change() {
    for initial_scale in [1.25_f64, 2.0] {
        let directory = tempfile::tempdir().unwrap();
        let files = EditorFiles {
            dir: directory.path().to_owned(),
        };
        let views = Probe {
            scale_was_already_applied: Rc::default(),
            runtime: Vec::new(),
        };
        let mut app =
            ShellApplication::new(files.clone(), ShellResources::new(1.0).unwrap(), views);
        let mut core = Core::for_tests(files).unwrap();
        core.set_image_assets(app.shell.images().clone());
        app.shell.core = core;
        app.shell.entry.sync(&app.shell.core);
        let mut aqua = Aqua::new(AquaConfig {
            headless: true,
            ..AquaConfig::default()
        })
        .unwrap();
        let id = AquaWindowId::new(4).unwrap();
        let mut size = WindowOptions::default().inner_size;
        size.width = 800.0;
        size.height = 600.0;
        let mut observed = ObservedWindowState::new(id, "scaled".into(), size);
        observed.scale_factor = initial_scale;
        aqua.add_headless_window(observed.clone());
        aqua.dispatch_start(&mut app, id);
        assert!(
            !app.views.runtime.is_empty(),
            "startup must build the probe view"
        );
        assert!(
            app.views.runtime.iter().all(|(scale, viewport)| {
                *scale == initial_scale as f32 && *viewport == aqua_core::Size::new(800.0, 600.0)
            }),
            "the observed viewport and scale must be installed before the first build"
        );
        let bounds =
            |app: &mut ShellApplication<Probe>, aqua: &Aqua, observed: &ObservedWindowState| {
                app.build_scene(observed, aqua.clock());
                app.a11y_nodes_for(id)
                    .unwrap()
                    .2
                    .into_iter()
                    .find(|node| node.semantic.label == "scale")
                    .expect("the probe text must expose accessibility bounds")
                    .bounds
                    .unwrap()
            };
        let initial = bounds(&mut app, &aqua, &observed);
        assert_eq!(
            (initial.w, initial.h),
            (80.0 * initial_scale as f32, 40.0 * initial_scale as f32),
            "initial accessibility bounds must use physical pixels"
        );

        size.width = 960.0;
        size.height = 720.0;
        observed.inner_size = size;
        aqua.dispatch_window_event(&mut app, id, &WindowEvent::Resized(id, size));
        let resized = bounds(&mut app, &aqua, &observed);
        assert_eq!(
            (resized.w, resized.h),
            (initial.w, initial.h),
            "ordinary resize must preserve the startup DPI scale"
        );
        assert_eq!(
            app.views.runtime.last(),
            Some(&(initial_scale as f32, aqua_core::Size::new(960.0, 720.0))),
            "ordinary resize must update the logical viewport while retaining scale"
        );

        observed.scale_factor = if initial_scale == 1.25 { 2.0 } else { 1.25 };
        aqua.dispatch_window_event(
            &mut app,
            id,
            &WindowEvent::ScaleChanged(id, observed.scale_factor),
        );
        let changed = bounds(&mut app, &aqua, &observed);
        assert_eq!(
            (changed.w, changed.h),
            (
                80.0 * observed.scale_factor as f32,
                40.0 * observed.scale_factor as f32
            ),
            "a DPI transition must update physical accessibility bounds"
        );
        assert_eq!(
            app.views.runtime.last(),
            Some(&(
                observed.scale_factor as f32,
                aqua_core::Size::new(960.0, 720.0)
            )),
            "a DPI transition must preserve the logical viewport"
        );
    }
}
