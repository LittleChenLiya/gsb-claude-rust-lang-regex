//! Demonstrates the multi-rule replacement component: it prints the new
//! log line plus the adopted/shadowed match ranges (no original text beyond
//! the replacement result is echoed by the range report itself).
//!
//! Run with:
//!
//! cargo run --example multi_replace_log

use regex::multi_replace::{MultiReplace, ReplacementRule};

fn main() {
    let mr = MultiReplace::new([
        // Highest priority at a shared start: redact the whole email.
        ReplacementRule::new(r"(?<user>[\w.]+)@[\w.]+", 10, "<email:$user>"),
        // Numeric tokens, e.g. user ids and ports.
        ReplacementRule::new(r"\d+", 0, "<num>"),
        // A deliberately overlapping competitor that must lose to the email
        // rule where they both start (priority decides), but wins elsewhere.
        ReplacementRule::new(r"\w+\.example", 5, "<host>"),
    ])
    .expect("rules compile and templates reference real groups");

    let log = "user 42 connected from alice@mail.example at 09:41";
    let result = mr.replace_all(log).expect("within limits");

    println!("input : {log}");
    println!("result: {}", result.text);
    println!("match report (rule, byte range, adopted?):");
    for report in &result.reports {
        println!(
            "  rule={} bytes={}..{} adopted={}",
            report.rule, report.range.start, report.range.end, report.adopted,
        );
    }
}
