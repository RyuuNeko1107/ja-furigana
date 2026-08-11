//! TTS engine 向け「音声記号列」 adapter の共有コア。
//!
//! AquesTalk 系の記号列 (VOICEVOX の kana 記法もこの系統) は、 どの engine でも
//!
//! - カタカナ + 長音符で読みを書く
//! - アクセント句に分ける
//! - 句ごとにアクセント核の位置を 1 つ持つ
//! - 句読点で間 (pause) を入れる
//!
//! という骨格が共通で、 違うのは **記号の綴り方** (`'` の置き方 / pause 記号 /
//! 疑問符 / 無声化記号の有無) だけ。 その共通部分 —— [`AccentResult`] を
//! 「モーラ列 + 核位置 + 区切りの強さ」 へ落とす所 —— をここに置き、
//! engine 固有の render は adapter crate 側 (ADR-0001) に任せる。
//!
//! ```
//! use furigana::accent_symbols::{to_mora_phrases, PhraseBreak};
//! # use furigana::Furigana;
//! # let mut f = Furigana::minimal().unwrap();
//! # f.add_reading("雨", "ア]メ");
//! let symbols = to_mora_phrases(&f.to_accent("雨が"));
//! assert_eq!(symbols.phrases[0].morae, ["ア", "メ", "ガ"]);
//! assert_eq!(symbols.phrases[0].nucleus, Some(1));
//! assert_eq!(symbols.trailing, PhraseBreak::None);
//! ```

use crate::api::{AccentResult, AccentToken};
use crate::kana::hira_to_kata;

/// アクセント句の区切りの強さ。 engine 固有の記号への写像は adapter が持つ。
///
/// 順序は 「弱い < 強い」 で、 記号 token が連続したときは強い方を採る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PhraseBreak {
    /// pause なしの句境界 (中黒 「・」 もこれ: 語中なので間を空けない)
    #[default]
    None,
    /// 短い pause (読点 / 空白由来)
    Short,
    /// 長い pause (句点 / 感嘆符由来)
    Long,
    /// 疑問文の終わり (= その文だけ尻上がり)
    Question,
}

/// 1 アクセント句 (モーラ列 + 核位置)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoraPhrase {
    /// モーラ列 (拗音 / 小書き母音は直前と合算済み、 カタカナ + `ー` のみ)
    pub morae: Vec<String>,
    /// アクセント核のモーラ位置 (1-based)。 `None` = 平板 (0 型) または accent 不明。
    ///
    /// AquesTalk 系記法は 0 型を直接表現できないので、 adapter は通常 `None` を
    /// 句末の核として render する。
    pub nucleus: Option<usize>,
    /// **この句の前** に置く区切り (先頭句は常に [`PhraseBreak::None`])
    pub break_before: PhraseBreak,
}

impl MoraPhrase {
    /// 核のモーラ位置 (1-based)。 平板 / 不明 / 範囲外は句末に落とす。
    ///
    /// 空の句には核を置けないので `0` を返す (adapter 側で空句は作らないこと)。
    #[must_use]
    pub fn nucleus_pos(&self) -> usize {
        match self.nucleus {
            Some(p) if p >= 1 && p <= self.morae.len() => p,
            _ => self.morae.len(),
        }
    }
}

/// [`to_mora_phrases`] の戻り値。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MoraPhrases {
    /// アクセント句列 (空句は含まない)。 読める token が無ければ空。
    pub phrases: Vec<MoraPhrase>,
    /// 発話末に残った区切り (= 入力が記号で終わっていた場合)。
    pub trailing: PhraseBreak,
}

/// カタカナ文字列をモーラ単位に分割する (拗音 / 小書き母音は直前と合算)。
#[must_use]
pub fn mora_split(reading: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for c in reading.chars() {
        if is_small_kana(c) {
            if let Some(last) = out.last_mut() {
                last.push(c);
                continue;
            }
        }
        out.push(c.to_string());
    }
    out
}

/// 記号列に書ける文字 (カタカナ + 長音符) か。
#[must_use]
pub fn is_symbol_kana(c: char) -> bool {
    matches!(c, 'ァ'..='ヶ' | 'ー')
}

fn is_small_kana(c: char) -> bool {
    matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')
}

/// 記号 token の区切り種別。 `None` = 記号 token ではない。
fn token_break(surface: &str) -> Option<PhraseBreak> {
    if surface.is_empty() {
        return None;
    }
    let mut brk: Option<PhraseBreak> = None;
    for c in surface.chars() {
        let b = match c {
            '？' | '?' => PhraseBreak::Question,
            '。' | '．' | '！' | '!' => PhraseBreak::Long,
            '、' | '，' | ',' | '…' | '‥' => PhraseBreak::Short,
            // 中黒は 「ジョン・スミス」 のような語中区切り = pause を入れない
            '・' => PhraseBreak::None,
            c if c.is_whitespace() => PhraseBreak::Short,
            _ => return None,
        };
        brk = Some(brk.map_or(b, |cur| cur.max(b)));
    }
    brk
}

/// 記号列に出せない文字を落としたカタカナ読み。
fn symbol_reading(reading: &str) -> String {
    hira_to_kata(reading)
        .chars()
        .filter(|c| is_symbol_kana(*c))
        .collect()
}

fn token_phrases(token: &AccentToken) -> Vec<MoraPhrase> {
    token
        .accent_phrases
        .iter()
        .filter_map(|ap| {
            let morae = mora_split(&symbol_reading(&ap.reading));
            if morae.is_empty() {
                return None;
            }
            Some(MoraPhrase {
                morae,
                nucleus: match ap.accent {
                    Some(a) if a >= 1 => Some(a as usize),
                    _ => None, // 平板 (0) / 不明
                },
                break_before: PhraseBreak::None,
            })
        })
        .collect()
}

/// 組み立て中の状態。
#[derive(Default)]
struct Builder {
    out: MoraPhrases,
    open: Option<MoraPhrase>,
    /// 直前に確定した句の後ろに置く区切り (次の句の `break_before` になる)
    pending: Option<PhraseBreak>,
}

impl Builder {
    fn flush(&mut self) {
        if let Some(mut p) = self.open.take() {
            if !p.morae.is_empty() {
                p.break_before = self.pending.take().unwrap_or(PhraseBreak::None);
                self.out.phrases.push(p);
            }
        }
    }

    fn add_break(&mut self, brk: PhraseBreak) {
        self.flush();
        self.pending = Some(self.pending.map_or(brk, |cur| cur.max(brk)));
    }

    fn open_phrase(&mut self, p: MoraPhrase) {
        self.flush();
        self.open = Some(p);
    }

    fn finish(mut self) -> MoraPhrases {
        self.flush();
        self.out.trailing = self.pending.unwrap_or(PhraseBreak::None);
        self.out
    }
}

/// [`AccentResult`] を 「モーラ列 + 核位置 + 区切り」 の列へ落とす。
///
/// 変換規則:
///
/// - accent 情報を持つ token は [`AccentPhrase`](crate::AccentPhrase) ごとに 1 句
/// - accent 不明の内容語は平板 fallback で 1 句
/// - ひらがな surface の token (助詞 / 助動詞 / 送り仮名) は直前の句に連結する
///   (`雨が` → 1 句 `アメガ`、 核位置は維持)
/// - 助詞 `は` / `へ` / `を` は音声表記へ (ワ / エ / オ)
/// - 読めない token (絵文字 / 記号 / URL / 英字 passthrough 等) は落として句境界だけ切る
/// - 句読点は [`PhraseBreak`] へ (文単位。 入力末尾の記号は
///   [`MoraPhrases::trailing`] に残る)
///
/// 空の句は作らないので、 adapter は 「各句に核を 1 つ」 を無条件に満たせる。
#[must_use]
pub fn to_mora_phrases(result: &AccentResult) -> MoraPhrases {
    let mut b = Builder::default();

    for token in &result.tokens {
        if let Some(brk) = token_break(&token.surface) {
            b.add_break(brk);
            continue;
        }

        // 助詞の発音変換: 記号列は音声表記なので は→ワ / へ→エ / を→オ。
        // 単独 token の場合のみ (語中の ハ/ヘ/ヲ は無関係)。 accent_phrases が
        // 付いていても 1 モーラの助詞なので、 変換した読みを優先する。
        let particle = match token.surface.as_str() {
            "は" => Some("ワ"),
            "へ" => Some("エ"),
            "を" => Some("オ"),
            _ => None,
        };
        let reading = match particle {
            Some(r) => r.to_string(),
            None => symbol_reading(&token.reading),
        };
        if reading.is_empty() {
            b.flush();
            continue;
        }

        if particle.is_none() && !token.accent_phrases.is_empty() {
            // dict bracket / 推定由来の句列をそのまま採用。
            // 末尾句は open にして後続の助詞連結を受ける。
            let mut phrases = token_phrases(token);
            if let Some(last) = phrases.pop() {
                for p in phrases {
                    b.open_phrase(p);
                }
                b.open_phrase(last);
            } else {
                b.flush();
            }
            continue;
        }

        let is_hiragana_function_word = !token.surface.is_empty()
            && token
                .surface
                .chars()
                .all(|c| matches!(c, 'ぁ'..='ゖ' | 'ー'));

        if is_hiragana_function_word {
            if let Some(p) = b.open.as_mut() {
                // 直前句へ連結。 核確定済みなら位置維持 (アメ + ガ で核は 1 のまま)、
                // 平板 / 不明は `None` のままなので核が句末へ動く
                p.morae.extend(mora_split(&reading));
                continue;
            }
        }

        // accent 不明の内容語 → 自分の句 (平板 fallback)
        b.open_phrase(MoraPhrase {
            morae: mora_split(&reading),
            nucleus: None,
            break_before: PhraseBreak::None,
        });
    }

    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Furigana;

    fn phrases(f: &Furigana, input: &str) -> MoraPhrases {
        to_mora_phrases(&f.to_accent(input))
    }

    fn shape(p: &MoraPhrases) -> Vec<(String, Option<usize>, PhraseBreak)> {
        p.phrases
            .iter()
            .map(|ph| (ph.morae.concat(), ph.nucleus, ph.break_before))
            .collect()
    }

    #[test]
    fn mora_split_merges_small_kana() {
        assert_eq!(mora_split("キョウ"), ["キョ", "ウ"]);
        assert_eq!(mora_split("カーテン"), ["カ", "ー", "テ", "ン"]);
        // 先頭に小書きが来ても panic しない (直前が無いので単独モーラ)
        assert_eq!(mora_split("ャ"), ["ャ"]);
        assert!(mora_split("").is_empty());
    }

    #[test]
    fn particle_attaches_to_previous_phrase_keeping_nucleus() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        let p = phrases(&f, "雨が");
        assert_eq!(
            shape(&p),
            [("アメガ".to_string(), Some(1), PhraseBreak::None)]
        );
    }

    #[test]
    fn heiban_phrase_has_no_nucleus() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("魚", "サ[カナ");
        let p = phrases(&f, "魚が");
        assert_eq!(p.phrases[0].nucleus, None);
        // 平板は句末を核位置として扱う
        assert_eq!(p.phrases[0].nucleus_pos(), 4);
    }

    #[test]
    fn ha_he_wo_become_phonetic_even_with_accent_phrases() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("は", "[ハ]");
        f.add_reading("見る", "ミル");
        assert_eq!(shape(&phrases(&f, "雨は"))[0].0, "アメワ");
        assert_eq!(shape(&phrases(&f, "雨を見る"))[0].0, "アメオ");
    }

    #[test]
    fn punctuation_becomes_break_before_next_phrase() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        f.add_reading("風", "カ[ゼ]");
        let p = phrases(&f, "雨、雪。風");
        assert_eq!(
            shape(&p).iter().map(|(_, _, b)| *b).collect::<Vec<_>>(),
            [PhraseBreak::None, PhraseBreak::Short, PhraseBreak::Long]
        );
        assert_eq!(p.trailing, PhraseBreak::None);
    }

    #[test]
    fn question_is_per_sentence_and_trailing_is_kept() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        f.add_reading("雪", "ユ]キ");
        // 文中の `？` は 次の句の break、 文末の `。` は trailing
        let p = phrases(&f, "雨？雪。");
        assert_eq!(p.phrases[1].break_before, PhraseBreak::Question);
        assert_eq!(p.trailing, PhraseBreak::Long);
    }

    #[test]
    fn nakaguro_does_not_pause() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("甲", "コ]ウ");
        f.add_reading("乙", "オ]ツ");
        assert_eq!(
            phrases(&f, "甲・乙").phrases[1].break_before,
            PhraseBreak::None
        );
    }

    #[test]
    fn unreadable_token_is_dropped_but_breaks_phrase() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("雨", "ア]メ");
        let p = phrases(&f, "雨🌧");
        assert_eq!(p.phrases.len(), 1);
        assert_eq!(p.phrases[0].morae.concat(), "アメ");
    }

    #[test]
    fn empty_input_yields_no_phrases() {
        let f = Furigana::minimal().unwrap();
        let p = phrases(&f, "");
        assert!(p.phrases.is_empty());
        assert_eq!(p.trailing, PhraseBreak::None);
    }

    #[test]
    fn multi_phrase_entry_splits_into_phrases() {
        let mut f = Furigana::minimal().unwrap();
        f.add_reading("都立", "[トウキョウ][ト]リツ");
        let p = phrases(&f, "都立");
        assert_eq!(
            shape(&p),
            [
                // `]` 無しの句は句末核として parse される (4 モーラ目 = ウ)
                ("トウキョウ".to_string(), Some(4), PhraseBreak::None),
                ("トリツ".to_string(), Some(1), PhraseBreak::None),
            ]
        );
    }

    #[test]
    fn no_phrase_is_empty() {
        // adapter が 「各句に核 1 つ」 を無条件に満たせることの前提
        let f = Furigana::builder().estimate_accent(true).build().unwrap();
        for input in ["今日は雨が降るかもしれない。", "田中さんが来た！", "…?"]
        {
            for p in to_mora_phrases(&f.to_accent(input)).phrases {
                assert!(!p.morae.is_empty(), "empty phrase for {input:?}");
                assert!(p.nucleus_pos() >= 1, "no nucleus for {input:?}");
                assert!(
                    p.morae.iter().all(|m| m.chars().all(is_symbol_kana)),
                    "non-kana mora in {input:?}: {:?}",
                    p.morae
                );
            }
        }
    }
}
