//! アクセント句の境界で、 (前の語の品詞, 次の語の品詞) → 切った / 連結した の回数を数える (規則の効き方の診断)。
//! cargo run --release -p ja-furigana --example pos_pairs -- <rules> <core> < corpus.txt
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::BufRead;
    let a: Vec<String> = std::env::args().collect();
    let f = furigana::Furigana::builder()
        .rules_dir(&a[1])
        .core_dict_dir(&a[2])
        .build()?;
    for line in std::io::stdin().lock().lines() {
        for (i, (s, r, p, chained)) in f.chain_debug_nodes(&line?).into_iter().enumerate() {
            println!("{i}\t{s}\t{r}\t{p}\t{}", u8::from(chained));
        }
        println!("--");
    }
    Ok(())
}
