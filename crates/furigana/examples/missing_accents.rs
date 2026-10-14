//! 語単体の核が表に無く 平板で埋めたノードを 頻度順に数える (アクセント表を育てる入力)。
//!
//! ```sh
//! cargo run --release -p ja-furigana --example missing_accents -- <rules_dir> <core_dict_dir> < corpus.txt > missing.tsv
//! ```
//! 出力は `回数 TAB 表記 TAB 読み TAB 品詞` (回数の多い順)。

use std::collections::HashMap;
use std::io::{BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: missing_accents <rules_dir> <core_dict_dir> < corpus.txt");
        std::process::exit(2);
    }
    let f = furigana::Furigana::builder()
        .rules_dir(&args[1])
        .core_dict_dir(&args[2])
        .build()?;
    let mut cnt: HashMap<(String, String, String), u64> = HashMap::new();
    for line in std::io::stdin().lock().lines() {
        for k in f.chain_missing_accents(&line?) {
            *cnt.entry(k).or_default() += 1;
        }
    }
    let mut v: Vec<_> = cnt.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for ((s, r, p), n) in v {
        writeln!(out, "{n}\t{s}\t{r}\t{p}")?;
    }
    Ok(())
}
