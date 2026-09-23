//! A short demo of streaming regex search over chunked byte input.
//!
//! Run with: cargo run --example streaming
//!
//! It shows, for a few patterns, exactly which matches each `feed` call
//! reports and which matches are only delivered by `finish`, and that
//! patterns with unbounded match width are rejected at construction.

use regex::{StreamMatch, StreamingFinder};

fn show(label: &str, matches: &[StreamMatch]) {
    if matches.is_empty() {
        println!("  {:<28} -> (nothing final yet)", label);
    } else {
        for m in matches {
            println!(
                "  {:<28} -> [{}..{}] {:?}",
                label, m.start, m.end, m.text
            );
        }
    }
}

fn demo(pattern: &str, chunks: &[&[u8]]) {
    println!("pattern {:?} fed as {:?}", pattern, chunks);
    let mut finder = StreamingFinder::new(pattern).unwrap();
    for (i, chunk) in chunks.iter().enumerate() {
        let matches = finder.feed(chunk).unwrap();
        show(&format!("feed #{} {:?}", i + 1, chunk), &matches);
    }
    let matches = finder.finish().unwrap();
    show("finish", &matches);
    println!();
}

fn main() {
    // A greedy match is only reported once it is provably final: the
    // `abb` below is delivered by `finish`, never prematurely as `ab`.
    demo(r"ab{1,2}", &[b"a", b"b", b"b"]);

    // A multi-byte codepoint (here U+00E9, 0xC3 0xA9) may be split
    // across chunk boundaries.
    demo("\\w{1,4}", &[b"h\xC3", b"\xA9llo ", b"w", b"orld"]);

    // `$` can only be decided once the stream ends.
    demo(r"b$", &[b"a", b"b"]);

    // Empty matches are delivered on codepoint boundaries.
    demo("", &[b"a\xC3", b"\xA9"]);

    // Patterns whose match width cannot be bounded fail at construction.
    for pattern in [r"a+", r"a{2,}", r"\w+"] {
        match StreamingFinder::new(pattern) {
            Ok(_) => println!("{:?}: unexpectedly accepted", pattern),
            Err(e) => println!("{:?}: construction failed: {}", pattern, e),
        }
    }
}
