//! A tiny log classifier built on `regex::selector::Selector`.
//!
//! Run with:
//!
//! ```text
//! $ cargo run --example selector_classify
//! xxabz: rule 1 `ab` wins in `xxabz` at 2..4
//!   group[0] = `ab`
//! xxzz: rule 0 `z` wins in `xxzz` at 2..3
//!   group[0] = `z`
//! ab: rule 0 `a|ab` wins in `ab` at 0..1
//!   group[0] = `a`
//! bytes: rule 1 `(?-u)\xFF+` wins over `z` in non-UTF-8 haystack at 4..6
//!   group[0] = [FF FF]
//! ```

use regex::selector::Selector;

fn show(label: &str, sel: &Selector, hay: &str) {
    match sel.classify(hay) {
        Some(pick) => {
            println!(
                "{}: rule {} `{}` wins in `{}` at {}..{}",
                label,
                pick.rule(),
                sel.pattern(pick.rule()),
                hay,
                pick.start(),
                pick.end(),
            );
            for (i, group) in pick.captures().iter().enumerate() {
                if let Some(m) = group {
                    println!("  group[{}] = `{}`", i, m.as_str());
                }
            }
        }
        None => println!("{}: no rule matched `{}`", label, hay),
    }
}

fn main() {
    // Two independent rules; they are deliberately NOT merged into one big
    // "z|ab" alternation.
    let sel = Selector::new(["z", "ab"]).unwrap();
    show("xxabz", &sel, "xxabz");

    // Same earliest start -> the rule declared first wins.
    let tied = Selector::new(["z", "zz"]).unwrap();
    show("xxzz", &tied, "xxzz");

    // A rule that is itself an alternation keeps its leftmost-first order.
    let alt = Selector::new(["a|ab"]).unwrap();
    show("ab", &alt, "ab");

    // The byte selector handles haystacks that are not valid UTF-8.
    let bsel =
        regex::selector::bytes::Selector::new(["z", r"(?-u)\xFF+"]).unwrap();
    let hay: &[u8] = b"xxab\xFF\xFFz";
    match bsel.classify(hay) {
        Some(pick) => {
            print!(
                "bytes: rule {} `{}` wins over `z` in non-UTF-8 haystack at {}..{}\n  group[0] = [",
                pick.rule(),
                bsel.pattern(pick.rule()),
                pick.start(),
                pick.end(),
            );
            for (i, &b) in pick.matched().iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                print!("{:02X}", b);
            }
            println!("]");
        }
        None => println!("bytes: no rule matched"),
    }
}
