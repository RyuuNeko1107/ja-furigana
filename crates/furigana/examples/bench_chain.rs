//! アクセント句 (連結規則) の各段階の時間とメモリを測る (最適化の足場)。
//!
//! ```sh
//! cargo run --release -p ja-furigana --example bench_chain -- <rules_dir> <core_dict_dir> < corpus.txt
//! ```
//!
//! 段階: 読み込み / hiragana / to_accent / Lindera の再解析だけ / to_accent_phrases。

use std::io::BufRead;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: bench_chain <rules_dir> <core_dict_dir> < corpus.txt");
        std::process::exit(2);
    }
    let lines: Vec<String> = std::io::stdin().lock().lines().map_while(Result::ok).filter(|l| !l.trim().is_empty()).collect();
    let t0 = Instant::now();
    let f = furigana::Furigana::builder().rules_dir(&args[1]).core_dict_dir(&args[2]).build()?;
    f.preload()?;
    println!("読み込み: {:.0} ms ({} 文)", t0.elapsed().as_secs_f64() * 1e3, lines.len());

    // 暖機
    for l in lines.iter().take(200) {
        let _ = f.to_hiragana(l);
        let _ = f.to_accent_phrases(l);
    }
    let bench = |name: &str, run: &dyn Fn(&str) -> usize| {
        let t = Instant::now();
        let mut n = 0usize;
        for l in &lines {
            n += run(l);
        }
        let dt = t.elapsed().as_secs_f64();
        println!("{name:<28} {:>7.0} ms  ({:>6.0} 文/秒)  [{n}]", dt * 1e3, lines.len() as f64 / dt);
    };
    bench("to_hiragana", &|l| f.to_hiragana(l).len());
    bench("to_accent (token の核)", &|l| f.to_accent(l).tokens.len());
    bench("tokenize (Lindera だけ)", &|l| f.tokenize_pos_only(l));
    bench("to_accent_phrases (連結)", &|l| f.to_accent_phrases(l).len());
    Ok(())
}
