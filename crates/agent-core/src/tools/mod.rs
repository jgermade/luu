//! What the agent can do, and the one place it is checked.
//!
//! The model never executes anything. It emits a structured request; this
//! module parses it, validates it against the [`Sandbox`], and runs real Rust.
//! Any path where model output reaches a shell or the filesystem without
//! passing that validation is a bug, however convenient.
//!
//! **The definitions live in the cached prefix**, so [`Tools::definitions`] is a
//! wire format: tools sorted by name, schemas serialized through `serde_json`'s
//! sorted maps, nothing interpolated. Reordering the set or re-serializing a
//! schema with different key order costs the whole prompt cache on every call,
//! and nothing fails — it just gets slower.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::sandbox::{Sandbox, Verdict};

pub mod command;
pub mod fs;

pub use command::RunCommand;
pub use fs::{EditFile, ListDir, ReadFile, WriteFile};

/// Tool output is capped rather than pruned. Pruning old results out of the
/// history is a later, measured change; a cap now is the part that is not a
/// strategy — a `cat` of a 2 MB file must not be able to blow the window open.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024;

/// What the model asked for, whatever syntax it used to ask.
///
/// The type is the interface. How a model expresses a call is a transport
/// detail that differs per backend — a fenced block today, native function
/// calling and a GBNF grammar next — and the loop must not care which.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

/// What running it produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub verdict: Verdict,
    /// What the model gets back. Empty on a denial: the verdict is the answer.
    pub output: String,
    /// The tool ran and failed — a missing file, a non-zero exit. Distinct from
    /// a denial, which never ran, and the model needs to tell them apart to
    /// decide whether trying something else is worth a turn.
    pub error: Option<String>,
    pub truncated: bool,
}

impl ToolOutcome {
    pub fn ok(verdict: Verdict, output: impl Into<String>) -> Self {
        let (output, truncated) = clamp(output.into());
        Self {
            verdict,
            output,
            error: None,
            truncated,
        }
    }

    pub fn failed(verdict: Verdict, error: impl Into<String>) -> Self {
        Self {
            verdict,
            output: String::new(),
            error: Some(error.into()),
            truncated: false,
        }
    }

    pub fn denied(verdict: Verdict) -> Self {
        let error = format!("denied: {}", verdict.rule);
        Self {
            verdict,
            output: String::new(),
            error: Some(error),
            truncated: false,
        }
    }

    /// How the result is rendered back into the conversation.
    ///
    /// Plain text and not JSON: a 7B pays for every token of a wrapper it did
    /// not need, and this one is read, never parsed.
    pub fn render(&self, name: &str) -> String {
        let mut text = match &self.error {
            Some(error) => format!("[{name}] {error}"),
            None => format!("[{name}] ok"),
        };
        if !self.output.is_empty() {
            text.push('\n');
            text.push_str(&self.output);
        }
        if self.truncated {
            text.push_str(&format!(
                "\n[{name}] output cut at {MAX_OUTPUT_BYTES} bytes"
            ));
        }
        text
    }
}

/// Cuts at a character boundary, so the result is still a string.
fn clamp(mut text: String) -> (String, bool) {
    if text.len() <= MAX_OUTPUT_BYTES {
        return (text, false);
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

/// One call and what came back, as it is kept in the history.
///
/// `text` is the assistant's own words verbatim rather than a re-rendering of
/// `call`: it is what goes back into the prompt, and a prompt whose earlier
/// turns are regenerated rather than replayed is a different prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolStep {
    pub text: String,
    pub call: ToolCall,
    pub outcome: ToolOutcome,
    pub duration_ms: u64,
}

impl ToolStep {
    /// The user-side message the result is fused into.
    pub fn result_text(&self) -> String {
        self.outcome.render(&self.call.name)
    }
}

pub type ToolFuture<'a> = Pin<Box<dyn Future<Output = ToolOutcome> + Send + 'a>>;

/// One thing the agent can do.
///
/// Object-safe on purpose — the registry holds `dyn Tool`, so adding a tool
/// never reaches the loop. `Pin<Box<dyn Future>>` rather than an `async fn`
/// for the same reason [`crate::backend::Backend`] returns a boxed stream.
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    /// One line, in the prompt, paid for on every call. Write it that way.
    fn description(&self) -> &'static str;
    /// JSON Schema for the arguments.
    fn parameters(&self) -> serde_json::Value;
    /// Runs it. Every implementation asks the sandbox first; there is no path
    /// through here that does not.
    fn run<'a>(&'a self, arguments: &'a serde_json::Value, sandbox: &'a Sandbox) -> ToolFuture<'a>;
}

/// The tool set, and its rendering into the stable prefix.
pub struct Tools {
    tools: Vec<Box<dyn Tool>>,
}

impl Default for Tools {
    fn default() -> Self {
        Self::standard()
    }
}

impl Tools {
    /// Sorted by name at construction, because the order is part of the cached
    /// prefix and the declaration order of a `vec!` is not a contract.
    pub fn new(mut tools: Vec<Box<dyn Tool>>) -> Self {
        tools.sort_by_key(|tool| tool.name());
        Self { tools }
    }

    pub fn standard() -> Self {
        Self::new(vec![
            Box::new(ReadFile),
            Box::new(ListDir),
            Box::new(EditFile),
            Box::new(WriteFile),
            Box::new(RunCommand),
        ])
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        self.tools.iter().map(|tool| tool.name())
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(AsRef::as_ref)
    }

    /// The block appended to the system text. Byte-stable across calls, across
    /// processes, and across the order the tools were declared in.
    pub fn definitions(&self) -> String {
        if self.tools.is_empty() {
            return String::new();
        }

        let mut text = String::from(PREAMBLE);
        for tool in &self.tools {
            text.push_str(&format!(
                "\n{} — {}\n{}\n",
                tool.name(),
                tool.description(),
                serde_json::to_string(&tool.parameters())
                    .expect("a schema built from json! always serializes"),
            ));
        }
        text
    }

    /// Runs a call, or says why it did not.
    pub async fn call(&self, call: &ToolCall, sandbox: &Sandbox) -> ToolOutcome {
        match self.get(&call.name) {
            Some(tool) => tool.run(&call.arguments, sandbox).await,
            None => ToolOutcome::denied(Verdict::deny(format!(
                "there is no tool called `{}`; the tools are {}",
                call.name,
                self.names().collect::<Vec<_>>().join(", ")
            ))),
        }
    }
}

/// The instructions, kept next to the rendering they are part of. Every byte
/// here is in the cached prefix and in every prompt.
const PREAMBLE: &str = "\
# Tools

To use one, end your reply with a fenced block and nothing after it:

```tool
{\"name\": \"read_file\", \"arguments\": {\"path\": \"src/main.rs\"}}
```

The result comes back as the next message. One call per reply. When no tool is
needed, answer in plain text and do not emit a block.
";

/// Reads a tool call out of what the model produced.
///
/// The one function that changes when the backend learns native function
/// calling; nothing above it does. It accepts the fenced form the preamble asks
/// for, and a bare JSON object, because a 7B that has never seen a tool API
/// drops the fence about a third of the time and refusing that would be
/// measuring the fence rather than the loop.
pub fn parse_call(text: &str) -> Option<ToolCall> {
    fenced(text)
        .and_then(|body| serde_json::from_str::<ToolCall>(body).ok())
        .or_else(|| bare_object(text))
        .filter(|call| !call.name.is_empty())
}

/// The body of the first ```tool block, if the block is closed. An unclosed one
/// is a call still being generated, and half a call is not a call.
fn fenced(text: &str) -> Option<&str> {
    let open = text.find("```tool")?;
    let body = &text[open + "```tool".len()..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    let close = body.find("```")?;
    Some(body[..close].trim())
}

/// The first `{…}` that parses as a call. Scanned from every `{` rather than
/// from the first, because models like to open with prose containing braces.
fn bare_object(text: &str) -> Option<ToolCall> {
    let bytes = text.as_bytes();
    for (start, _) in text
        .char_indices()
        .filter(|(i, c)| *c == '{' && bytes.get(i.wrapping_sub(1)).is_none_or(|b| *b != b'{'))
    {
        let mut stream = serde_json::Deserializer::from_str(&text[start..]).into_iter::<ToolCall>();
        if let Some(Ok(call)) = stream.next() {
            return Some(call);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fenced_call_parses() {
        let call = parse_call(
            "I will look at the file.\n\n```tool\n{\"name\": \"read_file\", \
             \"arguments\": {\"path\": \"src/main.rs\"}}\n```\n",
        )
        .unwrap();
        assert_eq!(call.name, "read_file");
        assert_eq!(call.arguments["path"], "src/main.rs");
    }

    #[test]
    fn a_call_without_the_fence_still_parses() {
        let call =
            parse_call("Sure. {\"name\":\"list_dir\",\"arguments\":{\"path\":\".\"}}").unwrap();
        assert_eq!(call.name, "list_dir");
    }

    #[test]
    fn prose_with_braces_before_the_call_does_not_stop_it_being_found() {
        let call =
            parse_call("The struct is `Foo { bar }`, so: {\"name\":\"list_dir\",\"arguments\":{}}")
                .unwrap();
        assert_eq!(call.name, "list_dir");
    }

    #[test]
    fn an_unclosed_block_is_not_a_call() {
        // It is a call still being generated. Executing half of one is how a
        // streaming agent runs something nobody asked for.
        assert!(parse_call("```tool\n{\"name\": \"run_command\"").is_none());
    }

    #[test]
    fn plain_prose_is_an_answer_and_not_a_call() {
        assert!(parse_call("The file defines one function, `main`.").is_none());
    }

    #[test]
    fn the_definitions_are_byte_identical_however_the_set_was_declared() {
        // The cached prefix is a wire format. Two processes that built the same
        // tool set differently must send the same bytes.
        let one = Tools::new(vec![Box::new(ReadFile), Box::new(ListDir)]).definitions();
        let other = Tools::new(vec![Box::new(ListDir), Box::new(ReadFile)]).definitions();
        assert_eq!(one, other);
        assert_eq!(
            one,
            Tools::new(vec![Box::new(ReadFile), Box::new(ListDir)]).definitions()
        );
    }

    #[test]
    fn schema_keys_serialize_in_a_fixed_order() {
        // This is the guard against a transitive dependency turning on
        // `serde_json/preserve_order`: the maps would become insertion-ordered,
        // every schema would render in declaration order, and the only symptom
        // would be a prompt cache that quietly stopped hitting.
        let forwards = serde_json::json!({"a": 1, "b": 2, "c": 3});
        let backwards = serde_json::json!({"c": 3, "b": 2, "a": 1});
        assert_eq!(
            serde_json::to_string(&forwards).unwrap(),
            serde_json::to_string(&backwards).unwrap(),
        );
    }

    #[test]
    fn the_standard_set_lists_every_tool_it_documents() {
        let tools = Tools::standard();
        let definitions = tools.definitions();
        for name in tools.names() {
            assert!(definitions.contains(name), "{name} is not in the prefix");
        }
        assert!(tools.get("no_such_tool").is_none());
    }

    #[test]
    fn output_is_cut_at_a_character_boundary_and_says_so() {
        let outcome = ToolOutcome::ok(
            Verdict::allow("test", crate::sandbox::Applied::Process),
            "ñ".repeat(MAX_OUTPUT_BYTES),
        );
        assert!(outcome.truncated);
        assert!(outcome.output.len() <= MAX_OUTPUT_BYTES);
        assert!(outcome.render("read_file").contains("cut at"));
    }
}
