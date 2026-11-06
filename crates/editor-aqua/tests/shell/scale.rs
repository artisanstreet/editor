use aqua::prelude::{
    AquaWindowId, InputEvent, ObservedWindowState, WindowEvent, WindowOptions,
};
use aqua::{AppCtx, Aqua, AquaConfig, UiApp};
use aqua_ui::element::{Box as UiBox, Element, Text};
use editor_aqua::core::{Core, EditorFiles};
use editor_aqua::shell::{
    application::{SectionViews, ShellApplication},
    host::{Shell, ShellResources},
};
use std::{cell::RefCell, rc::Rc};

struct Probe {
    scale_was_already_applied: Rc<RefCell<Vec<(f64, bool)>>>,
}

impl SectionViews for Probe {
    fn build(
        &mut self,
        _: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> anyhow::Result<Element> {
        let changed = ui
            .tree_mut()
            .set_gpui_layout_scale(Some(window.scale_factor as f32));
        self.scale_was_already_applied
            .borrow_mut()
            .push((window.scale_factor, !changed));
        Ok(UiBox::new().child(Text::new("scale").build()).build())
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
