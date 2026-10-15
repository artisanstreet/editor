//! VTE owns incremental UTF-8 and escape parsing; this sink keeps the
//! application's supported line-model operations and bounded OSC links.

pub(super) trait Perform {
    fn print(&mut self, ch: char);
    fn execute(&mut self, byte: u8);
    fn csi(&mut self, params: &Params, final_byte: u8);
    fn esc(&mut self, final_byte: u8);
    fn osc(&mut self, payload: &[u8]);
}

#[derive(Debug, Default)]
pub(super) struct Params(Vec<Vec<u32>>);
impl Params {
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
    pub(super) fn group(&self, index: usize) -> &[u32] {
        self.0.get(index).map_or(&[], Vec::as_slice)
    }
    pub(super) fn first(&self, index: usize) -> u32 {
        self.group(index).first().copied().unwrap_or(0)
    }
}

pub(super) struct Parser(vte::Parser<4096>);
impl Default for Parser {
    fn default() -> Self {
        Self(vte::Parser::new_with_size())
    }
}
impl std::fmt::Debug for Parser {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("VteParser")
    }
}
impl Parser {
    pub(super) fn advance<P: Perform>(&mut self, byte: u8, sink: &mut P) {
        self.0.advance(&mut Sink(sink), &[byte]);
    }
}
struct Sink<'a, P>(&'a mut P);
impl<P: Perform> vte::Perform for Sink<'_, P> {
    fn print(&mut self, ch: char) {
        self.0.print(ch);
    }
    fn execute(&mut self, byte: u8) {
        self.0.execute(byte);
    }
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if ignore || !intermediates.is_empty() {
            return;
        }
        let Ok(action) = u8::try_from(action as u32) else {
            return;
        };
        let params = Params(
            params
                .iter()
                .map(|group| group.iter().copied().map(u32::from).collect())
                .collect(),
        );
        self.0.csi(&params, action);
    }
    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if !ignore && intermediates.is_empty() {
            self.0.esc(byte);
        }
    }
    fn osc_dispatch(&mut self, params: &[&[u8]], _: bool) {
        let size = params
            .iter()
            .map(|part| part.len())
            .sum::<usize>()
            .saturating_add(params.len().saturating_sub(1));
        if size >= 4096 {
            return;
        }
        let mut payload = Vec::with_capacity(size);
        for (index, part) in params.iter().enumerate() {
            if index > 0 {
                payload.push(b';');
            }
            payload.extend_from_slice(part);
        }
        self.0.osc(&payload);
    }
}
