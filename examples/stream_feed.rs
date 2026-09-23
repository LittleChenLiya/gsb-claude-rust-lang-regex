//! Demonstrates streaming regex search over chunked byte input.
//!
//! Run with: cargo run --example stream_feed
//!
//! The example feeds input in small chunks with a short delay to
//! simulate a stream, prints the matches determined by each feed call
//! and by the final finish call, and shows that patterns with
//! unbounded match width are rejected at construction time. It runs
//! in well under 90 seconds.

use std::thread::sleep;
use std::time::Duration;

use regex::{StreamError, StreamFinder, StreamMatch};

fn show(matches: &[StreamMatch]) -> Vec<String> {
    matches
        .iter()
        .map(|m| format!("{}..{} {:?}", m.start(), m.end(), m.as_str()))
        .collect()
}

fn main() {
    // Patterns with unbounded repetition fail at construction time.
    match StreamFinder::new(r"a+") {
        Err(StreamError::UnboundedPattern) => {
            println!("StreamFinder::new(a+) -> UnboundedPattern");
        }
        other => panic!(
            "expected UnboundedPattern, got {:?}",
            other.map(|f| f.pattern().to_string())
        ),
    }

    // A greedy match is held back until it is fully determined.
    println!("--- pattern ab{{1,2}} over the chunks a | b | b ---");
    let mut finder = StreamFinder::new(r"ab{1,2}").unwrap();
    for chunk in [b"a", b"b", b"b"] {
        sleep(Duration::from_millis(300));
        let got = finder.feed(chunk).unwrap();
        println!(
            "feed({:?}) -> {:?}",
            String::from_utf8_lossy(chunk),
            show(&got)
        );
    }
    let got = finder.finish().unwrap();
    println!("finish() -> {:?}", show(&got));

    // Multi-byte UTF-8 chars may be split across chunks.
    println!("--- pattern é. over xé字y, fed two bytes at a time ---");
    let text = "xé字y";
    let mut finder = StreamFinder::new("é.").unwrap();
    for chunk in text.as_bytes().chunks(2) {
        sleep(Duration::from_millis(300));
        let got = finder.feed(chunk).unwrap();
        println!("feed({:?}) -> {:?}", chunk, show(&got));
    }
    let got = finder.finish().unwrap();
    println!("finish() -> {:?}", show(&got));

    // A trailing $ anchor can only be decided at finish.
    println!("--- pattern a$ over b | a ---");
    let mut finder = StreamFinder::new(r"a$").unwrap();
    for chunk in [b"b", b"a"] {
        sleep(Duration::from_millis(300));
        let got = finder.feed(chunk).unwrap();
        println!(
            "feed({:?}) -> {:?}",
            String::from_utf8_lossy(chunk),
            show(&got)
        );
    }
    let got = finder.finish().unwrap();
    println!("finish() -> {:?}", show(&got));
}
