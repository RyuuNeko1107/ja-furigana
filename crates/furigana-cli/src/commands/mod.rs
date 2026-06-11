//! サブコマンド実装

pub mod dict;
pub mod dict_pull;
pub mod lookup;
pub mod repl;
pub mod serve;

use crate::paths::Paths;
use anyhow::Result;
use furigana::Furigana;

/// CLI サブコマンドが共通で使う Furigana インスタンスを構築する
///
/// 起動時に `<data_dir>` 配下の rules / dict をスキャン:
/// - `rules/`           : 助数詞・文脈・スケール等のエンジンルール
/// - `dict/core/`       : 配布版語彙辞書 (`furigana dict pull` で取得)
/// - `dict/user/`       : ユーザー追加辞書
/// - `dict/overrides.toml` : 強制上書き
///
/// データ未配置の場合は warn を出すだけで起動継続 (degraded mode)。
/// すべて [`furigana-dict`](https://github.com/RyuuNeko1107/ja-furigana-dict)
/// から `furigana dict pull` で取得する想定。
pub fn build_furigana(paths: &Paths) -> Result<Furigana> {
    let mut b = Furigana::builder();
    let mut data_loaded = false;

    let rules = paths.rules_dir();
    if rules.exists() {
        b = b.rules_dir(&rules);
        data_loaded = true;
    }

    let core = paths.dict_core_dir();
    if core.exists() {
        b = b.core_dict_dir(&core);
        data_loaded = true;
    }
    let user = paths.dict_user_dir();
    if user.exists() {
        b = b.user_dict_dir(&user);
    }
    let overrides = paths.overrides_file();
    if overrides.exists() {
        b = b.overrides_file(&overrides);
    }
    // 旧 Strict-only path (= loanwords / single_overrides) は alpha.15 で撤廃済。
    // loanwords は alpha.21 で AlphabetPassthroughProvider に再統合済 (= core_dict_dir
    // 配下の role = "loanwords" file を builder が自動で拾う)。
    // single_overrides 相当は dict 側 `core/kanji/overrides.toml` の [[kanji]] block に移行。

    if !data_loaded {
        tracing::warn!(
            "rules / 辞書が未配置です ({}). 機能が制限された状態で起動します。\n  \
             先に `furigana dict pull` を実行するか、\n  \
             --data-dir で別の場所を指定してください。",
            paths.data_dir.display()
        );
    }

    Ok(b.build()?)
}
