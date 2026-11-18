use aqua::render::host::FrameTarget;
use aqua::render::link::{InitFactory, RenderContext};
use aqua_protocol::{Feedback, FrameId};
use aqua_runtime::RenderSide;

struct Presenter {
    _context: RenderContext,
}

impl FrameTarget for Presenter {
    fn wait_frame(&mut self) {}

    fn present(&mut self, _: &mut RenderSide, frame: FrameId) -> Vec<Feedback> {
        vec![Feedback::Presented(frame)]
    }
}

pub(super) fn presenter() -> InitFactory {
    Box::new(|context| {
        Box::new(move || {
            let presenter: Box<dyn FrameTarget> = Box::new(Presenter { _context: context });
            Ok(presenter)
        })
    })
}
