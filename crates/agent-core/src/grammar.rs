//! A GBNF grammar compiled from the same `Tool::parameters()` schemas the
//! prompt's own tool-definitions block already carries — reusing them rather
//! than writing a second description of a tool's shape, for the reason
//! [`crate::tools::fenced`]'s own comment gives about `parse_call` and
//! `task::parse_plan`: two descriptions would drift, and the drift would be
//! invisible until a call was refused.
//!
//! Scoped to the JSON Schema subset this project's own tools actually use —
//! `string`, `integer`, `array` of `string`, and `object` with `properties`
//! and `required` (in whatever order `serde_json`'s sorted map gives them,
//! which is not the order the schema was written in — see
//! [`Compiler::object_rule`]) — because that is what every tool in
//! [`Tools::standard`] emits. A schema outside that scope fails
//! [`compile`] rather than compiling to a grammar that accepts the wrong
//! thing.
//!
//! See `RECORD/2026-09-06.a-grammar-for-tool-calls.WIP.md`, and
//! `RECORD/2026-09-14.the-bare-grammar-field.completed.md` for why this is
//! worth compiling at all: `llama-server`'s own `/v1/chat/completions` takes
//! a bare `grammar` field and enforces it.

use serde_json::Value;

use crate::tools::Tools;

/// A tool's schema used something this compiler does not cover. Named rather
/// than guessed at: a tool added later with, say, a nested object argument
/// fails here, loudly, instead of silently compiling a grammar that accepts
/// anything for that field.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{tool}'s schema is outside the grammar compiler's scope: {reason}")]
pub struct GrammarError {
    pub tool: String,
    pub reason: String,
}

/// The fence the preamble asks for — `tools/mod.rs`'s `PREAMBLE` constant —
/// spelled out once here too. Pinned against drift by
/// [`tests::the_fence_and_the_preamble_agree_on_the_tag`] rather than by a
/// shared constant, because the preamble's copy is prose baked into a
/// bigger string and this one is a bare tag; a test that reads both is what
/// keeps them honest, the same role `select_probe` plays for the map.
const FENCE_TAG: &str = "tool";

/// Compiles a grammar for `root ::= prose | call`: free-form prose, or one
/// call to one of `tools`, fenced exactly the way the preamble asks for.
///
/// **Measured, on the standalone `llama-server` this project already
/// proved honours a bare `grammar` field, to change nothing: this
/// alternation is the same language as `prose` alone.** `prose` is
/// `[^\x00]*` — every non-NUL string — and every string `call` can produce
/// is also a non-NUL string, so `call`'s language is a strict subset of
/// `prose`'s. `root ::= prose | call` is therefore set-theoretically
/// identical to `root ::= prose`: the `call` branch adds nothing a sampler
/// could not already reach through `prose`, because there is nothing
/// outside `prose` for it to add. Sending the fifteen prompts of
/// `scripts/tasks/tool-call-probe.txt` through this grammar at temperature
/// 0 reproduced the unconstrained run's replies **byte for byte**, verdict
/// for verdict — see `RECORD/2026-09-14.the-alternation-that-was-not-one.completed.md`.
/// The compiler is still correct and still worth having (every rule it
/// builds parses on a live server and forces the exact shape it names, as
/// `tests` and that record's own isolated checks show); what does not work
/// is wrapping it in a "prose, or" alternative and expecting the "or" to
/// mean anything.
pub fn compile(tools: &Tools) -> Result<String, GrammarError> {
    let mut compiler = Compiler::default();
    let ws = compiler.ws();
    let name_key = json_literal("name");
    let arguments_key = json_literal("arguments");

    let mut call_rules = Vec::new();
    for name in tools.names() {
        let tool = tools
            .get(name)
            .expect("names() only yields tools that exist");
        let arguments = compiler.object_rule(name, &tool.parameters())?;
        let name_value = json_literal(name);
        let body = format!(
            "{name_key} \":\" {ws} {name_value} \",\" {ws} {arguments_key} \":\" {ws} {arguments}"
        );
        call_rules.push(compiler.define(format!("{}-call", rule_name(name)), body));
    }

    let one_of_calls = call_rules.join(" | ");
    let tool_call = compiler.define(
        "tool-call".into(),
        format!("\"{{\" {ws} ( {one_of_calls} ) {ws} \"}}\""),
    );
    let fence_open = literal(&format!("```{FENCE_TAG}"));
    let fence_close = literal("```");
    let call = compiler.define(
        "call".into(),
        format!("{fence_open} \"\\n\" {tool_call} \"\\n\" {fence_close}"),
    );
    // Any text at all, including empty — the "answer in plain text" half of
    // the preamble's own alternation. `[^\x00]` rather than `.`: GBNF has no
    // wildcard, and excluding only NUL is the same "anything" a person means
    // by it in practice.
    compiler.define("prose".into(), "[^\\x00]*".into());
    compiler.define("root".into(), format!("prose | {call}"));

    Ok(compiler.render())
}

#[derive(Default)]
struct Compiler {
    rules: Vec<(String, String)>,
}

impl Compiler {
    /// Defines a fresh, uniquely-named rule and returns its name. `compile`
    /// never reuses a name across two different tools' rules — each is
    /// prefixed with the tool's own name — so this never collides.
    fn define(&mut self, name: String, body: String) -> String {
        debug_assert!(
            !self.rules.iter().any(|(n, _)| *n == name),
            "rule {name} defined twice"
        );
        self.rules.push((name.clone(), body));
        name
    }

    /// Defines a rule the first time it is asked for and reuses it after —
    /// `ws`, `string`, `integer`, `string-array` and `empty` are the same
    /// rule for every tool that needs one, and writing a copy per tool would
    /// be the "two descriptions of the same shape" problem this module's own
    /// doc warns about, one level down.
    fn ensure(&mut self, name: &str, body: impl FnOnce() -> String) -> String {
        if !self.rules.iter().any(|(n, _)| n == name) {
            let body = body();
            self.rules.push((name.to_string(), body));
        }
        name.to_string()
    }

    fn ws(&mut self) -> String {
        self.ensure("ws", || "[ \\t\\n]*".into())
    }

    /// The empty match — GBNF's own `""` rather than an inlined bare empty
    /// string, because `(  ) | ( x )` (an empty group) is not the same thing
    /// as `( "" ) | ( x )` and only one of those is grammar every parser is
    /// certain to accept.
    fn empty(&mut self) -> String {
        self.ensure("empty", || "\"\"".into())
    }

    /// A JSON string, adapted from `llama.cpp`'s own `grammars/json.gbnf` —
    /// proven syntax rather than a second guess at what the parser accepts.
    fn string(&mut self) -> String {
        self.ensure("string", || {
            "\"\\\"\" ( [^\"\\\\\\x7F\\x00-\\x1F] | \"\\\\\" ( [\"\\\\bfnrt] | \"u\" [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] [0-9a-fA-F] ) )* \"\\\"\""
                .into()
        })
    }

    /// Only what this project's tools ever declare an integer for: a
    /// timeout, a line number. No exponent, no leading zero.
    fn integer(&mut self) -> String {
        self.ensure("integer", || "\"-\"? ( \"0\" | [1-9] [0-9]* )".into())
    }

    fn string_array(&mut self) -> String {
        let ws = self.ws();
        let string = self.string();
        self.ensure("string-array", || {
            format!("\"[\" {ws} ( {string} {ws} ( \",\" {ws} {string} {ws} )* )? \"]\"")
        })
    }

    /// The rule for one property's value. `tool` is the name the error
    /// names, not part of the rule — a `read_file` and a future tool that
    /// both take a plain `string` share the one `string` rule.
    fn value_rule(&mut self, tool: &str, schema: &Value) -> Result<String, GrammarError> {
        match schema.get("type").and_then(Value::as_str) {
            Some("string") => Ok(self.string()),
            Some("integer") => Ok(self.integer()),
            Some("array") => match schema
                .get("items")
                .and_then(|items| items.get("type"))
                .and_then(Value::as_str)
            {
                Some("string") => Ok(self.string_array()),
                other => Err(GrammarError {
                    tool: tool.to_string(),
                    reason: format!("an array of {other:?} — only an array of string is in scope"),
                }),
            },
            other => Err(GrammarError {
                tool: tool.to_string(),
                reason: format!("a property of type {other:?}, which is out of scope"),
            }),
        }
    }

    /// Compiles `schema`'s `object` shape into a rule matching that object
    /// literally, `{"key": value, …}`, in **`serde_json`'s own property
    /// order** — the sorted-map order [`crate::tools::Tools::definitions`]
    /// already commits this project to, not the order `json!` was written
    /// in. A property this project's tools declare as optional stays
    /// optional *at its own sorted position*: a required property that
    /// sorts after an optional one (`read_file`'s `path` sorts after
    /// `max_lines`) still has to appear whether or not the optional one
    /// before it does, which is why this builds two families of helper
    /// rules — `tail(i)`, everything from property `i` given something
    /// already came before it, and `head(i)`, the same thing given nothing
    /// has — rather than one linear optional chain.
    fn object_rule(&mut self, tool: &str, schema: &Value) -> Result<String, GrammarError> {
        let required: Vec<&str> = schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();

        let mut props: Vec<(String, String, bool)> = Vec::new();
        if let Some(map) = schema.get("properties").and_then(Value::as_object) {
            for (key, value_schema) in map {
                let rule = self.value_rule(tool, value_schema)?;
                props.push((key.clone(), rule, required.contains(&key.as_str())));
            }
        }

        let ws = self.ws();
        let empty = self.empty();
        let base = format!("{}-arguments", rule_name(tool));
        let n = props.len();

        let mut tail_ref = vec![empty.clone(); n + 1];
        for i in (0..n).rev() {
            let (key, rule, is_required) = &props[i];
            let seg = format!("\",\" {ws} {} {ws} \":\" {ws} {rule}", json_literal(key));
            let next = &tail_ref[i + 1];
            let body = match is_required {
                true => format!("{seg} {next}"),
                false => format!("( {seg} )? {next}"),
            };
            tail_ref[i] = self.define(format!("{base}-tail-{i}"), body);
        }

        let mut head_ref = vec![empty; n + 1];
        for i in (0..n).rev() {
            let (key, rule, is_required) = &props[i];
            let seg_first = format!("{} {ws} \":\" {ws} {rule}", json_literal(key));
            let tail_next = &tail_ref[i + 1];
            let include = format!("{seg_first} {tail_next}");
            let body = match is_required {
                true => include,
                false => {
                    let skip = &head_ref[i + 1];
                    format!("( {include} ) | ( {skip} )")
                }
            };
            head_ref[i] = self.define(format!("{base}-head-{i}"), body);
        }

        let head = &head_ref[0];
        Ok(self.define(base, format!("\"{{\" {ws} {head} {ws} \"}}\"")))
    }

    fn render(&self) -> String {
        self.rules
            .iter()
            .map(|(name, body)| format!("{name} ::= {body}\n"))
            .collect()
    }
}

/// A GBNF string literal matching exactly `text`, escaped the way the
/// parser's own string literals are.
fn literal(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// A GBNF literal matching `text` **as a JSON string**, quotes included —
/// `json_literal("path")` matches the six characters `"path"`, not the four
/// characters `path`. Every property key and every tool name is JSON text,
/// never a bare identifier, and [`literal`] alone would match the wrong
/// thing for both.
fn json_literal(text: &str) -> String {
    literal(&format!("\"{text}\""))
}

/// A tool name, safe to use as a **rule identifier** rather than as matched
/// text. `llama-server`'s own GBNF parser rejects `_` in a rule name outright
/// — `list_dir` alone, with no other content, fails to parse — found by
/// bisecting a real `failed to parse grammar` response one rule at a time
/// against the live server, not documented anywhere this project found. Every
/// one of this project's tool names has an underscore, so every rule this
/// module names after one needs this; the JSON *text* `"list_dir"` a call must
/// still contain is untouched — that is [`json_literal`]'s job, not this
/// one's.
fn rule_name(tool: &str) -> String {
    tool.replace('_', "-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::Tools;

    #[test]
    fn compiles_every_standard_tool() {
        let grammar = compile(&Tools::standard()).expect("every standard tool is in scope");
        assert!(grammar.contains("root ::="));
        // Rule *names* are hyphenated (`read-file-call`), never the tool's
        // own underscored spelling — see `rule_name`. The matched JSON text
        // stays `"read_file"`, checked separately below.
        assert!(grammar.contains("read-file-call ::="));
        assert!(grammar.contains("list-dir-call ::="));
        assert!(grammar.contains("run-command-call ::="));
        assert!(grammar.contains("edit-file-call ::="));
        assert!(grammar.contains("write-file-call ::="));
        assert!(grammar.contains("read_file"), "{grammar}");
        // Shared rules are written once, not once per tool.
        assert_eq!(grammar.matches("\nstring ::=").count(), 1, "{grammar}");
        assert_eq!(grammar.matches("\ninteger ::=").count(), 1, "{grammar}");
        assert_eq!(grammar.matches("\nempty ::=").count(), 1, "{grammar}");
    }

    #[test]
    fn the_fence_and_the_preamble_agree_on_the_tag() {
        let grammar = compile(&Tools::standard()).unwrap();
        assert!(grammar.contains("```tool"), "{grammar}");
        assert!(
            Tools::standard().definitions().contains("```tool"),
            "the preamble's own fence"
        );
    }

    #[test]
    fn root_keeps_the_alternation() {
        let grammar = compile(&Tools::standard()).unwrap();
        assert!(grammar.contains("root ::= prose | call"), "{grammar}");
    }

    #[test]
    fn json_literal_keeps_the_quotes() {
        assert_eq!(json_literal("path"), "\"\\\"path\\\"\"");
    }

    #[test]
    fn a_required_property_that_sorts_after_optional_ones_is_still_mandatory() {
        // read_file's schema, sorted: max_lines (optional), path (required),
        // start_line (optional) — `path` is index 1. Its own rule must not be
        // wrapped `( … )?`, which is how this compiler spells "optional",
        // even though it sorts between two optional properties.
        let parameters = Tools::standard().get("read_file").unwrap().parameters();
        let mut compiler = Compiler::default();
        compiler.object_rule("read_file", &parameters).unwrap();
        let (_, tail_1) = compiler
            .rules
            .iter()
            .find(|(name, _)| name == "read-file-arguments-tail-1")
            .expect("path is index 1");
        assert!(!tail_1.trim_start().starts_with('('), "{tail_1}");
    }

    #[test]
    fn an_out_of_scope_schema_fails_to_compile_instead_of_compiling_wrong() {
        let mut compiler = Compiler::default();
        let schema = serde_json::json!({
            "type": "object",
            "properties": { "nested": { "type": "object", "properties": {} } },
            "required": ["nested"],
        });
        let error = compiler.object_rule("hypothetical", &schema).unwrap_err();
        assert_eq!(error.tool, "hypothetical");
        assert!(error.reason.contains("\"object\""), "{error}");
    }

    #[test]
    fn list_dir_s_only_property_is_fully_optional() {
        let parameters = Tools::standard().get("list_dir").unwrap().parameters();
        let mut compiler = Compiler::default();
        let arguments = compiler.object_rule("list_dir", &parameters).unwrap();
        let (_, body) = compiler
            .rules
            .iter()
            .find(|(name, _)| *name == arguments)
            .unwrap();
        // The object rule itself must accept the empty object: head(0) is
        // reachable via its "skip" branch straight to `empty`.
        assert!(body.contains("list-dir-arguments-head-0"), "{body}");
    }
}
