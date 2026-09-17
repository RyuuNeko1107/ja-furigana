//! Lindera tokenize が to_ruby 全体に占める割合 + dict bucket 走査コストの切り分け。
//!
//! ```sh
//! FURIGANA_BENCH_CORE=../furigana-dict/core \
//! FURIGANA_BENCH_RULES=../furigana-dict/rules \
//!   cargo bench -p ja-furigana --bench lindera_share
//! ```
//!
//! - `lindera_only`: 形態素解析だけ (= 下限コスト)
//! - `to_ruby`: 同じ入力の full pipeline
//! - `hot_vs_cold`: dict の先頭 char bucket が巨大な字 (御713 / 大358 / 一163) 主体の
//!   入力と、 bucket 1 の字ばかりの入力を同程度の byte 数で比較。
//!   差が出れば `DictBridgeProvider::emit_entries` の bucket 全走査が hot と判る。

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use furigana::analyzer::Analyzer;
use furigana::Furigana;
use std::hint::black_box;

fn build() -> Furigana {
    let (core, rules) = (
        std::env::var("FURIGANA_BENCH_CORE").expect("FURIGANA_BENCH_CORE"),
        std::env::var("FURIGANA_BENCH_RULES").expect("FURIGANA_BENCH_RULES"),
    );
    let f = Furigana::builder()
        .rules_dir(&rules)
        .core_dict_dir(&core)
        .build()
        .expect("build with real dict");
    f.preload().expect("preload");
    eprintln!("[bench] real dict mounted: {} entries", f.dict_size());
    f
}

const MEDIUM: &str =
    "今日は北海道の鹿児島と秋葉原で一期一会の出会いがあった。明日は仲人の家に行く。";

/// bucket が巨大な字 (御 713 / 大 358 / 三 187 / 小 184 / 天 172 / 一 163) 主体。
const HOT: &str = "御飯と御茶を大きく三つ小さく天から一つ御礼申し上げます。";
/// bucket 1 の字ばかり (同程度の byte 数)。
const COLD: &str = "曖昧模糊たる薔薇窯変釉薬瑠璃硝子燐寸蝋燭絨毯襖障子。";

fn bench_share(c: &mut Criterion) {
    let f = build();
    let analyzer = Analyzer::new().expect("analyzer");

    let mut g = c.benchmark_group("lindera_share");
    for (label, text) in [("medium", MEDIUM), ("hot", HOT), ("cold", COLD)] {
        g.bench_with_input(BenchmarkId::new("lindera_only", label), text, |b, t| {
            b.iter(|| black_box(analyzer.tokenize(t)));
        });
        g.bench_with_input(BenchmarkId::new("to_ruby", label), text, |b, t| {
            b.iter(|| black_box(f.to_ruby(t)));
        });
    }
    g.finish();
}

criterion_group!(benches, bench_share);
criterion_main!(benches);
