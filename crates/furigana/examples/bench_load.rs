//! 起動時間の内訳: 辞書 / アクセント表 / 形態素解析器 (Lindera) の準備。
//! cargo run --release -p ja-furigana --example bench_load -- <rules_dir> <core_dict_dir>
use std::time::Instant;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<String> = std::env::args().collect();
    let (rules, core) = (&a[1], &a[2]);
    for (label, accent) in [("アクセント表なし", false), ("アクセント表あり", true)] {
        let t = Instant::now();
        let f = furigana::Furigana::builder().rules_dir(rules).core_dict_dir(core).accent_lexicon(accent).build()?;
        let build = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        f.preload()?;
        let pre = t.elapsed().as_secs_f64() * 1e3;
        println!("{label}: build(辞書+表) {build:.0} ms / preload(Lindera) {pre:.0} ms / 合計 {:.0} ms", build + pre);
    }
    Ok(())
}
