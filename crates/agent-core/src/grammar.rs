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
//! See `RECORD/2026-09-06.a-grammar-for-tool-calls.completed.md`, and
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

/// Compiles a grammar whose language is: any text that does **not** contain
/// the literal substring `` ```tool `` — followed, *if and only if it does
/// occur*, by nothing but a well-formed call and the closing fence, full
/// stop.
///
/// **The first version of this function compiled `root ::= prose | call`,
/// with `prose` a true wildcard (`[^\x00]*`) — measured, on a live server, to
/// change nothing at all**, because `call`'s language was a subset of
/// `prose`'s and a subset adds nothing to a union it is already inside of.
/// Sending the fifteen prompts of `scripts/tasks/tool-call-probe.txt`
/// through that version reproduced the unconstrained run's replies byte for
/// byte, verdict for verdict — see
/// `RECORD/2026-09-14.the-alternation-that-was-not-one.completed.md`. This
/// version closes that gap the only way a kept alternative *can* narrow
/// anything: `prose` no longer contains `call`'s trigger substring, so the
/// two languages are disjoint rather than nested, via
/// [`Compiler::avoid`].
///
/// **And a fence under a tool's own name is never typed.** A reply fenced
/// `` ```read_file `` instead ([`crate::tools::CallVerdict::Drifted`]'s own
/// shape) never contained the trigger, so until 2026-11-09 it was ordinary
/// prose here. Now every `` ```<tool> `` is avoided in the same automaton as
/// the trigger, and its last character is not offered: the model writes the
/// correct fence, or another word, or no fence. Forbidden rather than forced
/// toward a call, because the drifted fence's body is not the call's shape —
/// it is `{"path": …}` without the name, and a grammar that forced the name in
/// after the fact would be guessing which tool was meant.
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
    let fence_close = literal("```");
    // Everything a reply needs *after* the trigger substring has been
    // typed: the mandatory newline, the call itself, the mandatory closing
    // fence — and then nothing, because this is where `root`'s derivation
    // ends. Nothing follows a completed call; `CallVerdict::ContinuedPastFence`
    // has no grammatical shape left to occupy.
    let call_tail = compiler.define(
        "call-tail".into(),
        format!("\"\\n\" {tool_call} \"\\n\" {fence_close}"),
    );
    // The one fence forces a call; a fence under a tool's own name — the
    // drift `CallVerdict::Drifted` names, two of fifteen probe prompts in both
    // arms — is never completed. Roadmap row 7; see
    // `RECORD/2026-11-09.a-tool-s-own-fence-is-never-typed.completed.md`.
    let fence = format!("```{FENCE_TAG}");
    let drifts: Vec<String> = tools
        .names()
        .filter(|name| !name.starts_with(FENCE_TAG))
        .map(|name| format!("```{name}"))
        .collect();
    let mut patterns = vec![(fence.as_str(), End::Forced(&call_tail))];
    patterns.extend(drifts.iter().map(|d| (d.as_str(), End::Forbidden)));
    let safe0 = compiler.avoid("safe", &patterns);
    compiler.define("root".into(), safe0);

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

    /// Builds `pattern.chars().count()` mutually-recursive rules —
    /// `{prefix}-0` .. `{prefix}-{n-1}` — whose union is: any string that
    /// does not contain `pattern` as a substring, **except** that the
    /// moment a prefix of `pattern` is extended into a full match, the only
    /// continuation is `tail_rule` — not "back to safe text", not nothing.
    /// Returns `{prefix}-0`, the rule matching from the empty string.
    ///
    /// This is the standard substring-avoidance construction — a KMP
    /// automaton over `pattern`, with the accepting (fully-matched) state
    /// replaced by a forced hand-off to `tail_rule` instead of a dead end —
    /// built generically rather than by hand for `` ```tool `` specifically,
    /// because the alternative is re-deriving the failure function by hand
    /// for every fence this project ever adds, silently wrong the first time
    /// the pattern self-overlaps in a way `` ```tool `` happens not to.
    ///
    /// Kept for the tests that pin that construction state for state; the
    /// grammar itself is built by [`Self::avoid`], of which this is the
    /// one-pattern case.
    #[cfg(test)]
    fn avoid_until_forced(&mut self, prefix: &str, pattern: &str, tail_rule: &str) -> String {
        self.avoid(prefix, &[(pattern, End::Forced(tail_rule))])
    }

    /// `avoid_until_forced` over several patterns at once: text that
    /// contains none of them, where completing one either hands off to its
    /// tail rule ([`End::Forced`]) or cannot happen at all
    /// ([`End::Forbidden`] — the character that would complete it is not
    /// offered). One automaton for all of them, because two automata cannot
    /// both own the same stream of characters: [`automaton`] is Aho-Corasick,
    /// of which the single-pattern case is KMP, state for state.
    fn avoid(&mut self, prefix: &str, patterns: &[(&str, End<'_>)]) -> String {
        let states = automaton(patterns);
        let state_name = |i: usize| format!("{prefix}-{i}");
        for (i, edges) in states.iter().enumerate() {
            let mut branches = vec!["\"\"".to_string()];
            let mut excluded = String::new();
            for (c, edge) in edges {
                excluded.push_str(&class_char(*c));
                let dest = match edge {
                    Edge::To(next) => state_name(*next),
                    Edge::Ends(End::Forced(tail)) => tail.to_string(),
                    // Excluded and offered nowhere: the one character that
                    // would complete it is the one this state cannot take.
                    Edge::Ends(End::Forbidden) => continue,
                };
                branches.push(format!("( {} {dest} )", literal(&c.to_string())));
            }
            branches.push(format!("( [^\\x00{excluded}] {} )", state_name(0)));
            self.define(state_name(i), branches.join(" | "));
        }
        state_name(0)
    }

    fn render(&self) -> String {
        self.rules
            .iter()
            .map(|(name, body)| format!("{name} ::= {body}\n"))
            .collect()
    }
}

/// What completing a pattern in [`Compiler::avoid`] leads to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End<'a> {
    /// Only this rule may follow: the pattern is the start of something
    /// well-formed, and nothing else is.
    Forced(&'a str),
    /// Nothing may: the pattern is never completed.
    Forbidden,
}

/// Where a character takes a state of [`automaton`]. A character a state does
/// not list goes back to state 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge<'a> {
    To(usize),
    Ends(End<'a>),
}

/// The substring-avoidance automaton over `patterns`: one state per proper
/// prefix of a pattern (state 0 the empty one, the rest in breadth-first
/// order, which for one pattern is *matched `i` characters*), and for each
/// state the characters that lead anywhere but back to state 0.
///
/// Aho-Corasick: a trie of the patterns with failure links, and each state's
/// transitions completed through them, so a partial match that turns out to
/// be the start of another pattern — `` ```ru `` on the way to `` ```run_command ``
/// that becomes `` ```rust `` — falls back to the longest prefix still alive
/// rather than to nothing. A transition that completes a pattern, directly or
/// as a suffix of what was typed, is that pattern's [`End`].
fn automaton<'a>(patterns: &[(&str, End<'a>)]) -> Vec<Vec<(char, Edge<'a>)>> {
    assert!(
        patterns.iter().all(|(p, _)| !p.is_empty()),
        "every pattern to avoid needs a character"
    );
    // The trie: children, and the pattern a node completes.
    let mut children: Vec<Vec<(char, usize)>> = vec![Vec::new()];
    let mut ends: Vec<Option<End<'a>>> = vec![None];
    for (pattern, end) in patterns {
        let mut node = 0;
        for c in pattern.chars() {
            node = match children[node].iter().find(|(k, _)| *k == c) {
                Some((_, next)) => *next,
                None => {
                    children.push(Vec::new());
                    ends.push(None);
                    let next = children.len() - 1;
                    children[node].push((c, next));
                    next
                }
            };
        }
        ends[node].get_or_insert(*end);
    }
    // Failure links, breadth first, and each node's end inherited from the
    // longest suffix that completes a pattern.
    let mut fail = vec![0usize; children.len()];
    let mut order = Vec::new();
    let mut queue: std::collections::VecDeque<usize> =
        children[0].iter().map(|(_, n)| *n).collect();
    while let Some(node) = queue.pop_front() {
        order.push(node);
        for &(c, child) in &children[node].clone() {
            let mut f = fail[node];
            let target = loop {
                if let Some((_, next)) = children[f].iter().find(|(k, _)| *k == c) {
                    break *next;
                }
                if f == 0 {
                    break 0;
                }
                f = fail[f];
            };
            fail[child] = if target == child { 0 } else { target };
            if ends[child].is_none() {
                ends[child] = ends[fail[child]];
            }
            queue.push_back(child);
        }
    }
    // The states: the root and every node that completes nothing, numbered
    // in breadth-first order.
    let live: Vec<usize> = std::iter::once(0)
        .chain(order.into_iter().filter(|n| ends[*n].is_none()))
        .collect();
    let number = |node: usize| live.iter().position(|n| *n == node);
    let step = |mut node: usize, c: char| loop {
        if let Some((_, next)) = children[node].iter().find(|(k, _)| *k == c) {
            return *next;
        }
        if node == 0 {
            return 0;
        }
        node = fail[node];
    };
    live.iter()
        .map(|&node| {
            // The characters worth listing: the children of this node and of
            // every node on its failure chain; any other goes back to 0.
            let mut chars: Vec<char> = Vec::new();
            let mut f = node;
            loop {
                for (c, _) in &children[f] {
                    if !chars.contains(c) {
                        chars.push(*c);
                    }
                }
                if f == 0 {
                    break;
                }
                f = fail[f];
            }
            chars
                .into_iter()
                .filter_map(|c| {
                    let next = step(node, c);
                    match ends[next] {
                        Some(end) => Some((c, Edge::Ends(end))),
                        None if next == 0 => None,
                        None => Some((c, Edge::To(number(next).expect("a live node")))),
                    }
                })
                .collect()
        })
        .collect()
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

/// One character, escaped for a `[...]` character class rather than for a
/// `"..."` string literal — the two escaping rules are not the same (a
/// class only needs `]` and `\` escaped; a bare `"` is fine unescaped
/// inside brackets), and using [`literal`]'s escapes here would be a
/// second, subtly different definition of "escaped" that happened to agree
/// for every character this compiler has needed so far.
fn class_char(c: char) -> String {
    match c {
        ']' => "\\]".to_string(),
        '\\' => "\\\\".to_string(),
        other => other.to_string(),
    }
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
    fn the_fence_no_longer_appears_as_one_contiguous_literal() {
        // The whole point of `avoid_until_forced`: the trigger is split
        // into one character per state, so a partial match can fall back
        // to safe text instead of being a single token a model either
        // types whole or not at all.
        let grammar = compile(&Tools::standard()).unwrap();
        assert!(!grammar.contains("```tool"), "{grammar}");
        assert!(grammar.contains("safe-0"), "{grammar}");
        assert!(grammar.contains("call-tail"), "{grammar}");
        assert!(
            Tools::standard().definitions().contains("```tool"),
            "the preamble's own fence is still one literal — only the grammar's is split"
        );
    }

    #[test]
    fn root_is_the_automaton_s_start_state() {
        let grammar = compile(&Tools::standard()).unwrap();
        assert!(grammar.contains("root ::= safe-0"), "{grammar}");
    }

    #[test]
    fn avoid_until_forced_builds_one_state_per_character() {
        let mut compiler = Compiler::default();
        let start = compiler.avoid_until_forced("t", "ab", "TAIL");
        assert_eq!(start, "t-0");
        let names: Vec<&str> = compiler.rules.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["t-0", "t-1"]);
    }

    #[test]
    fn a_full_match_hands_off_to_tail_not_back_to_the_start() {
        // "ab": state 1 has matched "a"; on 'b' it completes the pattern and
        // must hand off to TAIL, not loop back into safe text.
        let mut compiler = Compiler::default();
        compiler.avoid_until_forced("t", "ab", "TAIL");
        let (_, state1) = compiler.rules.iter().find(|(n, _)| n == "t-1").unwrap();
        assert!(state1.contains("\"b\" TAIL"), "{state1}");
        assert!(!state1.contains("\"b\" t-0"), "{state1}");
    }

    #[test]
    fn a_self_overlapping_pattern_falls_back_through_the_kmp_chain() {
        // "aab": state 2 has matched "aa". A third 'a' does not restart at
        // state 0 — the longest prefix of "aab" that is a suffix of "aaa"
        // is "aa" again, so it must stay at state 2 (self-loop), and 'b'
        // still completes the pattern.
        let mut compiler = Compiler::default();
        compiler.avoid_until_forced("t", "aab", "TAIL");
        let (_, state2) = compiler.rules.iter().find(|(n, _)| n == "t-2").unwrap();
        assert!(state2.contains("\"a\" t-2"), "{state2}");
        assert!(state2.contains("\"b\" TAIL"), "{state2}");
    }

    #[test]
    fn a_partial_match_that_also_overlaps_falls_back_to_the_right_state() {
        // "abab": state 3 has matched "aba". On another 'a' (making
        // "abaa"), the longest prefix of "abab" that is a suffix of "abaa"
        // is "a" (length 1) — fall back to state 1, not state 0. On 'b' it
        // completes the pattern.
        let mut compiler = Compiler::default();
        compiler.avoid_until_forced("t", "abab", "TAIL");
        let (_, state3) = compiler.rules.iter().find(|(n, _)| n == "t-3").unwrap();
        assert!(state3.contains("\"a\" t-1"), "{state3}");
        assert!(state3.contains("\"b\" TAIL"), "{state3}");
    }

    #[test]
    fn every_state_may_end_the_string_right_there() {
        let mut compiler = Compiler::default();
        compiler.avoid_until_forced("t", "ab", "TAIL");
        for (name, body) in &compiler.rules {
            assert!(
                body.contains("\"\""),
                "{name} has no way to just stop: {body}"
            );
        }
    }

    /// Runs `text` through an [`automaton`]: where it ends up, or the end
    /// that stopped it.
    fn walk<'a>(states: &[Vec<(char, Edge<'a>)>], text: &str) -> Result<usize, End<'a>> {
        let mut state = 0;
        for c in text.chars() {
            state = match states[state].iter().find(|(k, _)| *k == c) {
                Some((_, Edge::To(next))) => *next,
                Some((_, Edge::Ends(end))) => return Err(*end),
                None => 0,
            };
        }
        Ok(state)
    }

    /// Row 7: one automaton, the trigger forced and a tool's own fence never
    /// completed, and everything else — another language's fence, a fence
    /// that starts like a tool's and turns away — still prose.
    #[test]
    fn a_tool_s_own_fence_is_never_completed_and_other_fences_are_prose() {
        let states = automaton(&[
            ("```tool", End::Forced("TAIL")),
            ("```read_file", End::Forbidden),
            ("```run_command", End::Forbidden),
        ]);
        assert_eq!(walk(&states, "a call:\n```tool"), Err(End::Forced("TAIL")));
        assert_eq!(walk(&states, "here:\n```read_file"), Err(End::Forbidden));
        assert_eq!(walk(&states, "``````run_command"), Err(End::Forbidden));
        for prose in [
            "```rust\nfn main() {}\n```",
            "```read_fil",
            "```ru",
            "``` tool",
        ] {
            assert!(walk(&states, prose).is_ok(), "{prose:?} is prose");
        }
        // A longer word that starts with the fence still has it as a prefix.
        assert_eq!(walk(&states, "```read_fileish"), Err(End::Forbidden));
        // `` ```ru `` was on its way to `run_command`; `s` turns it into
        // `rust`, and the automaton is back at the start, not stuck.
        assert_eq!(walk(&states, "```rus"), Ok(0));
    }

    /// The grammar it renders: every standard tool's fence is excluded from
    /// prose at its last character, and the trigger still hands off.
    #[test]
    fn the_compiled_grammar_avoids_every_tool_s_own_fence() {
        let grammar = compile(&Tools::standard()).unwrap();
        for name in Tools::standard().names() {
            assert!(
                !grammar.contains(&format!("```{name}")),
                "{name}: {grammar}"
            );
        }
        assert!(grammar.contains("call-tail"), "{grammar}");
        // The state one character short of `` ```list_dir `` offers no `r`,
        // neither as a branch nor through its catch-all.
        let rule = grammar_rule_for(&grammar, "```list_di");
        assert!(!rule.contains("( \"r\" "), "{rule}");
        assert!(rule.contains("[^\\x00r"), "{rule}");
    }

    /// Every name a rule refers to is a rule, once literals and classes are
    /// taken out: what a GBNF parser checks first, here where no server is.
    #[test]
    fn every_rule_the_grammar_names_is_defined() {
        let grammar = compile(&Tools::standard()).unwrap();
        let defined: std::collections::HashSet<&str> = grammar
            .lines()
            .filter_map(|line| line.split_once(" ::= ").map(|(name, _)| name))
            .collect();
        for line in grammar.lines() {
            let (_, body) = line.split_once(" ::= ").unwrap();
            let mut bare = String::new();
            let mut chars = body.chars();
            while let Some(c) = chars.next() {
                let close = match c {
                    '"' => '"',
                    '[' => ']',
                    other => {
                        bare.push(other);
                        continue;
                    }
                };
                // Through the closing quote or bracket, escapes skipped.
                while let Some(inner) = chars.next() {
                    if inner == '\\' {
                        chars.next();
                    } else if inner == close {
                        break;
                    }
                }
                bare.push(' ');
            }
            for word in bare.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
                if !word.is_empty() {
                    assert!(defined.contains(word), "{word} in {line}");
                }
            }
        }
    }

    /// The `safe-N` rule a prefix of the standard fences ends in.
    fn grammar_rule_for(grammar: &str, typed: &str) -> String {
        let tools = Tools::standard();
        let fence = format!("```{FENCE_TAG}");
        let drifts: Vec<String> = tools.names().map(|n| format!("```{n}")).collect();
        let mut patterns = vec![(fence.as_str(), End::Forced("call-tail"))];
        patterns.extend(drifts.iter().map(|d| (d.as_str(), End::Forbidden)));
        let state = walk(&automaton(&patterns), typed).unwrap();
        grammar
            .lines()
            .find(|line| line.starts_with(&format!("safe-{state} ::=")))
            .unwrap()
            .to_string()
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
