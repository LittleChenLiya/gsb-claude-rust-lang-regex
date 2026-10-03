//! A short example of classifying log lines with a [`RuleSelector`].
//!
//! The selector picks the rule whose match starts earliest; ties are broken by
//! declaration order. This example prints the winning rule and its captures
//! for each log line.
//!
//! Run it with:
//!
//! ```text
//! cargo run --example rule_selector
//! ```

use regex::selector::{Rule, RuleSelector};
use regex::Regex;

fn main() {
    let mut selector = RuleSelector::new();
    // Declare rules in order of preference for ties.
    selector.push(Rule::new(
        "error",
        Regex::new(r"(\w+): error: (.*)").unwrap(),
    ));
    selector.push(Rule::new(
        "warn",
        Regex::new(r"(\w+): warning: (.*)").unwrap(),
    ));
    selector.push(Rule::new(
        "info",
        Regex::new(r"(\w+): info: (.*)").unwrap(),
    ));
    // A catch-all that matches the empty string at the start of every line.
    // It only wins when nothing else matches (earlier rules start later).
    selector.push(Rule::new("other", Regex::new(r"").unwrap()));

    let lines = [
        "main: error: something went wrong",
        "main: warning: low disk space",
        "main: info: all good",
        "just a plain line",
    ];

    for line in lines {
        let selected = selector.select(line).expect("the catch-all always matches");
        println!("line:    {line}");
        println!("rule:    {}", selected.name());
        println!("match:   {:?}", selected.as_str());
        println!("span:    {}..{}", selected.start(), selected.end());
        let caps = selected.captures();
        for (i, group) in caps.iter().enumerate() {
            if let Some(m) = group {
                println!("  capture {i}: {:?}", m.as_str());
            }
        }
        println!();
    }
}
