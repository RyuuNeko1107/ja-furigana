//! VOICEVOX adapter — [`AccentResult`] を AquesTalk-風記法 (kana 記法) に変換する。
//!
//! ADR-0001: engine 固有フォーマットは lib 本体でなく adapter crate が持つ。
//! 仕様 reference: `voicevox_engine/tts_pipeline/kana_converter.py`
//! (詳細: ja-furigana repo `docs/PROPOSALS/intonation.md` §7.2)
//!
//! ## 記法
//!
//! - accent phrase を `/` で連結、 各 phrase は **カタカナ + `'` 1 個** (核モーラの直後)
//! - 平板 / accent 不明は **末尾 `'`** (VOICEVOX kana 記法は accent=0 を表現できない)
//! - 読点等は `、` (pause あり区切り)、 疑問文は末尾 `？`
//!
//! ## phrase 構成
//!
//! - accent 情報を持つ token は各 [`furigana::AccentPhrase`] が 1 phrase
//! - accent 不明 token は 平板 fallback で 1 phrase (intonation.md §7.2 案 A)
//! - **ひらがな surface の token (助詞/助動詞/送り仮名) は直前 phrase に連結** —
//!   `雨が` → `ア'メガ` (核確定 phrase への連結は核位置を維持、 平板/不明 phrase への
//!   連結は `'` が末尾へ動き高が続く形)
//! - 読めない token (絵文字 / 記号 / URL / 英字 passthrough 等、 reading がカナに
//!   落ちないもの) は出力から落とし、 phrase 境界だけ切る
//!
//! ## 使い方
//!
//! ```no_run
//! use furigana::Furigana;
//! use ja_furigana_voicevox::to_aques_kana;
//!
//! let f = Furigana::builder().estimate_accent(true).build().unwrap();
//! let kana = to_aques_kana(&f.to_accent("雨が降る"));
//! // VOICEVOX の POST /accent_phrases?is_kana=true にそのまま渡せる
//! ```

use furigana::accent_symbols::{to_mora_phrases, MoraPhrase, PhraseBreak};
use furigana::AccentResult;

/// 1 accent phrase を kana 記法へ (平板 / 不明は句末 `'`)。
fn render_phrase(phrase: &MoraPhrase) -> String {
    let pos = phrase.nucleus_pos();
    let mut out = String::new();
    for (i, m) in phrase.morae.iter().enumerate() {
        out.push_str(m);
        if i + 1 == pos {
            out.push('\'');
        }
    }
    out
}

/// [`AccentResult`] を AquesTalk-風記法 string に変換する。
///
/// 戻り値は VOICEVOX `POST /accent_phrases?is_kana=true` にそのまま渡せる形式。
/// 各 phrase に `'` がちょうど 1 個・空 phrase なし・句頭 `'` なし、 という
/// kana parser のエラー条件を構造的に満たす。 読める token が 1 つも無ければ
/// 空文字列を返す (caller 側で skip すること)。
///
/// 構築ロジック (phrase 分け / 助詞連結 / 記号の扱い) は lib 側の
/// [`furigana::accent_symbols`] と共有し、 本 crate は VOICEVOX 固有の
/// **記号の綴り方** だけを持つ:
///
/// - pause は種別を問わず `、` (kana 記法は長短を区別しない)
/// - 疑問文は全角 `？` を発話末に 1 個 (kana 記法は文中に置けない)
#[must_use]
pub fn to_aques_kana(result: &AccentResult) -> String {
    let symbols = to_mora_phrases(result);

    let mut out = String::new();
    let mut question = symbols.trailing == PhraseBreak::Question;
    for (i, phrase) in symbols.phrases.iter().enumerate() {
        if i > 0 {
            out.push(match phrase.break_before {
                PhraseBreak::None => '/',
                // kana 記法の pause は 1 種類だけ。 文中の `？` もここでは
                // pause として扱い、 尻上がりは末尾の `？` で表現する
                _ => '、',
            });
            if phrase.break_before == PhraseBreak::Question {
                question = true;
            }
        }
        out.push_str(&render_phrase(phrase));
    }
    if question && !out.is_empty() {
        out.push('？');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use furigana::Furigana;

    fn accent(f: &Furigana, input: &str) -> String {
        to_aques_kana(&f.to_accent(input))
    }

    #[test]
    fn atamadaka_with_particle_attached() {
        // 雨 = ア]メ (dict bracket) + が → ア'メガ
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("降る", "フル");
        assert_eq!(accent(&f, "雨が降る"), "ア'メガ/フル'");
    }

    #[test]
    fn heiban_particle_moves_apostrophe_to_end() {
        // 平板 (0) + が → 高が続く = サカナガ'
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("魚", "サ[カナ");
        assert_eq!(accent(&f, "魚が"), "サカナガ'");
    }

    #[test]
    fn odaka_keeps_nucleus_before_particle() {
        // 尾高 ハ[ナ] (accent=2) + が → ハナ'ガ (下がってから助詞)
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("花", "ハ[ナ]");
        assert_eq!(accent(&f, "花が"), "ハナ'ガ");
    }

    #[test]
    fn unknown_word_heiban_fallback() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("猫", "ネコ"); // bracket なし = accent 不明
        assert_eq!(accent(&f, "猫"), "ネコ'");
    }

    #[test]
    fn punctuation_becomes_pause() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        assert_eq!(accent(&f, "雨、雪"), "ア'メ、ユ'キ");
    }

    #[test]
    fn question_mark_appended_at_end() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        assert_eq!(accent(&f, "雨？"), "ア'メ？");
    }

    #[test]
    fn multi_phrase_entry_splits() {
        // [トウキョウ][ト]リツ → 2 phrases
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("都立", "[トウキョウ][ト]リツ");
        assert_eq!(accent(&f, "都立"), "トウキョウ'/ト'リツ");
    }

    #[test]
    fn unreadable_token_dropped() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        let out = accent(&f, "雨🌧");
        assert_eq!(out, "ア'メ");
    }

    #[test]
    fn empty_input_yields_empty_string() {
        let f = Furigana::minimal().unwrap();
        assert_eq!(accent(&f, ""), "");
    }

    #[test]
    fn particle_ha_he_wo_phonetic() {
        // kana 記法は音声表記: は→ワ / へ→エ / を→オ
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("見る", "ミル");
        assert_eq!(accent(&f, "雨は"), "ア'メワ");
        assert_eq!(accent(&f, "雨を見る"), "ア'メオ/ミル'");
    }

    #[test]
    fn every_phrase_has_exactly_one_apostrophe() {
        // kana parser のエラー条件 (ACCENT_NOTFOUND / ACCENT_TWICE / EMPTY_PHRASE) を
        // 構造的に満たすことの property check
        let f = Furigana::builder().estimate_accent(true).build().unwrap();
        for input in [
            "今日は雨が降るかもしれない。",
            "カーテンとエレベーターを買った",
            "田中さんが来た！",
            "峠道、注意？",
        ] {
            let out = accent(&f, input);
            for phrase in out.trim_end_matches('？').split(['/', '、']) {
                assert!(!phrase.is_empty(), "empty phrase in {out:?}");
                assert_eq!(
                    phrase.matches('\'').count(),
                    1,
                    "phrase {phrase:?} in {out:?}"
                );
                assert!(!phrase.starts_with('\''), "leading ' in {out:?}");
            }
        }
    }
}
