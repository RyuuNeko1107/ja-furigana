//! エンジンが実際に切り出す token 単位で (表記, 読み) の出現回数を数える (dict の保守用)。
//!
//! furigana-dict の `tools/gen_accent_lexicon.py --tokens` の入力 (アクセント専用の表の元) を作る。
//! ruby 出力は送り仮名を `{}` の外に出すので、 送り仮名付きの語 (強い / 違う) を数えるにはこちらを使う。
//!
//! ```sh
//! cargo run --release -p ja-furigana --example token_counts -- <rules_dir> <core_dict_dir> < corpus.txt > tokens.tsv
//! ```
//!
//! 出力は `表記 TAB 読み TAB 回数` (順不同)。 かな・漢字を含まない token と読みの空の token は数えない。
//! アクセント表自体は読み込まない (表の有無で区切りは変わらないが、 生成元を表に依存させないため)。

use std::collections::HashMap;
use std::io::{BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: token_counts <rules_dir> <core_dict_dir> < corpus.txt > tokens.tsv");
        std::process::exit(2);
    }
    let f = furigana::Furigana::builder()
        .rules_dir(&args[1])
        .core_dict_dir(&args[2])
        .accent_lexicon(false)
        .build()?;
    let mut cnt: HashMap<(String, String), u64> = HashMap::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        for t in f.to_accent(&line).tokens {
            let has_ja = t.surface.chars().any(|c| {
                ('\u{3041}'..='\u{30FA}').contains(&c) || ('\u{4E00}'..='\u{9FFF}').contains(&c)
            });
            if t.reading.is_empty() || !has_ja {
                continue;
            }
            *cnt.entry((t.surface, t.reading)).or_default() += 1;
        }
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for ((s, r), n) in cnt {
        writeln!(out, "{s}\t{r}\t{n}")?;
    }
    Ok(())
}
