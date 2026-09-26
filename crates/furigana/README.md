# ja-furigana

Japanese furigana (ruby) lookup library — Lindera + IPADIC ベースの形態素解析、
語彙辞書とルールはすべて TOML データ駆動。

> **Status**: 0.5.x (crates.io 公開中)。 読みは **Smart engine** (6 provider が band 付き候補を出し、
> Viterbi-like DP + band lexicographic 比較で path を選ぶ) で解決する。 公開 API の変更は
> [CHANGELOG](https://github.com/RyuuNeko1107/ja-furigana/blob/master/CHANGELOG.md) に記録
> (0.x のため minor で破壊的変更がありうる)。 MSRV: Rust 1.89+。

> **import 名に注意**: crate 名は `ja-furigana` ですが、Rust 上の `use` は
> `use furigana::Furigana;` (アンダースコアではなくそのまま `furigana`) です
> ([lib] name 設定により)。

```rust
use furigana::Furigana;

let mut f = Furigana::minimal()?;
f.add_reading("灰桜", "ハイザクラ");

println!("{}", f.to_ruby("灰桜の散る道"));
// → "{灰桜|はいざくら}の{散る|ちる}{道|みち}"

println!("{}", f.to_hiragana("灰桜の散る道"));
// → "はいざくらのちるみち"
# Ok::<_, furigana::FuriganaError>(())
```

辞書 / ルールを mount する場合は builder API を使います:

```rust
use furigana::Furigana;

let f = Furigana::builder()
    .core_dict_dir("/path/to/data")
    .rules_dir("/path/to/data")
    .user_dict_dir("/path/to/data/user")
    .overrides_file("/path/to/data/overrides.toml")
    .estimate_accent(true)   // 任意: to_accent で dict bracket の無い語も rule で accent 推定
    .build()?;
# Ok::<_, furigana::FuriganaError>(())
```

**外来語 (loanwords) サポート**: `core_dict_dir` / `user_dict_dir` 配下の
`role = "loanwords"` の TOML を recursive load。 英字の連続は `AlphabetPassthroughProvider` が
1 候補として切り出し、 完全一致 lookup (case-fold + 全角→半角) で IT 用語等を hit させる
([データ層の形式](https://github.com/RyuuNeko1107/ja-furigana-dict/blob/master/core/loanwords/it.toml) 参照)。
辞書に無い英単語は英字のまま (読みなし) で通す。

**出力ルール**: `to_hiragana` は surface の文字種で reading 表記を切替えます:
漢字を含む surface はひらがな化、 ASCII / カタカナ / 数字 / 記号のみの surface は
カタカナ統一。 例: `to_hiragana("Kubernetesが安定")` → `"クバネティスがあんてい"`。

CLI / HTTP server / 詳細は [project README](https://github.com/RyuuNeko1107/ja-furigana) を参照。

## License

MIT License.
