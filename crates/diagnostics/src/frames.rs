#[cfg(unix)]
#[expect(unsafe_code, reason = "dladdr is a C call")]
mod unix;
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "GetModuleHandleExW and GetModuleFileNameW are Win32 calls"
)]
mod windows;

#[cfg(unix)]
use unix::module_of;
#[cfg(windows)]
use windows::module_of;

pub const HEADING: &str = "frames (instruction pointer, module base and offset):";

#[derive(Debug)]
struct Module {
    path: String,
    base: usize,
}

#[derive(Debug)]
struct Frame {
    ip: usize,
    module: Option<Module>,
}

#[derive(Debug)]
pub struct Frames(Vec<Frame>);

impl Frames {
    #[must_use]
    pub fn capture() -> Self {
        let mut ips = Vec::new();
        backtrace::trace(|frame| {
            ips.push(frame.ip() as usize);
            true
        });
        Self(
            ips.into_iter()
                .map(|ip| Frame {
                    ip,
                    module: module_of(ip),
                })
                .collect(),
        )
    }
}

impl std::fmt::Display for Frames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, frame) in self.0.iter().enumerate() {
            match &frame.module {
                Some(module) => writeln!(
                    f,
                    "{index:>4}: ip=0x{:016x} base=0x{:016x} offset=0x{:x} module={}",
                    frame.ip,
                    module.base,
                    frame.ip.wrapping_sub(module.base),
                    module.path
                )?,
                None => writeln!(f, "{index:>4}: ip=0x{:016x} module=unknown", frame.ip)?,
            }
        }
        Ok(())
    }
}
