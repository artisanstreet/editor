#![expect(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    reason = "reading the Editor the way a screen reader does takes Win32 windows and UI Automation's COM client"
)]

use super::editor::Editor;
use aqua::AccessSink;
use aqua_a11y_uia::{Connection, UiaAdapter};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use std::fmt::Write as _;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HWND;
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
    UIA_EditControlTypeId, UIA_ExpandCollapsePatternId, UIA_GroupControlTypeId,
    UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId, UIA_InvokePatternId, UIA_ListControlTypeId,
    UIA_ListItemControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
    UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId, UIA_RadioButtonControlTypeId,
    UIA_ScrollBarControlTypeId, UIA_SliderControlTypeId, UIA_TabControlTypeId,
    UIA_TabItemControlTypeId, UIA_TextControlTypeId, UIA_TogglePatternId, UIA_ValuePatternId,
    UIA_WindowControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SW_SHOWNA,
    ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
};
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
}

type Reply = Result<String, String>;

pub struct Uia {
    hwnd: isize,
    requests: Option<Sender<Request>>,
    replies: Receiver<Reply>,
    client: Option<JoinHandle<()>>,
}

fn native(error: &windows::core::Error) -> String {
    format!("{error} ({:?})", error.code())
}

fn absent<T>(result: windows::core::Result<T>) -> Result<Option<T>, String> {
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

unsafe fn seen(element: &IUIAutomationElement) -> Result<Seen, String> {
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
) -> Result<(), String> {
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

unsafe fn find(
    walker: &IUIAutomationTreeWalker,
    element: &IUIAutomationElement,
    name: &str,
) -> Result<Option<IUIAutomationElement>, String> {
    unsafe {
        if element.CurrentName().map_err(|error| native(&error))? == name {
            return Ok(Some(element.clone()));
        }
        let mut child = absent(walker.GetFirstChildElement(element))?;
        while let Some(current) = child {
            if let Some(found) = find(walker, &current, name)? {
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
        let target = |name: &str| {
            find(&walker, &root, name)?
                .ok_or_else(|| format!("UI Automation has no element named {name:?}"))
        };
        match request {
            Request::Tree => {
                let mut out = String::new();
                write_tree(&mut out, &walker, &root, 0)?;
                Ok(out)
            }
            Request::Invoke { name } => target(name)?
                .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                .and_then(|pattern| pattern.Invoke())
                .map(|()| String::new())
                .map_err(|error| native(&error)),
            Request::Expand { name } => target(name)?
                .GetCurrentPatternAs::<IUIAutomationExpandCollapsePattern>(
                    UIA_ExpandCollapsePatternId,
                )
                .and_then(|pattern| pattern.Expand())
                .map(|()| String::new())
                .map_err(|error| native(&error)),
            Request::Toggle { name } => target(name)?
                .GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId)
                .and_then(|pattern| pattern.Toggle())
                .map(|()| String::new())
                .map_err(|error| native(&error)),
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
                Err(error) => Err(format!("start UI Automation: {}", native(error))),
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
                Ok(Ok(answer)) => return answer,
                Ok(Err(error)) => panic!("UI Automation failed {request:?}: {error}"),
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
            let tree = self.ask(uia, &Request::Tree);
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
