//! 辞書に無い英字語の扱いの確認用 (alpha_try <rules> <core> < lines)
use furigana::accent_symbols::{SymbolOptions, UnknownAlpha};
use std::io::BufRead;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let f = furigana::Furigana::builder()
        .rules_dir(&a[1])
        .core_dict_dir(&a[2])
        .build()?;
    for line in std::io::stdin().lock().lines().map_while(Result::ok) {
        for (name, m) in [
            ("drop", UnknownAlpha::Drop),
            ("spell", UnknownAlpha::Spell),
            ("romaji", UnknownAlpha::Romaji),
        ] {
            let mut o = SymbolOptions::default();
            o.unknown_alpha = m;
            let r = f.to_mora_phrases_with(&line, o);
            let s: Vec<String> = r.phrases.phrases.iter().map(|p| p.morae.concat()).collect();
            println!("{line} [{name}] {} | unknown={:?}", s.join("/"), r.unknown);
        }
    }
    Ok(())
}
