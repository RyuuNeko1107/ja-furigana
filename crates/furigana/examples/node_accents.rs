//! 診断用: 入力 1 行ごとに、 ノードの (表記, 読み, 品詞, 拍数, 語単体の核, 推定か) を TSV で出す。 OpenJTalk の生の形態素 (ojt_accent --nodes) との突き合わせ用。
//! cargo run --release -p ja-furigana --example node_accents -- <rules> <core> < corpus.txt
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::BufRead;
    let a: Vec<String> = std::env::args().collect();
    let f = furigana::Furigana::builder()
        .rules_dir(&a[1])
        .core_dict_dir(&a[2])
        .build()?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let nodes: Vec<String> = f
            .chain_debug_nodes_detail(&line)
            .into_iter()
            .map(|(s, r, p, m, ac, est)| format!("{s}|{r}|{p}|{m}|{ac}|{}", u8::from(est)))
            .collect();
        println!("{line}	{}", nodes.join("	"));
    }
    Ok(())
}
