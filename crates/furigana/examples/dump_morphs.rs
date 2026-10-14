//! 形態素ごとの品詞・活用を表示する (品詞表と突き合わせる診断用)。
//! cargo run --release -p ja-furigana --example dump_morphs -- "食べました"
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a = furigana::analyzer::Analyzer::new()?;
    for t in std::env::args().skip(1) {
        for m in a.tokenize(&t) {
            println!(
                "{}\t{:?}\t{:?}/{:?}/{:?}/{:?}\t{:?}/{:?}",
                m.surface,
                m.reading,
                m.pos,
                m.pos_detail,
                m.pos_detail2,
                m.pos_detail3,
                m.conjugation_type,
                m.conjugation_form
            );
        }
    }
    Ok(())
}
