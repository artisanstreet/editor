//! Byte-level escape-sequence state machine.
//!
//! A small subset of the DEC/ECMA-48 parser model that `vte` implements:
//! ground text decodes as UTF-8, and `ESC`, CSI, OSC and the DCS/SOS/PM/APC
//! string family are recognised. Every state survives a `feed` boundary, so
//! input split anywhere parses the same as input that arrives whole. Buffers
//! are capped and malformed input resolves to U+FFFD or is dropped; nothing
//! panics.

/// Escape: starts every sequence and aborts an unfinished one.
const ESC: u8 = 0x1b;
/// Bell: the xterm OSC terminator.
const BEL: u8 = 0x07;
/// Cancel: aborts a sequence.
const CAN: u8 = 0x18;
/// Substitute: aborts a sequence.
const SUB: u8 = 0x1a;
/// Longest CSI parameter/intermediate run kept; longer sequences are dropped.
const MAX_SEQUENCE_BYTES: usize = 256;
/// Longest OSC payload kept; longer payloads are dropped whole.
const MAX_OSC_BYTES: usize = 4_096;
/// Ceiling for one numeric parameter, so arithmetic never overflows.
const MAX_PARAM: u32 = 65_535;

/// Receives the actions the parser recognises.
pub(super) trait Perform {
    /// A printable character.
    fn print(&mut self, ch: char);
    /// A C0 control byte (or DEL) outside any string sequence.
    fn execute(&mut self, byte: u8);
    /// A complete public CSI sequence; private (`?`, `>`…) and intermediate
    /// forms never arrive here.
    fn csi(&mut self, params: &Params, final_byte: u8);
    /// A two-byte `ESC x` sequence.
    fn esc(&mut self, final_byte: u8);
    /// A complete OSC payload, terminator excluded.
    fn osc(&mut self, payload: &[u8]);
}

/// Parsed CSI parameters: `;`-separated groups of `:`-separated values.
///
/// An empty parameter reads as 0, and every group holds at least one value.
#[derive(Debug, Default)]
pub(super) struct Params {
    /// Every value in order.
    values: Vec<u32>,
    /// Start offset into `values` of each group.
    starts: Vec<usize>,
}

impl Params {
    /// Number of `;`-separated groups.
    pub(super) fn len(&self) -> usize {
        self.starts.len()
    }

    /// The values of group `index`, empty when out of range.
    pub(super) fn group(&self, index: usize) -> &[u32] {
        let Some(&start) = self.starts.get(index) else {
            return &[];
        };
        let end = self
            .starts
            .get(index + 1)
            .copied()
            .unwrap_or(self.values.len());
        &self.values[start..end]
    }

    /// The leading value of group `index`, 0 when absent.
    pub(super) fn first(&self, index: usize) -> u32 {
        self.group(index).first().copied().unwrap_or(0)
    }

    /// Re-parses from raw parameter bytes. Returns `false` for bytes outside
    /// digits, `;` and `:`, which mark a form this terminal does not speak.
    fn parse(&mut self, bytes: &[u8]) -> bool {
        self.values.clear();
        self.starts.clear();
        self.starts.push(0);
        let mut current = 0_u32;
        for &byte in bytes {
            match byte {
                b'0'..=b'9' => {
                    current = current
                        .saturating_mul(10)
                        .saturating_add(u32::from(byte - b'0'))
                        .min(MAX_PARAM);
                }
                b':' => {
                    self.values.push(current);
                    current = 0;
                }
                b';' => {
                    self.values.push(current);
                    current = 0;
                    self.starts.push(self.values.len());
                }
                _ => return false,
            }
        }
        self.values.push(current);
        true
    }
}

/// Where the parser is between bytes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum State {
    /// Plain text.
    #[default]
    Ground,
    /// After `ESC`.
    Escape,
    /// After `ESC` and an intermediate (`ESC ( B` charset selection…).
    EscapeIntermediate,
    /// Inside `ESC [`.
    Csi,
    /// Inside `ESC ]`.
    Osc,
    /// Inside OSC, just after an `ESC` that may start `ST`.
    OscEscape,
    /// Inside a DCS/SOS/PM/APC string, which is dropped.
    Ignore,
    /// Inside a dropped string, just after an `ESC` that may start `ST`.
    IgnoreEscape,
}

/// The incremental byte parser; owns every partial-sequence buffer.
#[derive(Debug, Default)]
pub(super) struct Parser {
    /// Current state.
    state: State,
    /// Bytes of a UTF-8 code point still being assembled.
    utf8: [u8; 4],
    /// How many of `utf8` are filled.
    utf8_len: usize,
    /// Total length of the code point being assembled; 0 when none is.
    utf8_need: usize,
    /// CSI parameter and intermediate bytes.
    sequence: Vec<u8>,
    /// Whether the CSI outgrew [`MAX_SEQUENCE_BYTES`] and will be dropped.
    sequence_overflow: bool,
    /// OSC payload bytes.
    osc: Vec<u8>,
    /// Whether the OSC outgrew [`MAX_OSC_BYTES`] and will be dropped.
    osc_overflow: bool,
    /// Reused CSI parameter storage.
    params: Params,
}

impl Parser {
    /// Consumes one byte, reporting any completed action to `sink`.
    pub(super) fn advance<P: Perform>(&mut self, byte: u8, sink: &mut P) {
        match self.state {
            State::Ground => self.ground(byte, sink),
            State::Escape => self.escape(byte, sink),
            State::EscapeIntermediate => match byte {
                ESC => self.enter_escape(),
                0x20..=0x2f => {}
                _ => self.state = State::Ground,
            },
            State::Csi => self.csi(byte, sink),
            State::Osc => match byte {
                BEL => self.finish_osc(sink),
                ESC => self.state = State::OscEscape,
                CAN | SUB => self.state = State::Ground,
                _ if self.osc.len() < MAX_OSC_BYTES => self.osc.push(byte),
                _ => self.osc_overflow = true,
            },
            State::OscEscape => {
                if byte == b'\\' {
                    self.finish_osc(sink);
                } else {
                    // A bare ESC ends the OSC and starts a new sequence.
                    self.enter_escape();
                    self.escape(byte, sink);
                }
            }
            State::Ignore => match byte {
                ESC => self.state = State::IgnoreEscape,
                CAN | SUB => self.state = State::Ground,
                _ => {}
            },
            State::IgnoreEscape => match byte {
                b'\\' | CAN | SUB => self.state = State::Ground,
                ESC => {}
                _ => self.state = State::Ignore,
            },
        }
    }

    /// Plain text: UTF-8 assembly, controls, and the start of sequences.
    fn ground<P: Perform>(&mut self, byte: u8, sink: &mut P) {
        if self.utf8_need > 0 {
            if (0x80..=0xbf).contains(&byte) {
                self.utf8[self.utf8_len] = byte;
                self.utf8_len += 1;
                if self.utf8_len == self.utf8_need {
                    let ch = std::str::from_utf8(&self.utf8[..self.utf8_len])
                        .ok()
                        .and_then(|text| text.chars().next())
                        .unwrap_or(char::REPLACEMENT_CHARACTER);
                    self.utf8_need = 0;
                    sink.print(ch);
                }
                return;
            }
            // Truncated code point: replace it, then read this byte afresh.
            self.utf8_need = 0;
            sink.print(char::REPLACEMENT_CHARACTER);
        }
        if byte == ESC {
            self.enter_escape();
            return;
        }
        match byte {
            0x00..=0x1f | 0x7f => sink.execute(byte),
            0x20..=0x7e => sink.print(char::from(byte)),
            0xc2..=0xdf => self.begin_utf8(byte, 2),
            0xe0..=0xef => self.begin_utf8(byte, 3),
            0xf0..=0xf4 => self.begin_utf8(byte, 4),
            _ => sink.print(char::REPLACEMENT_CHARACTER),
        }
    }

    /// Starts assembling a `need`-byte code point.
    fn begin_utf8(&mut self, lead: u8, need: usize) {
        self.utf8[0] = lead;
        self.utf8_len = 1;
        self.utf8_need = need;
    }

    /// Enters [`State::Escape`].
    fn enter_escape(&mut self) {
        self.state = State::Escape;
    }

    /// The byte after `ESC`.
    fn escape<P: Perform>(&mut self, byte: u8, sink: &mut P) {
        self.state = State::Ground;
        match byte {
            b'[' => {
                self.sequence.clear();
                self.sequence_overflow = false;
                self.state = State::Csi;
            }
            b']' => {
                self.osc.clear();
                self.osc_overflow = false;
                self.state = State::Osc;
            }
            b'P' | b'X' | b'^' | b'_' => self.state = State::Ignore,
            ESC => self.state = State::Escape,
            CAN | SUB => {}
            0x20..=0x2f => self.state = State::EscapeIntermediate,
            // Controls inside an escape still execute (VT behaviour).
            0x00..=0x1f => {
                sink.execute(byte);
                self.state = State::Escape;
            }
            0x30..=0x7e => sink.esc(byte),
            _ => {}
        }
    }

    /// One byte inside `ESC [`.
    fn csi<P: Perform>(&mut self, byte: u8, sink: &mut P) {
        match byte {
            0x20..=0x3f => {
                if self.sequence.len() < MAX_SEQUENCE_BYTES {
                    self.sequence.push(byte);
                } else {
                    self.sequence_overflow = true;
                }
            }
            0x40..=0x7e => {
                self.state = State::Ground;
                if !self.sequence_overflow && self.params.parse(&self.sequence) {
                    sink.csi(&self.params, byte);
                }
            }
            ESC => self.enter_escape(),
            CAN | SUB => self.state = State::Ground,
            0x00..=0x1f => sink.execute(byte),
            0x7f => {}
            _ => self.state = State::Ground,
        }
    }

    /// Delivers a terminated OSC payload unless it overflowed.
    fn finish_osc<P: Perform>(&mut self, sink: &mut P) {
        self.state = State::Ground;
        if !self.osc_overflow {
            sink.osc(&self.osc);
        }
    }
}
