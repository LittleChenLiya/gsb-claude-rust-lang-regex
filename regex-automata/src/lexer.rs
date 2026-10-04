/*!
A reusable lexer builder layer.

This module provides a builder API for constructing lexers (tokenizers) on top
of the regex automata infrastructure. Each state in the lexer contains an
ordered list of rules. Each rule matches the current position and specifies an
action: emit a token, skip the matched bytes, or switch to another state.

At each step, the lexer selects the rule with the longest byte match. If
multiple rules match with the same length, the rule declared first wins. Rules
that can match the empty string are rejected at build time. If no rule matches
at the current position, the lexer reports an error with the byte position and
current state name, without skipping any input.
*/

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::meta;
use crate::util::search::{Anchored, Input};

/// A builder for constructing a Lexer.
#[derive(Debug)]
pub struct LexerBuilder<T> {
    states: Vec<(String, Vec<Rule<T>>, bool)>,
    initial_state: Option<String>,
    max_state_switches: usize,
}

/// A builder for a single lexer state.
#[derive(Debug)]
pub struct StateBuilder<T> {
    parent: LexerBuilder<T>,
    name: String,
    rules: Vec<Rule<T>>,
    terminal: bool,
}

/// A rule in a lexer state.
#[derive(Debug)]
pub struct Rule<T> {
    pattern: String,
    action: Action<T>,
}

/// An action to take when a rule matches at the current position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action<T> {
    /// Emit a token of the given type.
    Emit(T),
    /// Skip the matched bytes without emitting a token.
    Skip,
    /// Switch to the state with the given name.
    Switch(String),
}

/// A compiled lexer.
#[derive(Debug)]
pub struct Lexer<T> {
    states: Vec<State<T>>,
    initial_state: usize,
    max_state_switches: usize,
}

/// A compiled lexer state.
#[derive(Debug)]
pub struct State<T> {
    name: String,
    rules: Vec<CompiledRule<T>>,
    terminal: bool,
}

/// A compiled rule.
#[derive(Debug)]
pub struct CompiledRule<T> {
    regex: meta::Regex,
    action: CompiledAction<T>,
}

/// A compiled action.
#[derive(Debug)]
enum CompiledAction<T> {
    Emit(T),
    Skip,
    Switch(usize),
}

/// A token produced by the lexer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<T> {
    /// The token type.
    pub ty: T,
    /// The start byte offset (inclusive) of the token.
    pub start: usize,
    /// The end byte offset (exclusive) of the token.
    pub end: usize,
}

/// An error that can occur during lexing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexerError {
    /// No rule matched at the current position.
    NoMatch {
        /// The byte position where no rule matched.
        position: usize,
        /// The name of the current state.
        state: String,
    },
    /// End of input was reached in a non-terminal state.
    UnexpectedEnd {
        /// The name of the non-terminal state.
        state: String,
    },
    /// Too many state switches occurred during a single scan.
    TooManyStateSwitches {
        /// The number of state switches that occurred.
        count: usize,
        /// The configured limit.
        limit: usize,
    },
    /// An error occurred while building the lexer.
    BuildError(String),
}

impl fmt::Display for LexerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LexerError::NoMatch { position, state } => {
                write!(f, "no rule matched at byte position {} in state {:?}", position, state)
            }
            LexerError::UnexpectedEnd { state } => {
                write!(f, "unexpected end of input in non-terminal state {:?}", state)
            }
            LexerError::TooManyStateSwitches { count, limit } => {
                write!(f, "too many state switches ({}), limit is {}", count, limit)
            }
            LexerError::BuildError(msg) => {
                write!(f, "lexer build error: {}", msg)
            }
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for LexerError {}

impl<T: fmt::Debug + Clone> Default for LexerBuilder<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: fmt::Debug + Clone> LexerBuilder<T> {
    /// Create a new lexer builder with no states.
    pub fn new() -> Self {
        LexerBuilder {
            states: Vec::new(),
            initial_state: None,
            max_state_switches: 10_000,
        }
    }

    /// Add a new state with the given name.
    pub fn state(self, name: &str) -> StateBuilder<T> {
        StateBuilder {
            parent: self,
            name: String::from(name),
            rules: Vec::new(),
            terminal: true,
        }
    }

    /// Set the initial state for the lexer.
    pub fn initial_state(mut self, name: &str) -> Self {
        self.initial_state = Some(String::from(name));
        self
    }

    /// Set the maximum number of state switches allowed during a single scan.
    pub fn max_state_switches(mut self, limit: usize) -> Self {
        self.max_state_switches = limit;
        self
    }

    /// Build the lexer.
    pub fn build(self) -> Result<Lexer<T>, LexerError> {
        if self.states.is_empty() {
            return Err(LexerError::BuildError(String::from("no states defined")));
        }

        let initial_name = self.initial_state.ok_or_else(|| {
            LexerError::BuildError(String::from("no initial state set"))
        })?;

        let state_names: Vec<String> = self.states.iter().map(|(n, _, _)| n.clone()).collect();

        let initial_idx = state_names
            .iter()
            .position(|n| n == &initial_name)
            .ok_or_else(|| {
                LexerError::BuildError(format!("initial state {:?} does not exist", initial_name))
            })?;

        let mut compiled_states: Vec<State<T>> = Vec::new();
        for (name, rules, terminal) in self.states {
            if rules.is_empty() {
                return Err(LexerError::BuildError(format!(
                    "state {:?} has no rules", name
                )));
            }

            let mut compiled_rules: Vec<CompiledRule<T>> = Vec::new();
            for rule in rules {
                let regex = meta::Regex::new(&rule.pattern).map_err(|e| {
                    LexerError::BuildError(format!(
                        "invalid pattern {:?}: {}", rule.pattern, e
                    ))
                })?;

                if regex.find("").is_some() {
                    return Err(LexerError::BuildError(format!(
                        "rule with pattern {:?} in state {:?} can match the empty string",
                        rule.pattern, name
                    )));
                }

                let action = match rule.action {
                    Action::Emit(t) => CompiledAction::Emit(t),
                    Action::Skip => CompiledAction::Skip,
                    Action::Switch(target) => {
                        let idx = state_names
                            .iter()
                            .position(|n| n == &target)
                            .ok_or_else(|| {
                                LexerError::BuildError(format!(
                                    "rule in state {:?} switches to non-existent state {:?}",
                                    name, target
                                ))
                            })?;
                        CompiledAction::Switch(idx)
                    }
                };

                compiled_rules.push(CompiledRule { regex, action });
            }

            compiled_states.push(State {
                name,
                rules: compiled_rules,
                terminal,
            });
        }

        Ok(Lexer {
            states: compiled_states,
            initial_state: initial_idx,
            max_state_switches: self.max_state_switches,
        })
    }
}

impl<T: fmt::Debug + Clone> StateBuilder<T> {
    /// Add a rule to this state.
    pub fn rule(mut self, pattern: &str, action: Action<T>) -> Self {
        self.rules.push(Rule {
            pattern: String::from(pattern),
            action,
        });
        self
    }

    /// Set whether this state is terminal.
    pub fn terminal(mut self, yes: bool) -> Self {
        self.terminal = yes;
        self
    }

    /// Finish configuring this state and return to the parent builder.
    pub fn done(self) -> LexerBuilder<T> {
        let mut parent = self.parent;
        parent.states.push((self.name, self.rules, self.terminal));
        parent
    }
}

impl<T: fmt::Debug + Clone> Lexer<T> {
    /// Scan the given haystack and return a list of tokens.
    pub fn scan<H: AsRef<[u8]>>(&self, haystack: H) -> Result<Vec<Token<T>>, LexerError> {
        let haystack = haystack.as_ref();
        let mut tokens = Vec::new();
        let mut pos = 0;
        let mut state = self.initial_state;
        let mut state_switches = 0;

        while pos < haystack.len() {
            let current_state = &self.states[state];
            let mut best_match: Option<(usize, usize, &CompiledAction<T>)> = None;

            for (rule_idx, rule) in current_state.rules.iter().enumerate() {
                let input = Input::new(haystack)
                    .range(pos..)
                    .anchored(Anchored::Yes);

                if let Some(m) = rule.regex.find(input) {
                    let match_len = m.len();
                    if best_match.is_none() || match_len > best_match.unwrap().1 {
                        best_match = Some((rule_idx, match_len, &rule.action));
                    }
                }
            }

            match best_match {
                Some((_, match_len, action)) => {
                    let end = pos + match_len;
                    match action {
                        CompiledAction::Emit(ty) => {
                            tokens.push(Token {
                                ty: ty.clone(),
                                start: pos,
                                end,
                            });
                            pos = end;
                        }
                        CompiledAction::Skip => {
                            pos = end;
                        }
                        CompiledAction::Switch(target_idx) => {
                            state = *target_idx;
                            pos = end;
                            state_switches += 1;
                            if state_switches > self.max_state_switches {
                                return Err(LexerError::TooManyStateSwitches {
                                    count: state_switches,
                                    limit: self.max_state_switches,
                                });
                            }
                        }
                    }
                }
                None => {
                    return Err(LexerError::NoMatch {
                        position: pos,
                        state: current_state.name.clone(),
                    });
                }
            }
        }

        if !self.states[state].terminal {
            return Err(LexerError::UnexpectedEnd {
                state: self.states[state].name.clone(),
            });
        }

        Ok(tokens)
    }

    /// Return the number of states in the lexer.
    pub fn state_len(&self) -> usize {
        self.states.len()
    }

    /// Return the name of the state with the given index.
    pub fn state_name(&self, idx: usize) -> Option<&str> {
        self.states.get(idx).map(|s| s.name.as_str())
    }
}

#[cfg(test)]
mod lexer_tests;
