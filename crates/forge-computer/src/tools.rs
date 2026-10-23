use forge_mcp::Tool;
use serde_json::{Value, json};
pub fn list() -> Vec<Tool> {
    vec![
        tool(
            "screenshot",
            "Capture the agent display. Returns PNG, dimensions, cursor and focus. Start here.",
            || json!({}),
            &[],
            true,
        ),
        tool(
            "zoom",
            "Read a region at higher resolution. Rectangle uses full screenshot pixels. Input still uses full screenshot coordinates.",
            || json!({"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}}),
            &["x", "y", "width", "height"],
            true,
        ),
        tool(
            "input",
            "Run up to 256 steps in order. Coordinates [x,y] use the last full screenshot. Steps: move:[x,y], click:[x,y] with optional button/count; down/up:[x,y] or true; drag:[[x,y],...] with optional button; scroll:[x,y] with dx/dy wheel notches (positive down/right); key:'ctrl+s'; key_down/key_up:'Shift'; type:'Unicode text'; wait:{ms:100} or {until:'changed'|'settled',timeout_ms:3000}. One action per step. Held keys/buttons release at batch end. Physical input interrupts and pauses. Completed step count is returned; never blindly retry.",
            || json!({"steps":{"type":"array","minItems":1,"maxItems":256,"items":{"type":"object"}},"screenshot":{"type":"boolean","default":false}}),
            &["steps"],
            false,
        ),
        tool(
            "windows",
            "List agent windows with ids, process, bounds and focus. include_user lists windows on other displays too, after user approval.",
            || json!({"include_user":{"type":"boolean","default":false}}),
            &[],
            true,
        ),
        tool(
            "window",
            "Change a window. bring_here requires user approval and remembers original bounds; send_back restores them.",
            || json!({"id":{"type":"integer","minimum":1},"action":{"type":"string","enum":["focus","close","maximize","bring_here","send_back"]}}),
            &["id", "action"],
            false,
        ),
        tool(
            "launch",
            "Launch a program directly with arguments, without a shell. User approves. Move its new process windows to the agent display for three seconds. No elevation.",
            || json!({"command":{"type":"string"},"args":{"type":"array","items":{"type":"string"}},"cwd":{"type":"string"}}),
            &["command"],
            false,
        ),
        tool(
            "clipboard",
            "Read or write the shared Windows text clipboard after user approval. Writes replace its contents and remain until changed.",
            || json!({"action":{"type":"string","enum":["read","write"]},"text":{"type":"string"}}),
            &["action"],
            false,
        ),
        tool(
            "handoff",
            "Pause input for the user to complete a task or resume after interruption. Wait for explicit user approval to resume.",
            || json!({"reason":{"type":"string"}}),
            &["reason"],
            false,
        ),
    ]
}
fn tool(
    name: &'static str,
    description: &'static str,
    properties: fn() -> Value,
    required: &'static [&'static str],
    read_only: bool,
) -> Tool {
    Tool {
        name,
        description,
        properties,
        required,
        read_only,
        destructive: !read_only,
    }
}
