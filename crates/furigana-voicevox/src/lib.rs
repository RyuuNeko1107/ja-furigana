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

use furigana::{AccentResult, AccentToken};

/// ひらがな → カタカナ (U+3041..U+3096 を +0x60 シフト)。
fn hira_to_kata(s: &str) -> String {
    s.chars()
        .map(|c| {
            if ('ぁ'..='ゖ').contains(&c) {
                char::from_u32(c as u32 + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn is_katakana_or_prolonged(c: char) -> bool {
    matches!(c, 'ァ'..='ヶ' | 'ー')
}

fn is_small_kana(c: char) -> bool {
    matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ')
}

/// カタカナ文字列を mora 単位に分割 (拗音 / 小書き母音は直前と合算)。
fn mora_split(reading: &str) -> Vec<String> {
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

/// 組み立て中の 1 accent phrase。
struct Phrase {
    morae: Vec<String>,
    /// `'` を置く mora 位置 (1-based)。 `None` = 平板/不明 (= 末尾 `'`)
    nucleus: Option<usize>,
}

impl Phrase {
    fn render(&self) -> String {
        let pos = match self.nucleus {
            // VOICEVOX kana 記法は 0 型を表現できないため 平板/不明は末尾 `'`
            Some(p) if p >= 1 && p <= self.morae.len() => p,
            _ => self.morae.len(),
        };
        let mut out = String::new();
        for (i, m) in self.morae.iter().enumerate() {
            out.push_str(m);
            if i + 1 == pos {
                out.push('\'');
            }
        }
        out
    }
}

#[derive(Clone, Copy)]
enum Sep {
    /// `/` — pause なし phrase 境界
    Slash,
    /// `、` — pause あり境界 (読点 / 文末由来)
    Pause,
}

#[derive(Default)]
struct Builder {
    rendered: Vec<(Sep, String)>,
    open: Option<Phrase>,
    pending_pause: bool,
    question: bool,
}

impl Builder {
    /// 組み立て中 phrase を確定 (空なら捨てる)。
    fn flush(&mut self) {
        if let Some(p) = self.open.take() {
            if !p.morae.is_empty() {
                let sep = if self.pending_pause {
                    Sep::Pause
                } else {
                    Sep::Slash
                };
                self.rendered.push((sep, p.render()));
                self.pending_pause = false;
            }
        }
    }

    fn finish(mut self) -> String {
        self.flush();
        let mut out = String::new();
        for (i, (sep, phrase)) in self.rendered.iter().enumerate() {
            if i > 0 {
                out.push(match sep {
                    Sep::Pause => '、',
                    Sep::Slash => '/',
                });
            }
            out.push_str(phrase);
        }
        if self.question && !out.is_empty() {
            out.push('？');
        }
        out
    }
}

fn is_punctuation_token(surface: &str) -> bool {
    !surface.is_empty()
        && surface.chars().all(|c| {
            matches!(
                c,
                '。' | '、' | '．' | '，' | '！' | '!' | '？' | '?' | '…' | '‥' | '・'
            ) || c.is_whitespace()
        })
}

fn token_phrases(token: &AccentToken) -> Vec<Phrase> {
    token
        .accent_phrases
        .iter()
        .filter_map(|ap| {
            let morae = mora_split(&hira_to_kata(&ap.reading));
            if morae.is_empty() {
                return None;
            }
            let nucleus = match ap.accent {
                Some(a) if a >= 1 => Some(a as usize),
                _ => None, // 平板 (0) / 不明 → 末尾 '
            };
            Some(Phrase { morae, nucleus })
        })
        .collect()
}

/// [`AccentResult`] を AquesTalk-風記法 string に変換する。
///
/// 戻り値は VOICEVOX `POST /accent_phrases?is_kana=true` にそのまま渡せる形式。
/// 各 phrase に `'` がちょうど 1 個・空 phrase なし・句頭 `'` なし、 という
/// kana parser のエラー条件を構造的に満たす。 読める token が 1 つもなければ
/// 空文字列を返す (caller 側で skip すること)。
#[must_use]
pub fn to_aques_kana(result: &AccentResult) -> String {
    let mut b = Builder::default();

    for token in &result.tokens {
        if is_punctuation_token(&token.surface) {
            if token.surface.chars().any(|c| matches!(c, '？' | '?')) {
                b.question = true;
            }
            b.flush();
            b.pending_pause = true;
            continue;
        }

        // 助詞の発音変換: kana 記法は音声表記なので は→ワ / へ→エ / を→オ。
        // 単独 token の場合のみ (語中の ハ/ヘ/ヲ は無関係)。
        let reading = match token.surface.as_str() {
            "は" => "ワ".to_string(),
            "へ" => "エ".to_string(),
            "を" => "オ".to_string(),
            _ => hira_to_kata(&token.reading),
        };
        if reading.is_empty() || !reading.chars().all(is_katakana_or_prolonged) {
            // 読めない token は落として phrase 境界だけ切る
            b.flush();
            continue;
        }

        if !token.accent_phrases.is_empty() {
            // dict bracket / 推定由来の phrase 列をそのまま採用。
            // 末尾 phrase は open にして後続の助詞連結を受ける。
            b.flush();
            let mut phrases = token_phrases(token);
            if let Some(last) = phrases.pop() {
                for p in phrases {
                    b.open = Some(p);
                    b.flush();
                }
                b.open = Some(last);
            }
            continue;
        }

        let is_hiragana_function_word = token
            .surface
            .chars()
            .all(|c| matches!(c, 'ぁ'..='ゖ' | 'ー'))
            && !token.surface.is_empty();

        if is_hiragana_function_word {
            if let Some(p) = b.open.as_mut() {
                // 直前 phrase へ連結。 核確定済みなら位置維持 (ア'メガ)、
                // 平板/不明は None のまま = ' が末尾へ動く (サカナガ')
                p.morae.extend(mora_split(&reading));
                continue;
            }
        }

        // accent 不明の content token → 自分の phrase (平板 fallback)
        b.flush();
        b.open = Some(Phrase {
            morae: mora_split(&reading),
            nucleus: None,
        });
    }

    b.finish()
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
