#![expect(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    reason = "reading the Editor the way a screen reader does takes Win32 windows and UI Automation's COM client"
)]

use super::editor::Editor;
use crate::replay::ReplayingEngine;
use aqua::{AccessSink, Aqua, AquaConfig};
use aqua_a11y_uia::{Connection, UiaAdapter};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use editor_aqua::core::Core;
use editor_aqua::qualification::fixture::ForgeServer;
use editor_aqua::shell::application::ShellApplication;
use editor_aqua::shell::host::ShellResources;
use editor_aqua::shell::root::RootViews;
use std::fmt::Write as _;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tempfile::TempDir;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_MULTITHREADED, CoCreateInstance,
    CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation8, ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded,
    ExpandCollapseState_PartiallyExpanded, IUIAutomation, IUIAutomationElement,
    IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern, IUIAutomationTogglePattern,
    IUIAutomationTreeWalker, IUIAutomationValuePattern, ToggleState_Indeterminate, ToggleState_On,
    UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_CustomControlTypeId, UIA_DocumentControlTypeId,
    UIA_E_ELEMENTNOTAVAILABLE, UIA_EditControlTypeId, UIA_ExpandCollapsePatternId,
    UIA_GroupControlTypeId, UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId,
    UIA_InvokePatternId, UIA_ListControlTypeId, UIA_ListItemControlTypeId, UIA_MenuControlTypeId,
    UIA_MenuItemControlTypeId, UIA_PATTERN_ID, UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId,
    UIA_RadioButtonControlTypeId, UIA_ScrollBarControlTypeId, UIA_SliderControlTypeId,
    UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_TextControlTypeId, UIA_TogglePatternId,
    UIA_ValuePatternId, UIA_WindowControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW,
    GetWindowThreadProcessId, IsWindowVisible, MSG, PM_REMOVE, PeekMessageW, PostMessageW,
    SW_SHOWNA, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::BOOL;
use windows::core::w;

const ANSWER: Duration = Duration::from_secs(20);
const QUIET: Duration = Duration::from_millis(300);

struct UiaSink {
    adapter: UiaAdapter,
}

impl AccessSink for UiaSink {
    fn needs_snapshot(&self) -> bool {
        self.adapter.connection() == Connection::Connected
    }

    fn connection_epoch(&self) -> u64 {
        u64::from(self.needs_snapshot())
    }

    fn apply(&mut self, update: &AccessTreeUpdate) {
        self.adapter.apply(update);
    }

    fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        self.adapter.drain_actions()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Request {
    Tree,
    Invoke { name: String },
    Toggle { name: String },
    Expand { name: String },
    SetValue { name: String, value: String },
}

#[derive(Debug)]
enum Failure {
    Vanished(String),
    Native(String),
    Missing(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vanished(text) => write!(f, "an element vanished mid-read: {text}"),
            Self::Native(text) => f.write_str(text),
            Self::Missing(name) => write!(f, "UI Automation has no element named {name:?}"),
        }
    }
}

type Reply = Result<String, Failure>;

pub struct Uia {
    hwnd: isize,
    requests: Option<Sender<Request>>,
    replies: Receiver<Reply>,
    client: Option<JoinHandle<()>>,
}

fn native(error: &windows::core::Error) -> Failure {
    let text = format!("{error} ({:?})", error.code());
    match error.code().0.cast_unsigned() {
        UIA_E_ELEMENTNOTAVAILABLE => Failure::Vanished(text),
        _ => Failure::Native(text),
    }
}

fn absent<T>(result: windows::core::Result<T>) -> Result<Option<T>, Failure> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.code().is_ok() => Ok(None),
        Err(error) => Err(native(&error)),
    }
}

fn kind(id: UIA_CONTROLTYPE_ID) -> String {
    [
        (UIA_ButtonControlTypeId, "Button"),
        (UIA_CheckBoxControlTypeId, "CheckBox"),
        (UIA_ComboBoxControlTypeId, "ComboBox"),
        (UIA_CustomControlTypeId, "Custom"),
        (UIA_DocumentControlTypeId, "Document"),
        (UIA_EditControlTypeId, "Edit"),
        (UIA_GroupControlTypeId, "Group"),
        (UIA_HyperlinkControlTypeId, "Hyperlink"),
        (UIA_ImageControlTypeId, "Image"),
        (UIA_ListControlTypeId, "List"),
        (UIA_ListItemControlTypeId, "ListItem"),
        (UIA_MenuControlTypeId, "Menu"),
        (UIA_MenuItemControlTypeId, "MenuItem"),
        (UIA_PaneControlTypeId, "Pane"),
        (UIA_ProgressBarControlTypeId, "ProgressBar"),
        (UIA_RadioButtonControlTypeId, "RadioButton"),
        (UIA_ScrollBarControlTypeId, "ScrollBar"),
        (UIA_SliderControlTypeId, "Slider"),
        (UIA_TabControlTypeId, "Tab"),
        (UIA_TabItemControlTypeId, "TabItem"),
        (UIA_TextControlTypeId, "Text"),
        (UIA_WindowControlTypeId, "Window"),
    ]
    .into_iter()
    .find_map(|(known, name)| (known == id).then(|| name.to_owned()))
    .unwrap_or_else(|| format!("ControlType({})", id.0))
}

struct Seen {
    kind: String,
    name: String,
    value: Option<String>,
    states: Vec<&'static str>,
}

unsafe fn seen(element: &IUIAutomationElement) -> Result<Seen, Failure> {
    unsafe {
        let kind = kind(
            element
                .CurrentControlType()
                .map_err(|error| native(&error))?,
        );
        let name = element
            .CurrentName()
            .map_err(|error| native(&error))?
            .to_string();
        let value =
            absent(element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId))?
                .map(|pattern| pattern.CurrentValue().map(|value| value.to_string()))
                .transpose()
                .map_err(|error| native(&error))?
                .filter(|value| !value.is_empty() && *value != name);
        let toggled =
            absent(element.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId))?
                .map(|pattern| pattern.CurrentToggleState())
                .transpose()
                .map_err(|error| native(&error))?
                .map(|state| {
                    if state == ToggleState_On {
                        "checked"
                    } else if state == ToggleState_Indeterminate {
                        "mixed"
                    } else {
                        "unchecked"
                    }
                });
        let expanded = absent(
            element.GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                UIA_ExpandCollapsePatternId,
            ),
        )?
        .map(|pattern| pattern.CurrentExpandCollapseState())
        .transpose()
        .map_err(|error| native(&error))?
        .and_then(|state| {
            if state == ExpandCollapseState_Expanded
                || state == ExpandCollapseState_PartiallyExpanded
            {
                Some("expanded")
            } else if state == ExpandCollapseState_Collapsed {
                Some("collapsed")
            } else {
                None
            }
        });
        let disabled = !element
            .CurrentIsEnabled()
            .map_err(|error| native(&error))?
            .as_bool();
        let focused = element
            .CurrentHasKeyboardFocus()
            .map_err(|error| native(&error))?
            .as_bool();
        let states = [toggled, expanded]
            .into_iter()
            .flatten()
            .chain(disabled.then_some("disabled"))
            .chain(focused.then_some("focused"))
            .collect();
        Ok(Seen {
            kind,
            name,
            value,
            states,
        })
    }
}

unsafe fn write_tree(
    out: &mut String,
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    depth: usize,
) -> Result<(), Failure> {
    unsafe {
        let seen = seen(element)?;
        let structural = matches!(seen.kind.as_str(), "Group" | "Pane" | "Custom")
            && seen.name.is_empty()
            && seen.value.is_none()
            && seen.states.iter().all(|state| *state == "disabled");
        let depth = if structural {
            depth
        } else {
            write!(out, "{:indent$}{}", "", seen.kind, indent = depth * 2)
                .expect("invariant: writing to a String succeeds");
            if !seen.name.is_empty() {
                write!(out, " {:?}", seen.name).expect("invariant: writing to a String succeeds");
            }
            if let Some(value) = &seen.value {
                write!(out, " value={value:?}").expect("invariant: writing to a String succeeds");
            }
            if !seen.states.is_empty() {
                write!(out, " [{}]", seen.states.join(", "))
                    .expect("invariant: writing to a String succeeds");
            }
            out.push('\n');
            depth + 1
        };
        let mut child = absent(walker.GetFirstChildElement(element))?;
        while let Some(current) = child {
            write_tree(out, walker, &current, depth)?;
            child = absent(walker.GetNextSiblingElement(&current))?;
        }
        Ok(())
    }
}

unsafe fn find<P: windows::core::Interface>(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    name: &str,
    pattern: UIA_PATTERN_ID,
) -> Result<Option<P>, Failure> {
    unsafe {
        if element.CurrentName().map_err(|error| native(&error))? == name
            && let Some(found) = absent(element.GetCurrentPatternAs::<P>(pattern))?
        {
            return Ok(Some(found));
        }
        let mut child = absent(walker.GetFirstChildElement(element))?;
        while let Some(current) = child {
            if let Some(found) = find(walker, &current, name, pattern)? {
                return Ok(Some(found));
            }
            child = absent(walker.GetNextSiblingElement(&current))?;
        }
        Ok(None)
    }
}

unsafe fn answer(client: &IUIAutomation, hwnd: isize, request: &Request) -> Reply {
    unsafe {
        let root = client
            .ElementFromHandle(HWND(hwnd as *mut core::ffi::c_void))
            .map_err(|error| native(&error))?;
        let walker = client.ControlViewWalker().map_err(|error| native(&error))?;
        match request {
            Request::Tree => {
                let mut out = String::new();
                write_tree(&mut out, &walker, &root, 0)?;
                Ok(out)
            }
            Request::Invoke { name } => {
                find::<IUIAutomationInvokePattern>(&walker, &root, name, UIA_InvokePatternId)?
                    .ok_or_else(|| Failure::Missing(name.clone()))?
                    .Invoke()
                    .map(|()| String::new())
                    .map_err(|error| native(&error))
            }
            Request::Expand { name } => find::<IUIAutomationExpandCollapsePattern>(
                &walker,
                &root,
                name,
                UIA_ExpandCollapsePatternId,
            )?
            .ok_or_else(|| Failure::Missing(name.clone()))?
            .Expand()
            .map(|()| String::new())
            .map_err(|error| native(&error)),
            Request::Toggle { name } => {
                find::<IUIAutomationTogglePattern>(&walker, &root, name, UIA_TogglePatternId)?
                    .ok_or_else(|| Failure::Missing(name.clone()))?
                    .Toggle()
                    .map(|()| String::new())
                    .map_err(|error| native(&error))
            }
            Request::SetValue { name, value } => {
                find::<IUIAutomationValuePattern>(&walker, &root, name, UIA_ValuePatternId)?
                    .ok_or_else(|| Failure::Missing(name.clone()))?
                    .SetValue(&windows::core::BSTR::from(value.as_str()))
                    .map(|()| String::new())
                    .map_err(|error| native(&error))
            }
        }
    }
}

fn client(hwnd: isize, requests: Receiver<Request>, replies: Sender<Reply>) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let client: windows::core::Result<IUIAutomation> = unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .and_then(|()| CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER))
        };
        for request in requests {
            let reply = match &client {
                Ok(client) => unsafe { answer(client, hwnd, &request) },
                Err(error) => Err(Failure::Native(format!(
                    "start UI Automation: {}",
                    native(error)
                ))),
            };
            if replies.send(reply).is_err() {
                return;
            }
        }
    })
}

fn pump_messages() {
    let mut message = MSG::default();
    while unsafe { PeekMessageW(&raw mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        unsafe {
            let _translated = TranslateMessage(&raw const message);
            DispatchMessageW(&raw const message);
        }
    }
}

impl Editor {
    #[must_use]
    pub fn read_through_uia(&mut self) -> Uia {
        let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        assert!(
            initialized.is_ok(),
            "the Editor's thread joins a single-threaded COM apartment: {initialized:?}"
        );
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0),
                w!("STATIC"),
                w!("Artisan Editor"),
                WS_POPUP,
                -32000,
                -32000,
                1000,
                700,
                None,
                None,
                None,
                None,
            )
        }
        .unwrap_or_else(|error| panic!("create the Editor's provider window: {}", native(&error)));
        let raw = hwnd.0 as isize;
        let adapter = UiaAdapter::attach(raw, self.access_waker()).unwrap_or_else(|error| {
            panic!("attach UI Automation to the Editor's window: {error:?}")
        });
        self.attach_access(Box::new(UiaSink { adapter }));
        let _shown = unsafe { ShowWindow(hwnd, SW_SHOWNA) };
        let (requests, received) = channel();
        let (answered, replies) = channel();
        Uia {
            hwnd: raw,
            requests: Some(requests),
            replies,
            client: Some(client(raw, received, answered)),
        }
    }

    #[track_caller]
    fn ask(&mut self, uia: &Uia, request: &Request) -> String {
        self.reply(uia, request)
            .unwrap_or_else(|error| panic!("UI Automation failed {request:?}: {error}"))
    }

    #[track_caller]
    fn reply(&mut self, uia: &Uia, request: &Request) -> Reply {
        uia.requests
            .as_ref()
            .expect("invariant: the UI Automation client runs until the reader drops")
            .send(request.clone())
            .expect("the UI Automation client is running");
        let deadline = Instant::now() + ANSWER;
        loop {
            pump_messages();
            self.frame();
            match uia.replies.try_recv() {
                Ok(reply) => return reply,
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    panic!("the UI Automation client stopped during {request:?}")
                }
            }
            assert!(
                Instant::now() < deadline,
                "UI Automation did not answer {request:?} within {ANSWER:?}"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
    }

    #[track_caller]
    pub fn uia_settle(&mut self, uia: &Uia, what: &str, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + ANSWER;
        let mut previous = String::new();
        let mut quiet_since = Instant::now();
        loop {
            let tree = match self.reply(uia, &Request::Tree) {
                Ok(tree) => tree,
                Err(Failure::Vanished(_)) => String::new(),
                Err(error) => panic!("UI Automation failed to read the tree: {error}"),
            };
            if !done(&tree) || tree != previous {
                quiet_since = Instant::now();
            }
            if quiet_since.elapsed() >= QUIET {
                return tree;
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {ANSWER:?} waiting for {what}; UI Automation sees:\n{tree}"
            );
            previous = tree;
        }
    }

    #[track_caller]
    pub fn uia_invoke(&mut self, uia: &Uia, name: &str) {
        self.ask(
            uia,
            &Request::Invoke {
                name: name.to_owned(),
            },
        );
    }

    #[track_caller]
    pub fn uia_expand(&mut self, uia: &Uia, name: &str) {
        self.ask(
            uia,
            &Request::Expand {
                name: name.to_owned(),
            },
        );
    }

    #[track_caller]
    pub fn uia_toggle(&mut self, uia: &Uia, name: &str) {
        self.ask(
            uia,
            &Request::Toggle {
                name: name.to_owned(),
            },
        );
    }
}

impl Drop for Uia {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(client) = self.client.take() {
            client
                .join()
                .expect("the UI Automation client thread finishes cleanly");
        }
        let _destroyed = unsafe { DestroyWindow(HWND(self.hwnd as *mut core::ffi::c_void)) };
    }
}

impl std::fmt::Debug for Uia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Uia")
            .field("hwnd", &self.hwnd)
            .finish_non_exhaustive()
    }
}

pub struct NativeEditor {
    hwnd: isize,
    requests: Option<Sender<Request>>,
    replies: Receiver<Reply>,
    client: Option<JoinHandle<()>>,
    window: Option<JoinHandle<()>>,
    closed: Receiver<Result<(), String>>,
    _prepared: (TempDir, Vec<ForgeServer>, ReplayingEngine),
}

struct Search {
    process: u32,
    found: Option<HWND>,
}

unsafe extern "system" fn editor_window(hwnd: HWND, search: LPARAM) -> BOOL {
    let search = unsafe { &mut *(search.0 as *mut Search) };
    let mut process = 0;
    let _thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut process)) };
    let mut class = [0u16; 32];
    let length = unsafe { GetClassNameW(hwnd, &mut class) };
    let aqua = length > 0
        && String::from_utf16_lossy(&class[..length.unsigned_abs() as usize]) == "AquaShell32";
    if process == search.process && aqua && unsafe { IsWindowVisible(hwnd) }.as_bool() {
        search.found = Some(hwnd);
        return BOOL(0);
    }
    BOOL(1)
}

fn find_editor_window() -> Option<isize> {
    let mut search = Search {
        process: std::process::id(),
        found: None,
    };
    let enumerated = unsafe {
        EnumWindows(
            Some(editor_window),
            LPARAM(std::ptr::from_mut(&mut search) as isize),
        )
    };
    match (search.found, enumerated) {
        (Some(hwnd), _) => Some(hwnd.0 as isize),
        (None, Ok(())) => None,
        (None, Err(error)) => panic!("list this process's windows: {}", native(&error)),
    }
}

fn run_native(
    core: impl FnOnce() -> Core,
    (width, height): (f64, f64),
    closed: &Sender<Result<(), String>>,
) {
    let mut config = AquaConfig::default();
    "Artisan Editor".clone_into(&mut config.window.title);
    config.window.inner_size.width = width;
    config.window.inner_size.height = height;
    let ran = Aqua::new(config).and_then(|aqua| {
        let resources = ShellResources::for_runtime(&aqua, 1.0)
            .ok_or_else(|| "lay out the Editor at scale 1".to_owned())?;
        let app = ShellApplication::for_qualification(core(), resources, RootViews::default());
        aqua.run(app)
    });
    let _reported = closed.send(ran);
}

impl NativeEditor {
    pub(super) fn open(
        core: impl FnOnce() -> Core + Send + 'static,
        size: (f64, f64),
        kept: (TempDir, Vec<ForgeServer>, ReplayingEngine),
    ) -> Self {
        let (closed_sender, closed) = channel();
        let window = std::thread::spawn(move || run_native(core, size, &closed_sender));
        let deadline = Instant::now() + ANSWER;
        let hwnd = loop {
            if let Some(hwnd) = find_editor_window() {
                break hwnd;
            }
            if let Ok(ran) = closed.try_recv() {
                panic!("the Editor's native window closed before it was shown: {ran:?}");
            }
            assert!(
                Instant::now() < deadline,
                "the Editor's native window did not appear within {ANSWER:?}"
            );
            std::thread::park_timeout(Duration::from_millis(10));
        };
        let (requests, received) = channel();
        let (answered, replies) = channel();
        Self {
            hwnd,
            requests: Some(requests),
            replies,
            client: Some(client(hwnd, received, answered)),
            window: Some(window),
            closed,
            _prepared: kept,
        }
    }
}

impl NativeEditor {
    #[track_caller]
    fn ask(&self, request: &Request) -> String {
        self.reply(request)
            .unwrap_or_else(|error| panic!("UI Automation failed {request:?}: {error}"))
    }

    #[track_caller]
    fn reply(&self, request: &Request) -> Reply {
        self.requests
            .as_ref()
            .expect("invariant: the UI Automation client runs until the reader drops")
            .send(request.clone())
            .expect("the UI Automation client is running");
        self.replies
            .recv_timeout(ANSWER)
            .unwrap_or_else(|error| panic!("UI Automation did not answer {request:?}: {error}"))
    }

    #[track_caller]
    pub fn uia_settle(&self, what: &str, done: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + ANSWER;
        let mut previous = String::new();
        let mut quiet_since = Instant::now();
        loop {
            let tree = match self.reply(&Request::Tree) {
                Ok(tree) => tree,
                Err(Failure::Vanished(_)) => String::new(),
                Err(error) => panic!("UI Automation failed to read the tree: {error}"),
            };
            if !done(&tree) || tree != previous {
                quiet_since = Instant::now();
            }
            if quiet_since.elapsed() >= QUIET {
                return tree;
            }
            assert!(
                Instant::now() < deadline,
                "timed out after {ANSWER:?} waiting for {what}; UI Automation sees:\n{tree}"
            );
            previous = tree;
            std::thread::park_timeout(Duration::from_millis(10));
        }
    }

    #[track_caller]
    pub fn uia_set_value(&self, name: &str, value: &str) {
        self.ask(&Request::SetValue {
            name: name.to_owned(),
            value: value.to_owned(),
        });
    }

    #[track_caller]
    pub fn close(mut self) {
        let posted = unsafe {
            PostMessageW(
                Some(HWND(self.hwnd as *mut core::ffi::c_void)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            )
        };
        assert!(
            posted.is_ok(),
            "ask the Editor's window to close: {posted:?}"
        );
        match self.closed.recv_timeout(ANSWER) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => panic!("the Editor's window loop failed: {error}"),
            Err(error) => panic!("the Editor's window did not close within {ANSWER:?}: {error}"),
        }
        if let Some(window) = self.window.take() {
            window
                .join()
                .expect("the Editor's window thread finishes cleanly");
        }
    }
}

impl Drop for NativeEditor {
    fn drop(&mut self) {
        self.requests.take();
        if let Some(client) = self.client.take() {
            client
                .join()
                .expect("the UI Automation client thread finishes cleanly");
        }
    }
}

impl std::fmt::Debug for NativeEditor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeEditor")
            .field("hwnd", &self.hwnd)
            .finish_non_exhaustive()
    }
}
