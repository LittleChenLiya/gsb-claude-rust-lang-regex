//! Command line interface to the explicit capture group renamer
//! (`regex::rename::rename_captures`).
//!
//! Usage:
//!
//! ```text
//! cargo run --example rename -- 'PATTERN' 'OLD=NEW' ...
//! ```
//!
//! With no arguments it runs a built-in demo: it renames `year` to `y` in
//! a date pattern, shows that the matched bytes and group numbers are
//! preserved, fetches the old `year` content via `y` and prints the
//! migration table.

use regex::rename::{rename_captures, Renamed};
use regex::Regex;

const DEMO_PATTERN: &str = r"(?P<year>\d{4})-(?P<month>\d{2})-(?P<day>\d{2})";
const DEMO_HAYSTACK: &str = "2026-10-05";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = if args.is_empty() { demo() } else { run(&args) };
    std::process::exit(code);
}

fn run(args: &[String]) -> i32 {
    let pattern = &args[0];
    let mut pairs: Vec<(String, String)> = vec![];
    for arg in &args[1..] {
        let Some((old, new)) = arg.split_once('=') else {
            eprintln!("invalid rename '{arg}', expected OLD=NEW");
            return 2;
        };
        pairs.push((old.to_string(), new.to_string()));
    }
    let pairs: Vec<(&str, &str)> =
        pairs.iter().map(|(o, n)| (o.as_str(), n.as_str())).collect();
    match rename_captures(pattern, pairs) {
        Ok(renamed) => {
            println!("new pattern: {}", renamed.pattern());
            println!("migration table:");
            print!("{}", renamed.migration_table());
            0
        }
        Err(err) => {
            eprintln!("{err}");
            1
        }
    }
}

fn demo() -> i32 {
    println!("pattern:  {DEMO_PATTERN}");
    println!("rename:   year=y");
    let renamed: Renamed =
        match rename_captures(DEMO_PATTERN, [("year", "y")]) {
            Ok(renamed) => renamed,
            Err(err) => {
                eprintln!("{err}");
                return 1;
            }
        };
    println!("new pattern: {}", renamed.pattern());

    let old = Regex::new(DEMO_PATTERN).unwrap();
    let caps_old = old.captures(DEMO_HAYSTACK).unwrap();
    let caps_new = renamed.regex().captures(DEMO_HAYSTACK).unwrap();
    println!("haystack: {DEMO_HAYSTACK}");
    for i in 0..caps_old.len() {
        let old_span = caps_old.get(i).map(|m| (m.start(), m.end()));
        let new_span = caps_new.get(i).map(|m| (m.start(), m.end()));
        assert_eq!(old_span, new_span, "group {i} moved");
        println!("group {i}: {old_span:?} == {new_span:?}");
    }
    let year = caps_new.name("y").unwrap().as_str();
    assert_eq!(year, caps_old.name("year").unwrap().as_str());
    println!("old year content fetched via 'y': {year}");

    println!("migration table:");
    print!("{}", renamed.migration_table());
    0
}
