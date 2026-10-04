/*!
A reusable, state-based lexer construction layer.

This module provides a small framework for building tokenizers ("lexers")
on top of a [dense DFA](crate::dfa::dense::DFA). A lexer
is made up of a set of named _states_. Each state has an ordered list of
_rules_, where each rule pairs a regex pattern with an [`Action`]. An action
either emits a token (with a caller-chosen `u32` kind) or skips the matched
text, and optionally transitions to another state (by switching to it, or by
pushing/popping a state stack for recursive grammars).

At each step of lexing, the rules of the current state are tried at the
current position and the rule with the _longest_ byte match wins. If several
rules match the same number of bytes, the rule declared first wins. Rules
whose pattern can match the empty string are rejected at build time, so
lexing always makes progress. When no rule matches at the current position,
lexing stops and reports a [`LexError`] containing the byte offset and the
state. Corrupt input is never silently skipped.

# Example

This example lexes the string `sum 12` into an identifier and an integer
token, skipping whitespace. It also has a state for quoted strings.

```
use regex_automata::lexer::{Action, LexerBuilder};
use regex_automata::Span;

const IDENT: u32 = 1;
const INT: u32 = 2;
const STRING: u32 = 3;

let mut builder = LexerBuilder::new();
let main = builder.add_state("main");
let string = builder.add_state("string");

builder.rule(main, r"[a-zA-Z_][a-zA-Z0-9_]*", Action::token(IDENT));
builder.rule(main, r"[0-9]+", Action::token(INT));
builder.rule(main, r"\s+", Action::skip());
builder.rule(main, "\"", Action::skip().push(string));
builder.rule(string, "[^\"]+", Action::token(STRING));
builder.rule(string, "\"", Action::skip().pop());

let lexer = builder.build()?;
let tokens = lexer.lex(b"sum 12")?;
assert_eq!(2, tokens.len());
assert_eq!(IDENT, tokens[0].kind());
assert_eq!(Span::from(0..3), tokens[0].span());
assert_eq!(INT, tokens[1].kind());
assert_eq!(Span::from(4..6), tokens[1].span());

// An unterminated string ends lexing in a non-final state.
let err = lexer.lex(b"\"oops").unwrap_err();
assert_eq!(Some(string), err.state());
# Ok::<(), Box<dyn std::error::Error>>(())
```

# Crate features

This module is only available when both the `syntax` and `dfa-build`
features are enabled.
*/

use alloc::{string::String, string::ToString, vec::Vec};

use crate::{
    dfa::{dense, Automaton},
    util::search::{Input, MatchKind, Span},
    Anchored, MatchError,
};

/// The default maximum depth of the lexer state stack.
const DEFAULT_MAX_STATE_DEPTH: usize = 64;

/// An identifier for a lexer state.
///
/// State identifiers are returned by [`LexerBuilder::add_state`] and are
/// used to wire up transitions between states. They also appear in error
/// messages so that callers can report _where_ lexing failed.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StateId(usize);

impl StateId {
    /// Returns this state identifier as a `usize`.
    pub fn as_usize(self) -> usize {
        self.0
    }
}

/// What a lexer should do with its current state after a rule matches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transition {
    /// Stay in the current state. This is the default.
    Stay,
    /// Switch to the given state, replacing the current state.
    Switch(StateId),
    /// Push the current state onto the state stack and switch to the given
    /// state. If the stack is already at its depth limit
    /// ([`LexerBuilder::max_state_depth`]), lexing fails with a
    /// depth limit error.
    Push(StateId),
    /// Pop the state stack and switch to the popped state. If the stack is
    /// empty, lexing fails with a stack underflow error.
    Pop,
}

/// What a lexer should do when a rule matches.
///
/// An action says whether the matched text is emitted as a token (with a
/// caller-chosen `u32` kind) or skipped, and how the lexer state changes
/// afterwards. Use the constructors [`Action::token`] and [`Action::skip`]
/// and then optionally chain one of `switch`, `push` or `pop`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Action {
    emit: Option<u32>,
    transition: Transition,
}

impl Action {
    /// Emit a token with the given kind when this rule matches.
    ///
    /// The lexer stays in its current state unless a transition is chained
    /// onto this action.
    pub fn token(kind: u32) -> Action {
        Action { emit: Some(kind), transition: Transition::Stay }
    }

    /// Skip the matched text (emit no token) when this rule matches.
    ///
    /// The lexer stays in its current state unless a transition is chained
    /// onto this action.
    pub fn skip() -> Action {
        Action { emit: None, transition: Transition::Stay }
    }

    /// Switch to the given state after this rule matches.
    pub fn switch(mut self, state: StateId) -> Action {
        self.transition = Transition::Switch(state);
        self
    }

    /// Push the current state and switch to the given state after this rule
    /// matches.
    pub fn push(mut self, state: StateId) -> Action {
        self.transition = Transition::Push(state);
        self
    }

    /// Pop the state stack after this rule matches.
    pub fn pop(mut self) -> Action {
        self.transition = Transition::Pop;
        self
    }
}

/// A single token produced by a [`Lexer`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    kind: u32,
    span: Span,
}

impl Token {
    /// Returns the caller-chosen kind of this token.
    pub fn kind(&self) -> u32 {
        self.kind
    }

    /// Returns the byte span of this token in the haystack.
    pub fn span(&self) -> Span {
        self.span
    }
}

/// A builder for a state-based [`Lexer`].
///
/// States are added with [`LexerBuilder::add_state`]. The first state added
/// is the start state (unless overridden with
/// [`LexerBuilder::start_state`]) and is also the only state in which lexing
/// may finish, unless other states are explicitly marked as final with
/// [`LexerBuilder::final_state`].
///
/// Rules are added with [`LexerBuilder::rule`]. Within a state, rules are
/// tried in declaration order, and ties between matches of equal length are
/// broken in favor of the rule declared first. (Longer matches always win
/// over earlier declaration, however.)
#[derive(Clone, Debug)]
pub struct LexerBuilder {
    states: Vec<StateDef>,
    rules: Vec<(StateId, RuleDef)>,
    start: Option<StateId>,
    final_overrides: Vec<(StateId, bool)>,
    max_state_depth: usize,
    syntax: crate::util::syntax::Config,
    thompson: crate::nfa::thompson::Config,
    dfa: dense::Config,
}

#[derive(Clone, Debug)]
struct StateDef {
    name: String,
}

#[derive(Clone, Debug)]
struct RuleDef {
    pattern: String,
    action: Action,
}

impl LexerBuilder {
    /// Create a new lexer builder with a default configuration.
    pub fn new() -> LexerBuilder {
        LexerBuilder {
            states: Vec::new(),
            rules: Vec::new(),
            start: None,
            final_overrides: Vec::new(),
            max_state_depth: DEFAULT_MAX_STATE_DEPTH,
            syntax: crate::util::syntax::Config::new(),
            thompson: crate::nfa::thompson::Config::new(),
            dfa: dense::Config::new().match_kind(MatchKind::All),
        }
    }

    /// Add a new state with the given name and return its identifier.
    ///
    /// The first state added is the start state of the lexer (and the only
    /// state in which lexing may finish) unless configured otherwise.
    pub fn add_state(&mut self, name: &str) -> StateId {
        let id = StateId(self.states.len());
        self.states.push(StateDef { name: name.to_string() });
        id
    }

    /// Add a rule to a state.
    ///
    /// The `pattern` is a regular expression that is matched anchored at the
    /// current position whenever `state` is the current state. When the rule
    /// wins (longest match, then declaration order), `action` is applied.
    ///
    /// Rules are considered in the order they are declared for each state.
    pub fn rule(
        &mut self,
        state: StateId,
        pattern: &str,
        action: Action,
    ) -> &mut LexerBuilder {
        self.rules
            .push((state, RuleDef { pattern: pattern.to_string(), action }));
        self
    }

    /// Set the start state of the lexer.
    ///
    /// By default, the start state is the first state added.
    pub fn start_state(&mut self, state: StateId) -> &mut LexerBuilder {
        self.start = Some(state);
        self
    }

    /// Mark whether lexing may finish (reach the end of the haystack) in
    /// the given state.
    ///
    /// By default, only the start state is final. If lexing ends in a
    /// non-final state, an "unexpected end of input" error is returned.
    /// This is how things like unterminated strings are reported.
    pub fn final_state(
        &mut self,
        state: StateId,
        yes: bool,
    ) -> &mut LexerBuilder {
        self.final_overrides.push((state, yes));
        self
    }

    /// Set the maximum depth of the lexer state stack.
    ///
    /// [`Transition::Push`] transitions that would exceed this depth fail
    /// with a depth limit error. This bounds recursion for grammars with
    /// nested constructs.
    ///
    /// The default is 64.
    pub fn max_state_depth(&mut self, depth: usize) -> &mut LexerBuilder {
        self.max_state_depth = depth;
        self
    }

    /// Set the syntax configuration used to compile rule patterns.
    ///
    /// This permits toggling things like Unicode and UTF-8 support. For
    /// example, with UTF-8 support disabled
    /// ([`crate::util::syntax::Config::utf8`]), patterns may match arbitrary
    /// bytes (including invalid UTF-8) via classes like `[^\x00-\x7F]`.
    pub fn syntax(
        &mut self,
        config: crate::util::syntax::Config,
    ) -> &mut LexerBuilder {
        self.syntax = config;
        self
    }

    /// Set the Thompson NFA configuration used to compile rule patterns.
    pub fn thompson(
        &mut self,
        config: crate::nfa::thompson::Config,
    ) -> &mut LexerBuilder {
        self.thompson = config;
        self
    }

    /// Set the dense DFA compilation configuration.
    ///
    /// Note that the match kind is always overridden to
    /// [`MatchKind::All`], since the lexer requires longest-match
    /// semantics.
    pub fn dense(&mut self, config: dense::Config) -> &mut LexerBuilder {
        self.dfa = config.match_kind(MatchKind::All);
        self
    }
    /// Build the lexer.
    ///
    /// This returns an error if any rule pattern fails to compile (for
    /// example, a pattern like `\xFF` that is not valid when Unicode mode
    /// is enabled), if any rule pattern can match the empty string, or if a
    /// rule or transition refers to an unknown state.
    pub fn build(&self) -> Result<Lexer, BuildError> {
        if self.states.is_empty() {
            return Err(BuildError::new(BuildErrorKind::NoStates));
        }
        let start = self.start.unwrap_or(StateId(0));
        self.check_state(start)?;
        for &(id, _) in self.final_overrides.iter() {
            self.check_state(id)?;
        }
        // Group rules by state, preserving declaration order.
        let mut state_rules: Vec<Vec<&RuleDef>> =
            (0..self.states.len()).map(|_| Vec::new()).collect();
        for &(id, ref rule) in self.rules.iter() {
            self.check_state(id)?;
            self.check_transition(rule.action.transition)?;
            state_rules[id.0].push(rule);
        }
        let mut states = Vec::with_capacity(self.states.len());
        for (i, def) in self.states.iter().enumerate() {
            let rules = &state_rules[i];
            let mut builder = dense::Builder::new();
            builder
                .configure(self.dfa.clone())
                .syntax(self.syntax.clone())
                .thompson(self.thompson.clone());
            let dfa = if rules.is_empty() {
                None
            } else {
                let patterns: Vec<&str> =
                    rules.iter().map(|r| r.pattern.as_str()).collect();
                let dfa = builder
                    .build_many(&patterns)
                    .map_err(|err| BuildError::regex(i, err))?;
                // Reject any rule that can match the empty string. Since
                // searches are anchored and use longest-match semantics, an
                // anchored search of the empty haystack matches precisely
                // when some rule matches empty, and reports the first such
                // rule.
                let input = Input::new("").anchored(Anchored::Yes);
                if let Some(m) =
                    dfa.try_search_fwd(&input).map_err(BuildError::search)?
                {
                    return Err(BuildError::new(BuildErrorKind::EmptyMatch {
                        state: i,
                        rule: m.pattern().as_usize(),
                    }));
                }
                Some(dfa)
            };
            let is_final = self
                .final_overrides
                .iter()
                .rev()
                .find(|&&(id, _)| id.0 == i)
                .map(|&(_, yes)| yes)
                .unwrap_or(i == start.0);
            states.push(LexerState {
                name: def.name.clone(),
                dfa,
                actions: rules.iter().map(|r| r.action).collect(),
                is_final,
            });
        }
        Ok(Lexer { states, start, max_state_depth: self.max_state_depth })
    }

    fn check_state(&self, id: StateId) -> Result<(), BuildError> {
        if id.0 >= self.states.len() {
            return Err(BuildError::new(BuildErrorKind::UnknownState {
                id: id.0,
            }));
        }
        Ok(())
    }

    fn check_transition(&self, t: Transition) -> Result<(), BuildError> {
        match t {
            Transition::Stay | Transition::Pop => Ok(()),
            Transition::Switch(id) | Transition::Push(id) => {
                self.check_state(id)
            }
        }
    }
}

impl Default for LexerBuilder {
    fn default() -> LexerBuilder {
        LexerBuilder::new()
    }
}

/// A compiled state-based lexer.
///
/// A lexer is built with [`LexerBuilder`] and produces a sequence of
/// [`Token`]s from a byte haystack via [`Lexer::lex`].
#[derive(Clone, Debug)]
pub struct Lexer {
    states: Vec<LexerState>,
    start: StateId,
    max_state_depth: usize,
}

#[derive(Clone, Debug)]
struct LexerState {
    name: String,
    dfa: Option<dense::OwnedDFA>,
    actions: Vec<Action>,
    is_final: bool,
}

impl Lexer {
    /// Lex the entire haystack, returning the tokens found.
    ///
    /// Lexing starts in the start state at offset 0. At each step, the
    /// current state's rules are matched anchored at the current position;
    /// the longest match wins, with ties broken by rule declaration order.
    ///
    /// # Errors
    ///
    /// This returns an error when no rule matches at the current position
    /// (the byte offset and state are reported and the input is not
    /// skipped), when a state transition would exceed the state stack depth
    /// limit or pop an empty stack, or when the haystack ends while the
    /// lexer is in a non-final state (e.g. an unterminated string).
    pub fn lex(&self, haystack: &[u8]) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        let mut pos = 0;
        let mut current = self.start;
        let mut stack: Vec<StateId> = Vec::new();
        while pos < haystack.len() {
            let state = &self.states[current.0];
            let m = match state.dfa.as_ref() {
                None => None,
                Some(dfa) => {
                    let input = Input::new(haystack)
                        .span(pos..haystack.len())
                        .anchored(Anchored::Yes);
                    dfa.try_search_fwd(&input).map_err(LexError::search)?
                }
            };
            let m = match m {
                None => {
                    return Err(LexError::new(LexErrorKind::NoMatch {
                        offset: pos,
                        state: current,
                    }));
                }
                Some(m) => m,
            };
            let span = Span { start: pos, end: m.offset() };
            if span.is_empty() {
                // This is defensively checked at lex time even though rules
                // that match the empty string are rejected at build time:
                // some patterns (e.g. those with look-around assertions)
                // can match empty only in certain haystacks. Returning an
                // error here guarantees lexing always terminates.
                return Err(LexError::new(LexErrorKind::EmptyMatch {
                    offset: pos,
                    state: current,
                }));
            }
            let action = state.actions[m.pattern().as_usize()];
            if let Some(kind) = action.emit {
                tokens.push(Token { kind, span });
            }
            pos = span.end;
            match action.transition {
                Transition::Stay => {}
                Transition::Switch(next) => current = next,
                Transition::Push(next) => {
                    if stack.len() >= self.max_state_depth {
                        return Err(LexError::new(LexErrorKind::DepthLimit {
                            offset: span.start,
                        }));
                    }
                    stack.push(current);
                    current = next;
                }
                Transition::Pop => match stack.pop() {
                    None => {
                        return Err(LexError::new(
                            LexErrorKind::StackUnderflow {
                                offset: span.start,
                            },
                        ));
                    }
                    Some(prev) => current = prev,
                },
            }
        }
        if !self.states[current.0].is_final {
            return Err(LexError::new(LexErrorKind::UnexpectedEof {
                state: current,
            }));
        }
        Ok(tokens)
    }

    /// Returns the start state of this lexer.
    pub fn start_state(&self) -> StateId {
        self.start
    }

    /// Returns the name of the given state.
    ///
    /// # Panics
    ///
    /// Panics if `state` is not a valid state identifier for this lexer.
    pub fn state_name(&self, state: StateId) -> &str {
        &self.states[state.0].name
    }

    /// Returns the number of states in this lexer.
    pub fn state_len(&self) -> usize {
        self.states.len()
    }
}

/// An error that occurred while building a [`Lexer`].
#[derive(Clone, Debug)]
pub struct BuildError {
    kind: BuildErrorKind,
}

#[derive(Clone, Debug)]
enum BuildErrorKind {
    /// No states were added to the lexer.
    NoStates,
    /// A rule, transition or configuration referred to an unknown state.
    UnknownState { id: usize },
    /// A rule pattern failed to compile. `state` is the index of the state
    /// the rule belongs to.
    Regex { state: usize, err: dense::BuildError },
    /// A rule pattern can match the empty string. `state` and `rule` are
    /// the indices of the state and the rule within that state.
    EmptyMatch { state: usize, rule: usize },
    /// A search failed unexpectedly while validating patterns.
    Search { err: MatchError },
}

impl BuildError {
    fn new(kind: BuildErrorKind) -> BuildError {
        BuildError { kind }
    }

    fn regex(state: usize, err: dense::BuildError) -> BuildError {
        BuildError::new(BuildErrorKind::Regex { state, err })
    }

    fn search(err: MatchError) -> BuildError {
        BuildError::new(BuildErrorKind::Search { err })
    }
}

impl core::fmt::Display for BuildError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind {
            BuildErrorKind::NoStates => {
                write!(f, "cannot build a lexer with no states")
            }
            BuildErrorKind::UnknownState { id } => {
                write!(f, "reference to unknown lexer state {}", id)
            }
            BuildErrorKind::Regex { state, ref err } => write!(
                f,
                "error compiling rule patterns for lexer state {}: {}",
                state, err,
            ),
            BuildErrorKind::EmptyMatch { state, rule } => write!(
                f,
                "rule {} in lexer state {} can match the empty string, \
                 which is not allowed",
                rule, state,
            ),
            BuildErrorKind::Search { ref err } => write!(
                f,
                "unexpected search error while building lexer: {}",
                err,
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.kind {
            BuildErrorKind::Regex { ref err, .. } => Some(err),
            BuildErrorKind::Search { ref err } => Some(err),
            _ => None,
        }
    }
}

/// An error that occurred while lexing a haystack.
///
/// Every lexing error reports enough context to point at the problem: the
/// byte offset at which lexing failed and/or the state the lexer was in.
/// Lexers never skip over input that cannot be matched.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LexError {
    kind: LexErrorKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LexErrorKind {
    /// No rule in the current state matched at the current position.
    NoMatch { offset: usize, state: StateId },
    /// A rule produced a zero-length match. (Rules that match the empty
    /// string are rejected at build time, but some patterns can match empty
    /// only in particular haystacks.)
    EmptyMatch { offset: usize, state: StateId },
    /// A push transition would exceed the state stack depth limit.
    DepthLimit { offset: usize },
    /// A pop transition occurred with an empty state stack.
    StackUnderflow { offset: usize },
    /// The haystack ended while the lexer was in a non-final state.
    UnexpectedEof { state: StateId },
    /// The underlying DFA search failed unexpectedly.
    Search { err: MatchError },
}

impl LexError {
    fn new(kind: LexErrorKind) -> LexError {
        LexError { kind }
    }

    fn search(err: MatchError) -> LexError {
        LexError::new(LexErrorKind::Search { err })
    }

    /// Returns the byte offset in the haystack at which lexing failed, if
    /// this error is associated with a particular offset.
    pub fn offset(&self) -> Option<usize> {
        match self.kind {
            LexErrorKind::NoMatch { offset, .. }
            | LexErrorKind::EmptyMatch { offset, .. }
            | LexErrorKind::DepthLimit { offset }
            | LexErrorKind::StackUnderflow { offset } => Some(offset),
            LexErrorKind::UnexpectedEof { .. }
            | LexErrorKind::Search { .. } => None,
        }
    }

    /// Returns the state the lexer was in when lexing failed, if this error
    /// is associated with a particular state.
    pub fn state(&self) -> Option<StateId> {
        match self.kind {
            LexErrorKind::NoMatch { state, .. }
            | LexErrorKind::EmptyMatch { state, .. }
            | LexErrorKind::UnexpectedEof { state } => Some(state),
            LexErrorKind::DepthLimit { .. }
            | LexErrorKind::StackUnderflow { .. }
            | LexErrorKind::Search { .. } => None,
        }
    }
}

impl core::fmt::Display for LexError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.kind {
            LexErrorKind::NoMatch { offset, state } => write!(
                f,
                "no lexer rule matched at byte offset {} in state {}",
                offset,
                state.as_usize(),
            ),
            LexErrorKind::EmptyMatch { offset, state } => write!(
                f,
                "lexer rule matched empty string at byte offset {} \
                 in state {}",
                offset,
                state.as_usize(),
            ),
            LexErrorKind::DepthLimit { offset } => write!(
                f,
                "lexer state stack depth limit exceeded at byte offset {}",
                offset,
            ),
            LexErrorKind::StackUnderflow { offset } => write!(
                f,
                "lexer state stack underflow at byte offset {}",
                offset,
            ),
            LexErrorKind::UnexpectedEof { state } => write!(
                f,
                "unexpected end of input in lexer state {}",
                state.as_usize(),
            ),
            LexErrorKind::Search { ref err } => {
                write!(f, "unexpected lexer search error: {}", err)
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for LexError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self.kind {
            LexErrorKind::Search { ref err } => Some(err),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;

    const IDENT: u32 = 1;
    const INT: u32 = 2;
    const STRING: u32 = 3;

    fn toks(lexer: &Lexer, haystack: &[u8]) -> Vec<(u32, Span)> {
        lexer
            .lex(haystack)
            .unwrap()
            .iter()
            .map(|t| (t.kind(), t.span()))
            .collect()
    }

    // A lexer for identifiers, integers, whitespace and quoted strings.
    fn demo_lexer() -> (Lexer, StateId, StateId) {
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        let string = b.add_state("string");
        b.rule(main, r"[a-zA-Z_][a-zA-Z0-9_]*", Action::token(IDENT));
        b.rule(main, r"[0-9]+", Action::token(INT));
        b.rule(main, r"\s+", Action::skip());
        b.rule(main, "\"", Action::skip().push(string));
        b.rule(string, "[^\"\n]+", Action::token(STRING));
        b.rule(string, "\"", Action::skip().pop());
        let lexer = b.build().unwrap();
        (lexer, main, string)
    }

    #[test]
    fn sum_12_token_boundaries() {
        let (lexer, _, _) = demo_lexer();
        assert_eq!(
            toks(&lexer, b"sum 12"),
            vec![(IDENT, Span::from(0..3)), (INT, Span::from(4..6)),],
        );
    }

    #[test]
    fn string_state_roundtrip() {
        let (lexer, _, _) = demo_lexer();
        assert_eq!(
            toks(&lexer, b"sum \"a b\" 12"),
            vec![
                (IDENT, Span::from(0..3)),
                (STRING, Span::from(5..8)),
                (INT, Span::from(10..12)),
            ],
        );
    }

    #[test]
    fn unterminated_string_reports_end_state() {
        let (lexer, _, string) = demo_lexer();
        let err = lexer.lex(b"\"abc").unwrap_err();
        assert_eq!(Some(string), err.state());
        assert_eq!(None, err.offset());
        assert_eq!(
            err,
            LexError::new(LexErrorKind::UnexpectedEof { state: string })
        );
    }

    #[test]
    fn no_match_reports_offset_and_state() {
        let (lexer, main, string) = demo_lexer();
        // '@' cannot be matched in the main state; the lexer must not skip
        // the corrupt byte.
        let err = lexer.lex(b"sum @").unwrap_err();
        assert_eq!(
            err,
            LexError::new(LexErrorKind::NoMatch { offset: 4, state: main })
        );
        assert_eq!(Some(4), err.offset());
        assert_eq!(Some(main), err.state());
        // A newline cannot be matched inside a string either.
        let err = lexer.lex(b"\"ab\ncd\"").unwrap_err();
        assert_eq!(
            err,
            LexError::new(LexErrorKind::NoMatch { offset: 3, state: string })
        );
    }

    #[test]
    fn longest_match_beats_declaration_order() {
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        // Declare the shorter rule first. The longer match must still win.
        b.rule(main, "=", Action::token(1));
        b.rule(main, "==", Action::token(2));
        let lexer = b.build().unwrap();
        assert_eq!(toks(&lexer, b"=="), vec![(2, Span::from(0..2))]);
        assert_eq!(toks(&lexer, b"="), vec![(1, Span::from(0..1))]);
        assert_eq!(
            toks(&lexer, b"==="),
            vec![(2, Span::from(0..2)), (1, Span::from(2..3))],
        );
    }

    #[test]
    fn equal_length_match_uses_declaration_order() {
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        // Both rules match exactly "=="; the first declared must win.
        b.rule(main, "==", Action::token(1));
        b.rule(main, "={2}", Action::token(2));
        let lexer = b.build().unwrap();
        assert_eq!(toks(&lexer, b"=="), vec![(1, Span::from(0..2))]);
    }

    #[test]
    fn non_ascii_bytes() {
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        // Permit rules to match raw (possibly invalid UTF-8) bytes.
        b.syntax(crate::util::syntax::Config::new().utf8(false));
        b.rule(main, r"[a-z]+", Action::token(1));
        b.rule(main, r"(?-u:[^\x00-\x7F])+", Action::token(2));
        let lexer = b.build().unwrap();
        // "héllo" in UTF-8: 'h', 0xC3 0xA9, 'l', 'l', 'o'.
        assert_eq!(
            toks(&lexer, "héllo".as_bytes()),
            vec![
                (1, Span::from(0..1)),
                (2, Span::from(1..3)),
                (1, Span::from(3..6)),
            ],
        );
    }

    #[test]
    fn invalid_byte_pattern_rejected_at_build() {
        // By default, patterns may not match invalid UTF-8, so a raw \xFF
        // byte pattern is illegal.
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        b.rule(main, r"(?-u:\xFF)", Action::token(1));
        assert!(b.build().is_err());

        // A syntactically invalid pattern is also rejected.
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        b.rule(main, r"[z-a]", Action::token(1));
        assert!(b.build().is_err());

        // The raw byte pattern is fine when the UTF-8 invariant and Unicode
        // mode are both disabled.
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        b.syntax(
            crate::util::syntax::Config::new().unicode(false).utf8(false),
        );
        b.rule(main, r"\xFF", Action::token(1));
        let lexer = b.build().unwrap();
        assert_eq!(toks(&lexer, b"\xFF"), vec![(1, Span::from(0..1))]);
    }

    #[test]
    fn empty_matching_rule_rejected_at_build() {
        for pat in &[r"a*", r"(?:a|)", r"x{0,3}"] {
            let mut b = LexerBuilder::new();
            let main = b.add_state("main");
            b.rule(main, pat, Action::token(1));
            let err = b.build().unwrap_err();
            assert!(
                matches!(
                    err.kind,
                    BuildErrorKind::EmptyMatch { state: 0, rule: 0 }
                ),
                "pattern: {}",
                pat,
            );
        }
    }

    #[test]
    fn state_stack_depth_limit() {
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        b.max_state_depth(2);
        b.rule(main, r"\(", Action::skip().push(main));
        b.rule(main, r"\)", Action::skip().pop());
        b.rule(main, r"[a-z]+", Action::token(1));
        let lexer = b.build().unwrap();

        // Two pushes are fine.
        assert_eq!(toks(&lexer, b"((x))"), vec![(1, Span::from(2..3))]);
        // The third push exceeds the depth limit.
        let err = lexer.lex(b"(((x").unwrap_err();
        assert_eq!(err, LexError::new(LexErrorKind::DepthLimit { offset: 2 }));
        assert_eq!(Some(2), err.offset());
        // Popping an empty stack is an error too.
        let err = lexer.lex(b")").unwrap_err();
        assert_eq!(
            err,
            LexError::new(LexErrorKind::StackUnderflow { offset: 0 })
        );
    }

    #[test]
    fn build_time_validation() {
        // No states at all.
        assert!(LexerBuilder::new().build().is_err());
        // A rule referencing an unknown state.
        let mut b = LexerBuilder::new();
        let _main = b.add_state("main");
        b.rule(StateId(99), "x", Action::token(1));
        assert!(b.build().is_err());
        // A transition to an unknown state.
        let mut b = LexerBuilder::new();
        let main = b.add_state("main");
        b.rule(main, "x", Action::token(1).switch(StateId(99)));
        assert!(b.build().is_err());
    }
}
