use crate::conversation::presentation::PresentationScope;
use aqua_core::Rect;
use artisan_api::{Item, Seq};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct NativeState {
    pub completed_frame: u64,
    pub page_loaded: bool,
    pub page_in_flight: bool,
    pub load_error: Option<String>,
    pub row_count: usize,
    pub measured_row_count: usize,
    pub landing: bool,
}

impl NativeState {
    pub fn observe(shell: &crate::shell::host::Shell, completed_frame: u64) -> Option<Self> {
        let conversation = &shell.entry.workspace()?.conversation;
        let view = conversation.view.as_ref();
        Some(Self {
            completed_frame,
            page_loaded: view.is_some_and(|view| view.viewport.loaded()),
            page_in_flight: view.is_some_and(|view| view.viewport.in_flight),
            load_error: conversation.load_error.clone(),
            row_count: conversation.rows.len(),
            measured_row_count: conversation.row_heights.len(),
            landing: conversation.landing.is_some(),
        })
    }
}

pub fn record_native_state(
    shell: &crate::shell::host::Shell,
    completed_frame: u64,
) -> anyhow::Result<()> {
    let Some(path) = std::env::var_os("ARTISAN_AQUA_QUALIFICATION_STATE") else {
        return Ok(());
    };
    let state = NativeState::observe(shell, completed_frame);
    std::fs::write(path, serde_json::to_vec(&state)?)?;
    Ok(())
}

#[derive(Clone, Debug, Serialize)]
pub struct Observation {
    pub account: String,
    pub host: String,
    pub thread: String,
    pub agent: Option<String>,
    pub items: Vec<Item>,
    pub page_loaded: bool,
    pub page_in_flight: bool,
    pub page_exhausted: bool,
    pub page_oldest: Option<Seq>,
    pub page_gap: Option<Seq>,
    pub load_error: Option<String>,
    pub landing: bool,
    pub work_in_flight: bool,
    pub row_count: usize,
    pub measured_row_count: usize,
    pub prepared_inline_prose: Vec<String>,
    pub visible_row_start: Option<usize>,
    pub scroll_y_bits: Option<u32>,
    pub transcript_bounds_bits: Option<[u32; 4]>,
    pub composer_bounds_bits: Option<[u32; 4]>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PaintedRow {
    pub key: String,
    pub item: Option<String>,
    pub item_seq: Option<Seq>,
    pub observed_seq: Option<Seq>,
    pub bounds_bits: [u32; 4],
    pub clip_bits: [u32; 4],
    pub revealed: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Paint {
    pub generation: u64,
    pub completed_frame: Option<u64>,
    pub host: String,
    pub thread: String,
    pub agent: Option<String>,
    pub rows: Vec<PaintedRow>,
}

#[derive(Default)]
pub struct PaintCollector {
    armed: Option<(PresentationScope, u64, Paint)>,
}

impl PaintCollector {
    pub fn arm(
        &mut self,
        scope: Option<PresentationScope>,
        generation: u64,
        after_serial: u64,
    ) -> bool {
        self.armed = scope.map(|scope| {
            let paint = Paint {
                generation,
                completed_frame: None,
                host: scope.host.0.clone(),
                thread: scope.thread.to_string(),
                agent: scope.agent.as_ref().map(ToString::to_string),
                rows: Vec::new(),
            };
            (scope, after_serial, paint)
        });
        self.armed.is_some()
    }

    pub fn begin_frame(&mut self, scope: &PresentationScope, frame: u64) {
        match &mut self.armed {
            Some((armed, _, paint)) if armed == scope => {
                paint.completed_frame = Some(frame);
                paint.rows.clear();
            }
            _ => self.armed = None,
        }
    }

    pub fn record(&mut self, serial: u64, row: PaintedRow) {
        if let Some((_, after_serial, paint)) = &mut self.armed
            && paint.completed_frame.is_some()
            && serial > *after_serial
        {
            paint.rows.push(row);
        }
    }

    pub fn current(&self, scope: &PresentationScope) -> Option<Paint> {
        self.armed
            .as_ref()
            .filter(|(armed, _, _)| armed == scope)
            .map(|(_, _, paint)| paint.clone())
    }
}

pub fn rect_bits(bounds: Rect) -> [u32; 4] {
    [
        bounds.x.to_bits(),
        bounds.y.to_bits(),
        bounds.w.to_bits(),
        bounds.h.to_bits(),
    ]
}
