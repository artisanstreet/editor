use super::install;
use super::model::{Effect, Event, Kind, Phase, Setup};
use super::view::{self, Props};
use crate::shell::host::{ShellResources, set_window_scale};
use crate::ui::theme::ArtisanTheme;
use aqua::images::ManagedUiApp;
use aqua::mailbox::{Mailbox, Sender};
use aqua::prelude::{
    A11yNode, AquaWindowId, CursorIcon, DragRegions, FrameReason, InputEvent, Instant, NodeId,
    ObservedWindowState, Scene, WindowEvent,
};
use aqua::{AppCtx, Application, ExternalFrame};
use aqua_protocol::{DialogKind, DialogOutcome, DialogSpec, RequestId};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::time::Duration;

const SPIN_INTERVAL: Duration = Duration::from_millis(33);
const MAILBOX: NonZeroUsize = NonZeroUsize::new(256).expect("a nonzero capacity");

struct SetupApplication {
    ui: ManagedUiApp,
    theme: ArtisanTheme,
    setup: Setup,
    spin: u32,
    mounted: bool,
    choices: Rc<RefCell<VecDeque<Event>>>,
    work: Mailbox<Event>,
    folder_request: Option<RequestId>,
}

pub fn run(setup: Setup) -> anyhow::Result<()> {
    let mut config = aqua::AquaConfig::default();
    config.window.title = "Artisan Editor Setup".to_owned();
    config.window.inner_size.width = view::WINDOW.0;
    config.window.inner_size.height = view::WINDOW.1;
    config.window.resizable = false;
    config.window.maximizable = false;
    let runtime = aqua::Aqua::new(config).map_err(anyhow::Error::msg)?;
    let resources = ShellResources::for_runtime(&runtime, 1.0)
        .ok_or_else(|| anyhow::anyhow!("could not configure the setup window"))?
        .with_color_profile(aqua_core::ColorProfile::GpuiWindowsEncoded);
    runtime
        .run(SetupApplication {
            ui: resources.into_managed(),
            theme: ArtisanTheme::resolved(),
            setup,
            spin: 0,
            mounted: false,
            choices: Rc::default(),
            work: Mailbox::new(MAILBOX),
            folder_request: None,
        })
        .map_err(anyhow::Error::msg)
}

fn find_distros(sender: Sender<Event>) {
    std::thread::spawn(move || {
        let found = crate::core::runtime::runtime().block_on(crate::core::hosts::wsl_state());
        let distros = found
            .map(|distros| distros.into_iter().map(|distro| distro.name).collect())
            .unwrap_or_default();
        let _closed = sender.send(Event::DistrosFound(distros));
    });
}

fn install(arguments: Vec<String>, sender: Sender<Event>) {
    std::thread::spawn(move || {
        let progress = sender.clone();
        let result = install::run(&arguments, |line| {
            let _closed = progress.send(Event::Progressed(line.to_owned()));
        });
        let _closed = sender.send(Event::Finished(
            result.map_err(|error| format!("{error:#}")),
        ));
    });
}

impl SetupApplication {
    fn perform(&mut self, effect: Effect, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        match effect {
            Effect::PickFolder => {
                self.folder_request = Some(ctx.dialog(
                    id,
                    DialogSpec {
                        kind: DialogKind::PickFolder,
                        title: "Choose where to install Artisan Editor".to_owned(),
                        filters: Vec::new(),
                    },
                ));
            }
            Effect::Install(arguments) => install(arguments, self.work.sender()),
            Effect::Quit => ctx.quit(),
        }
    }

    fn drain(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        let mut events = self.work.drain();
        events.extend(self.choices.borrow_mut().drain(..));
        let changed = !events.is_empty();
        for event in events {
            if let Some(effect) = self.setup.apply(event) {
                self.perform(effect, ctx, id);
            }
        }
        if changed || !self.mounted {
            self.rebuild(ctx, id);
        }
    }

    fn rebuild(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        let choices = Rc::clone(&self.choices);
        let element = view::screen(
            &self.theme,
            &Props {
                setup: &self.setup,
                spin: self.spin,
                emit: Rc::new(move |event| choices.borrow_mut().push_back(event)),
            },
        );
        let result = if self.mounted {
            self.ui.ui_mut().refresh(vec![element])
        } else {
            self.ui.ui_mut().mount(|_| element)
        };
        match result {
            Ok(()) => self.mounted = true,
            Err(error) => tracing::error!(%error, "could not show setup"),
        }
        ctx.request_frame(id, FrameReason::DAMAGE);
        if matches!(self.setup.phase, Phase::Installing { .. }) {
            ctx.request_animation_deadline(id, ctx.now() + SPIN_INTERVAL);
        }
    }
}

impl Application for SetupApplication {
    fn on_external_file_drag(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        event: &aqua::ExternalFileDrag,
    ) {
        self.ui.on_external_file_drag(ctx, id, event);
    }
    fn input_invalidation_epoch(&self, id: AquaWindowId) -> u64 {
        self.ui.ui().input_invalidation_epoch(id)
    }
    fn color_profile(&self) -> aqua_core::ColorProfile {
        self.ui.color_profile()
    }
    fn color_profile_for(&self, id: AquaWindowId) -> aqua_core::ColorProfile {
        self.ui.color_profile_for(id)
    }
    fn on_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        self.ui.on_click_settings(settings);
    }
    fn on_caption_hover(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        control: Option<aqua_protocol::CaptionControl>,
    ) {
        self.ui.on_caption_hover(ctx, id, control);
    }
    fn on_start(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        self.ui.on_start(ctx, id);
        if let Some(window) = ctx.window_state(id) {
            let ui = self.ui.ui_mut();
            ui.set_viewport(
                window.inner_size.width as f32,
                window.inner_size.height as f32,
                window.scale_factor as f32,
            );
            set_window_scale(ui, window.scale_factor as f32);
        }
        ctx.bind_mailbox(id, &self.work);
        match self.setup.kind {
            Kind::Fresh => find_distros(self.work.sender()),
            Kind::Update { .. } => self.choices.borrow_mut().push_back(Event::Started),
        }
        self.drain(ctx, id);
    }
    fn can_close(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) -> bool {
        !matches!(self.setup.phase, Phase::Installing { .. }) && self.ui.can_close(ctx, id)
    }
    fn on_wake(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        self.ui.on_wake(ctx, id);
        self.drain(ctx, id);
    }
    fn on_frame_completed(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, sequence: u64) {
        self.ui.on_frame_completed(ctx, id, sequence);
    }
    fn on_animation_frame(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, now: Instant) {
        self.ui.on_animation_frame(ctx, id, now);
        if matches!(self.setup.phase, Phase::Installing { .. }) {
            self.spin = (self.spin + 1) % view::SPIN_STEPS;
            self.rebuild(ctx, id);
        }
    }
    fn on_dialog(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        request: RequestId,
        outcome: DialogOutcome,
    ) {
        if self.folder_request != Some(request) {
            self.ui.on_dialog(ctx, id, request, outcome);
            return;
        }
        self.folder_request = None;
        if let Ok(Some(folder)) = crate::ui::dialogs::chosen_path(outcome) {
            self.choices.borrow_mut().push_back(Event::Located(folder));
        }
        self.drain(ctx, id);
    }
    fn on_motion_event(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        event: &aqua_protocol::MotionEvent,
    ) {
        self.ui.on_motion_event(ctx, id, event);
    }
    fn step_input(&mut self, id: AquaWindowId, input: aqua::RuntimeInput) {
        self.ui.step_input(id, input);
    }
    fn take_output(&mut self, id: AquaWindowId) -> aqua::RuntimeOutput {
        self.ui.take_output(id)
    }
    fn motion_commands(&mut self, id: AquaWindowId) -> Vec<aqua_protocol::MotionCommand> {
        self.ui.motion_commands(id)
    }
    fn cursor(&self) -> Option<CursorIcon> {
        Some(self.ui.ui().cursor())
    }
    fn cursor_for(&self, id: AquaWindowId) -> Option<CursorIcon> {
        self.ui.cursor_for(id)
    }
    fn on_window_event(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, event: &WindowEvent) {
        self.ui.on_window_event(ctx, id, event);
        if let WindowEvent::ScaleChanged(_, scale) = event {
            set_window_scale(self.ui.ui_mut(), *scale as f32);
            self.rebuild(ctx, id);
        }
    }
    fn on_input(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, input: &InputEvent) {
        self.ui.on_input(ctx, id, input);
        self.drain(ctx, id);
    }
    fn on_a11y_action(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        request: &aqua_protocol::A11yActionRequest,
    ) {
        self.ui.on_a11y_action(ctx, id, request);
        self.drain(ctx, id);
    }
    fn prepare_accessibility(
        &mut self,
        window: &ObservedWindowState,
        now: Instant,
    ) -> Option<Instant> {
        self.ui.prepare_accessibility(window, now)
    }
    fn a11y_nodes(&self) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        self.ui.a11y_nodes()
    }
    fn a11y_nodes_for(&self, id: AquaWindowId) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        self.ui.a11y_nodes_for(id)
    }
    fn chrome_regions(&self, window: &ObservedWindowState) -> Option<DragRegions> {
        self.ui.chrome_regions(window)
    }
    fn externals(&mut self, frame: &mut ExternalFrame<'_>) {
        self.ui.externals(frame);
    }
    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        self.ui.build_scene(window, now)
    }
    fn scene_mut(&mut self, id: AquaWindowId) -> Option<&mut Scene> {
        self.ui.scene_mut(id)
    }
    fn reactive_runtime(&self) -> Option<&aqua_reactive::Runtime> {
        self.ui.reactive_runtime()
    }
    fn reactive_runtime_for(&self, id: AquaWindowId) -> Option<&aqua_reactive::Runtime> {
        self.ui.reactive_runtime_for(id)
    }
    fn on_render_error(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, message: &str) {
        self.ui.on_render_error(ctx, id, message);
    }
}
