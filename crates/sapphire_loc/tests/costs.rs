//! What a lookup and a formatted string cost. Ignored, since a time means nothing in a debug build or
//! on a loaded CI machine:
//!
//! `cargo test -p sapphire_loc --release --test costs -- --ignored --nocapture --test-threads=1`

#![allow(clippy::print_stdout, reason = "with --nocapture, the measurements the spec quotes")]
#![allow(clippy::disallowed_methods, reason = "a stopwatch around the calls, in a test nothing else reads")]

use std::hint::black_box;
use std::time::Instant;

use sapphire_loc::{LocaleSettings, Localizer, StringTable, Value};
use sapphire_strings::{Entry, Pattern, Span};

const EN: &[u8] = include_bytes!("../testdata/cooked/en.sloc");
const JA: &[u8] = include_bytes!("../testdata/cooked/ja.sloc");

const ROUNDS: usize = 9;
const CALLS: u32 = 200_000;

/// The best round's median-free mean, in nanoseconds per call: the best round is the one a loaded
/// machine disturbed least.
fn nanos(mut work: impl FnMut()) -> f64 {
    (0..ROUNDS)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..CALLS {
                work();
            }
            start.elapsed().as_nanos() as f64 / f64::from(CALLS)
        })
        .fold(f64::INFINITY, f64::min)
}

/// `table` with `count` more plain messages, the size a whole game's text might be.
fn grown(mut table: StringTable, count: usize) -> StringTable {
    let mut entries: Vec<(String, Entry)> =
        table.entries.iter().map(|entry| (table.str(entry.key).to_owned(), *entry)).collect();
    for index in 0..count {
        let key = format!("generated-menu-entry-number-{index:05}");
        let text = format!("Generated text number {index}");
        let (key_span, text_span) = (push(&mut table.pool, &key), push(&mut table.pool, &text));
        entries.push((key, Entry { key: key_span, pattern: Pattern::Plain(text_span) }));
    }
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
    table.entries = entries.into_iter().map(|(_, entry)| entry).collect();
    table
}

fn push(pool: &mut String, text: &str) -> Span {
    let span = Span { start: pool.len() as u32, len: text.len() as u32 };
    pool.push_str(text);
    span
}

#[test]
#[ignore = "a time, for a release build on a quiet machine"]
fn costs() {
    println!("loc-costs entries nanoseconds per call, the best of {ROUNDS} rounds of {CALLS}");
    for extra in [0usize, 1_000, 10_000] {
        let en = grown(StringTable::read(EN).unwrap(), extra);
        let ja = grown(StringTable::read(JA).unwrap(), extra);
        let entries = ja.entries.len();
        let cooked = ja.to_cooked().unwrap();
        let start = Instant::now();
        black_box(StringTable::read(&cooked).unwrap());
        let read = start.elapsed();

        let start = Instant::now();
        let mut loc = Localizer::new(LocaleSettings { text: "ja".to_owned(), voice: "en".to_owned() });
        loc.install(en).unwrap();
        loc.install(ja).unwrap();
        let install = start.elapsed();

        let middle = format!("generated-menu-entry-number-{:05}", extra / 2);
        let plain_key = if extra == 0 { "title" } else { middle.as_str() };
        let plain = nanos(|| {
            black_box(loc.text(black_box(plain_key)));
        });
        let fallback = nanos(|| {
            black_box(loc.text(black_box("only-english")));
        });
        let unknown = nanos(|| {
            black_box(loc.text(black_box("no-such-key")));
        });
        let mut out = String::with_capacity(256);
        let mut format = |key: &str, args: &[(&str, Value<'_>)]| {
            nanos(|| {
                out.clear();
                loc.format(black_box(key), args, &mut out);
                black_box(out.len());
            })
        };
        let name = [("name", Value::Text("Aoi"))];
        let variable = format("welcome", &name);
        let reference = format("welcome-back", &name);
        let plural = format("enemies-left", &[("count", Value::Integer(1234))]);
        let number = format("score", &[("points", Value::Integer(1_234_567))]);
        let fraction = format("damage", &[("amount", Value::Float(12.345))]);
        println!(
            "loc-costs entries={entries} cooked_bytes={} read_us={:.1} install_two_us={:.1} table_bytes={} text={plain:.1} \
             text_fallback={fallback:.1} text_unknown={unknown:.1} format_variable_and_term={variable:.1} \
             format_reference={reference:.1} format_plural_and_number={plural:.1} format_number={number:.1} \
             format_fraction={fraction:.1}",
            cooked.len(),
            read.as_secs_f64() * 1e6,
            install.as_secs_f64() * 1e6,
            loc.stats().table_bytes,
        );
    }
}
