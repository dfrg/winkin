//! Segmentation timings over the 1 MB corpora: the crate's own segmenters
//! beside ICU4X's line, grapheme and word segmenters, the baseline they must
//! beat.
//!
//! For each corpus and segmenter it prints the best time of several scans,
//! the throughput, the boundaries found, and the allocations one scan makes.
//! The segmenters are built before anything is timed; a scan is a walk over
//! every boundary, as winkin uses them. ICU's line and word segmenters use
//! its dictionaries for Chinese, Japanese and the Southeast Asian scripts, as
//! winkin's `dictionaries` feature does. The crate's grapheme and line
//! counts add the text's ends, which ICU yields and the crate's iterators do
//! not.
//!
//! The crate's line and word segmenters run with the dictionaries the
//! generator writes, loaded from the crate's sources, and without any, where
//! an SA run does not break inside and each ideograph is a word. Line breaks
//! differ from ICU's at the ends of SA runs, where ICU breaks before a space
//! and the crate does not; words differ where ICU's longest match takes
//! everything read at a miss. The crate's word counts add the text's ends,
//! as its line and grapheme counts do.
//!
//! ```text
//! cargo bench -p cleft --bench segmentation
//! cargo bench -p cleft --bench segmentation -- thai   # corpora whose name has "thai"
//! ```

use std::hint::black_box;
use std::time::{Duration, Instant};

use cleft::{
    Dictionaries, Dictionary, Graphemes, LineBreaks, LineOptions, Utf8Scalars, WordBreaks,
    WordOptions,
};
use icu_segmenter::options::{LineBreakOptions, WordBreakInvariantOptions};
use icu_segmenter::{GraphemeClusterSegmenter, LineSegmenter, WordSegmenter};
use testing::allocator::{Counting, count_allocations};
use testing::corpora::segmentation_corpora;

#[global_allocator]
static GLOBAL: Counting = Counting;

/// How long each cell keeps scanning, at least, before its best is taken.
const BUDGET: Duration = Duration::from_millis(1500);

/// The least and most scans of one cell.
const SCANS: (usize, usize) = (3, 50);

/// Returns the best time of repeated calls to `scan`, and what the last
/// returned.
fn best(mut scan: impl FnMut() -> usize) -> (Duration, usize) {
    let started = Instant::now();
    let mut best = Duration::MAX;
    let mut found = 0;
    let mut scans = 0;
    while scans < SCANS.0 || (scans < SCANS.1 && started.elapsed() < BUDGET) {
        let start = Instant::now();
        found = black_box(scan());
        best = best.min(start.elapsed());
        scans += 1;
    }
    (best, found)
}

fn main() {
    let filter = std::env::args()
        .skip(1)
        .find(|argument| !argument.starts_with("--"))
        .unwrap_or_default()
        .to_lowercase();
    let line = LineSegmenter::new_dictionary(LineBreakOptions::default());
    let grapheme = GraphemeClusterSegmenter::new();
    let word = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let load = |bytes: &'static [u8]| Dictionary::new(bytes).expect("a valid payload");
    let none = Dictionaries::new();
    let dictionaries = Dictionaries::new()
        .with_thai(load(include_bytes!("../src/dictionary/data/thai.dafsa")))
        .and_then(|set| set.with_lao(load(include_bytes!("../src/dictionary/data/lao.dafsa"))))
        .and_then(|set| set.with_khmer(load(include_bytes!("../src/dictionary/data/khmer.dafsa"))))
        .and_then(|set| {
            set.with_myanmar(load(include_bytes!("../src/dictionary/data/myanmar.dafsa")))
        })
        .and_then(|set| {
            set.with_chinese_japanese(load(include_bytes!("../src/dictionary/data/cj.dafsa")))
        })
        .expect("each payload in its slot");

    println!(
        "cleft's segmenters and ICU4X 2.3's, its dictionaries compiled in; best of at least {} scans.\n",
        SCANS.0
    );
    println!("| corpus | segmenter | time | MB/s | boundaries | allocations |");
    println!("| :-- | :-- | --: | --: | --: | --: |");
    for corpus in segmentation_corpora() {
        if !corpus.name.to_lowercase().contains(&filter) {
            continue;
        }
        let text = corpus.text.as_str();
        let scans: [(&str, &dyn Fn() -> usize); 8] = [
            ("ICU4X line", &|| line.segment_str(text).count()),
            ("line", &|| {
                LineBreaks::new(Utf8Scalars::new(text), LineOptions::new())
                    .with_dictionaries(&dictionaries)
                    .count()
                    + 2
            }),
            ("line, no dictionaries", &|| {
                LineBreaks::new(Utf8Scalars::new(text), LineOptions::new()).count() + 2
            }),
            ("ICU4X grapheme", &|| grapheme.segment_str(text).count()),
            ("grapheme", &|| {
                Graphemes::new(Utf8Scalars::new(text)).count() + 1
            }),
            ("ICU4X word", &|| word.segment_str(text).count()),
            ("word", &|| {
                WordBreaks::new(Utf8Scalars::new(text), WordOptions::new())
                    .with_dictionaries(&dictionaries)
                    .count()
                    + 2
            }),
            ("word, no dictionaries", &|| {
                WordBreaks::new(Utf8Scalars::new(text), WordOptions::new())
                    .with_dictionaries(&none)
                    .count()
                    + 2
            }),
        ];
        for (name, scan) in scans {
            let mut found = 0;
            let allocations = count_allocations(|| found = scan());
            let (time, again) = best(scan);
            assert_eq!(
                found, again,
                "{}: {name} found different boundaries",
                corpus.name
            );
            let megabytes = text.len() as f64 / (1 << 20) as f64;
            println!(
                "| {} | {name} | {:.2} ms | {:.1} | {found} | {allocations} |",
                corpus.name,
                time.as_secs_f64() * 1e3,
                megabytes / time.as_secs_f64(),
            );
        }
    }
}
