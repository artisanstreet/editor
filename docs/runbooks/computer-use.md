# Computer use

Forge serves eight app-independent tools at `/mcp/computer/{thread-token}`:
`screenshot`, `zoom`, `input`, `windows`, `window`, `launch`, `clipboard`,
and `handoff`. Browsers use the same capture and input code as every other app.

## Windows setup

Run **Windows Forge in your signed-in Windows session**. WSL Forge does not
expose this backend. Install a virtual monitor separately and extend the desktop
onto it. Driver installation and a live viewer in the Editor are not included.

Identify the monitor's device name in Windows display settings or with a display
diagnostic tool, then set it in the environment **before Forge starts**:

```powershell
$env:ARTISAN_COMPUTER_DISPLAY = '\\.\DISPLAY3'
forge
```

Use the actual `\\.\DISPLAYn` name assigned to the virtual monitor. Never choose
your primary monitor. Forge refuses a missing or primary monitor and does not
advertise the computer server. A disconnected display fails subsequent calls.
Restart Forge after changing the device name. This environment variable belongs
to the Windows Forge process, not the Editor or its WSL terminal.

Enable `computer_use` in Artisan's Forge settings. It defaults to false, and
changes through the Artisan MCP require approval. Start a new engine turn so it
receives the computer server configuration. Every tool call rechecks the setting,
so existing thread credentials lose access when it is off.

The setting affects this Forge host. Multiple threads share one configured
monitor. Calls serialize; concurrent calls return a busy result. This is not an
isolated Windows session, and installing another monitor does not create another
keyboard focus or cursor.

## Tools

Take `screenshot` before `input`. Its image dimensions define input coordinates.
The backend maps those coordinates to native pixels, including monitors at
negative coordinates and mixed DPI. `zoom` uses a rectangle in the original
screenshot and returns a sharper crop; it does not change input coordinates.
Window bounds also use full screenshot coordinates, relative to the agent display.

```json
{
  "steps": [
    {"click": [400, 300]},
    {"type": "Hello world"},
    {"key": "ctrl+s"},
    {"wait": {"until": "settled", "timeout_ms": 3000}}
  ],
  "screenshot": true
}
```

`input` accepts move, click, down, up, drag, scroll, key, key_down, key_up, type,
and wait. Use `type` for Unicode text. A chord presses its keys in order and
releases them in reverse. Separate down/up steps support held keys and drags
within one batch. Every batch releases its held keys and buttons before returning,
including after errors. Holds do not persist between calls. A batch accepts at
most 256 steps, a wait at most 30 seconds, and total input work at most 60 seconds.
After input it checks for a stable screen for up to one second, and reports
`settled`, `screen_changed`, and the completed step count. Animated screens may
return `settled: false`.

`windows` lists agent windows. Set `include_user: true` to request approval to list
windows elsewhere. `window` supports focus, close, maximize, bring_here and
send_back. Borrowing a window requires approval and remembers its old bounds for
that thread. `send_back` restores those bounds. Restoring maximized/minimized
state separately is not implemented.

`launch` requires approval and directly starts the command with its argument list
and optional working directory. It does not implicitly run a shell. It moves
windows owned by the new process and its descendants onto the agent monitor for
three seconds. Apps that open windows later or delegate to an existing instance
may need `bring_here`.

`clipboard` reads or writes shared Windows Unicode text after approval. Writes
replace the clipboard and persist until another application changes it. It does
not preserve the user's prior clipboard or its non-text formats.

## User input and handoff

Input starts only after at least 500 ms without physical input and while no keys
or mouse buttons are held. A Windows hook distinguishes Artisan's tagged input
from other input. Physical input interrupts the batch, releases held inputs and
pauses further agent input. The result includes a fresh screenshot. It does not
retry any step. Partial clicks or text may have already reached the app even
when the current step is not counted as complete.

Without an interruption, the runner attempts to restore the prior cursor and
foreground window. If you intervene, it leaves your cursor and focus alone.
A `handoff` pauses input and shows an approval card; allowing that card resumes
input. Denying or leaving it unanswered keeps input paused. Handoff also stops an
in-progress input batch at its next event or polling check. Take a new screenshot
after the user resumes.

A virtual monitor still shares your Windows session. Apps can open windows on
other monitors or change focus themselves. Keyboard and wheel input stop when
focus leaves the agent display. This backend does not control UAC's secure
desktop, bypass elevation, or install an elevation service. Windows may refuse
input into elevated apps; use handoff. See Microsoft's [SendInput documentation](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)
for the integrity-level restriction.

## Verification

```sh
cargo test -p forge-computer -p forge-mcp -p forge-http
cargo check -p forge-computer -p forge-http --target x86_64-pc-windows-gnu
```

The HTTP tests need permission to bind loopback sockets. The tests use a mock
desktop and cover input interruption, held-input cleanup, coordinate validation,
images, setting revocation, and the MCP protocol. Windows cross-compilation
checks Win32 bindings; actual capture, input hooks, clipboard ownership and focus
restoration still need a Windows session for an end-to-end smoke test.
