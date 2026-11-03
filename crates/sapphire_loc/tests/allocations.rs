//! What allocates, counted by the engine's counting allocator: nothing, for a plain lookup. Its own
//! binary, with one test, since a counting allocator is process-wide and a harness allocates between
//! tests.

use std::hint::black_box;

use sapphire_core::alloc::{CountingAllocator, count_in_scope};
use sapphire_loc::{LocaleSettings, Localizer, StringTable, Value};

#[global_allocator]
static GLOBAL: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

const EN: &[u8] = include_bytes!("../testdata/cooked/en.sloc");
const JA: &[u8] = include_bytes!("../testdata/cooked/ja.sloc");

fn allocations(mut work: impl FnMut()) -> u64 {
    let ((), counts) = count_in_scope(|| {
        for _ in 0..100 {
            work();
        }
    });
    counts.allocations
}

#[test]
fn what_allocates() {
    let mut loc = Localizer::new(LocaleSettings { text: "ja".to_owned(), voice: "en".to_owned() });
    loc.install(StringTable::read(EN).unwrap()).unwrap();
    loc.install(StringTable::read(JA).unwrap()).unwrap();

    // Found, fallen back to English, and unknown.
    for key in ["title", "save.tooltip", "only-english", "no-such-key"] {
        let count = allocations(|| {
            black_box(loc.text(black_box(key)));
        });
        assert_eq!(count, 0, "text({key})");
    }
    let mut scratch = String::with_capacity(256);
    let count = allocations(|| {
        black_box(loc.resolve(black_box("title"), &[], &mut scratch).len());
    });
    assert_eq!(count, 0, "resolve of a plain message");

    // A formatted string, once its buffer has grown, for numbers of up to 8 digits.
    let args = [("count", Value::Integer(12_345_678)), ("name", Value::Text("Aoi"))];
    for key in ["welcome", "enemies-left", "welcome-back", "only-english-count"] {
        let mut out = String::with_capacity(256);
        let count = allocations(|| {
            out.clear();
            loc.format(black_box(key), &args, &mut out);
        });
        assert_eq!(count, 0, "format({key}) gave {out}");
    }

    // The switch, which does nothing but pick a chain out, and what shows text does after it.
    let mut loc2 = Localizer::new(LocaleSettings::default());
    loc2.install(StringTable::read(EN).unwrap()).unwrap();
    loc2.install(StringTable::read(JA).unwrap()).unwrap();
    let count = allocations(|| {
        loc2.set_text_locale("ja").unwrap();
        loc2.set_text_locale("en").unwrap();
    });
    // Its chain's vector, in either direction, and growing it for the fallback.
    assert!(count <= 400, "{count} allocations in 200 switches");
}
