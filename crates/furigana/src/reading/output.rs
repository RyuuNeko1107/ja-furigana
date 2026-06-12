//! [`ReadingToken`] 列の出力形式変換
//!
//! - `tokens_to_hiragana`: TTS / ひらがな展開 (surface 文字種ごとに切替)
//! - `tokens_to_ruby`    : `{kanji|hiragana}` 形式 (HTML ルビ生成等の前段)

use super::ReadingToken;
use crate::char_class;
use crate::kana;

/// surface が「漢字を含む」 かどうかで reading 出力を切替えるルールに使う判定
///
/// 出力ルール:
/// - **漢字を含む** → reading をひらがな化 (例: 「灰桜」 → 「はいざくら」)
/// - **漢字を含まない** (= ASCII / 全角英字 / カタカナ / ひらがな / 数字 / 記号 のみ) →
///   reading を **カタカナのまま維持** (例: 「Kubernetes」 → 「クバネティス」、
///   「3本」 (漢字「本」 含む) → 「さんぼん」 だが、 「3」 単独 → 「サン」)
///
/// 「ひらがな / カタカナ / ASCII 英字 のみ」 の surface は通常 reading が surface と
/// 同等 (ひらがなは reading なしで surface のまま、 カタカナや英字は dict ヨミ) なので
/// そのまま出力すれば自然な日本語表現になる。
fn surface_has_kanji(surface: &str) -> bool {
    surface.chars().any(kana::is_kanji_char)
}

/// surface が **全部 ひらがな or カタカナ** (= kanji / alphabet 等を含まない) かを判定。
///
/// user が既に kana で書いた surface は、 形態素解析の reading が異なっても
/// (= 「は」 particle で UniDic が 「ワ」 を返す等) surface をそのまま使う方針。
fn surface_is_all_kana(surface: &str) -> bool {
    !surface.is_empty()
        && surface
            .chars()
            .all(|c| kana::is_hiragana_char(c) || char_class::is_katakana_loose_char(c))
}

/// surface に **ASCII / 全角 alphabet** が含まれるかを判定。
///
/// hiragana mode で 「数字 / 記号 のみ」 vs 「alphabet 含み」 を区別する用途:
/// - alphabet 含み → loanword phonetic で **katakana に統一** (「Kubernetes」 → 「クバネティス」)
/// - 数字 / 記号のみ → 周辺の hiragana 文脈に合わせて **hiragana 化** (「2」 → 「に」、 「〜」 → 「から」)
fn surface_has_alphabet(surface: &str) -> bool {
    surface.chars().any(|c| {
        c.is_ascii_alphabetic() || matches!(c, '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}')
    })
}

/// トークン列をひらがな文字列に変換 (= 表記読み版)
///
/// **方針**: ja-furigana は 「読み」 ライブラリであり、 「発音」 処理は TTS engine
/// (= VOICEVOX 等の OpenJTalk 系) の責務。 助詞 「は」 → 「わ」 等の音韻変換は
/// 行わず、 入力表記を尊重した出力を返す。 TTS で 「自然な発音」 を得たい場合は
/// 出力を VOICEVOX 等に流すと、 TTS 側で適切に処理される (= raw 平文でも
/// VOICEVOX 内部で 助詞変換 / アクセント推定が走る)。
///
/// - 読みあり + surface が漢字を含む → reading を **ひらがな化** (kata_to_hira)
/// - 読みあり + surface が全 kana → **surface をそのまま** (= 入力表記尊重、
///   「は」 + UniDic 発音 「ワ」 でも 「は」 のまま、 user が書いた kana 維持)
/// - 読みあり + surface が alphabet 含み → reading を **カタカナに統一**
///   (= 「Kubernetes」+「クバネティス」 のような loanword phonetic は kata 慣習)
/// - 読みあり + その他 (= 数字 / 記号のみ) → reading を **ひらがな化** (kata_to_hira)
///   周辺の hiragana 文脈に合わせて 「2〜3回」 → 「に から さんかい」 のように統一表現
///   (★alpha.21、 旧 = 数字 / 記号も katakana 統一だったが、 「ニカラさんかい」 のような
///   kata / hira 混在違和感を解消)
/// - 読みなし → surface をそのまま
#[must_use]
pub fn tokens_to_hiragana(tokens: &[ReadingToken]) -> String {
    let mut out = String::new();
    for t in tokens {
        if let Some(reading) = &t.reading {
            if surface_has_kanji(&t.surface) {
                out.push_str(&kana::kata_to_hira(reading));
            } else if surface_is_all_kana(&t.surface) {
                // 全 kana surface は user 入力 (= 「こんにちは」 等) → 表記そのまま維持
                // 助詞 は→わ 等の音韻変換は TTS engine (VOICEVOX 等) に任せる
                out.push_str(&t.surface);
            } else if surface_has_alphabet(&t.surface) {
                // alphabet loanword phonetic → カタカナに統一
                // (例: 「Kubernetes」+「クバネティス」、「C++」+「シープラスプラス」)
                out.push_str(&kana::hira_to_kata(reading));
            } else {
                // 数字 / 記号のみ → 周辺 hiragana 文脈に合わせる
                // (例: 「2」 → 「に」、 「〜」 → 「から」、 「+」 → 「ぷらす」)
                out.push_str(&kana::kata_to_hira(reading));
            }
        } else {
            out.push_str(&t.surface);
        }
    }
    out
}

/// トークン列を `{漢字|ひらがな}` 形式の ruby 文字列に変換
///
/// - 読みあり + surface が **全部 kana** → surface をそのまま (ruby 不要)
/// - 読みあり + ひらがな化後 surface と一致 → surface をそのまま (ruby 不要)
/// - 読みあり + その他 → `{surface|reading}`
/// - 読みなし → surface をそのまま
#[must_use]
pub fn tokens_to_ruby(tokens: &[ReadingToken]) -> String {
    let mut out = String::new();
    for t in tokens {
        match &t.reading {
            Some(reading) => {
                if surface_is_all_kana(&t.surface) {
                    // user が kana で書いた surface は ruby 不要
                    out.push_str(&t.surface);
                    continue;
                }
                let hira = kana::kata_to_hira(reading);
                if hira == t.surface {
                    out.push_str(&t.surface);
                } else {
                    out.push('{');
                    out.push_str(&t.surface);
                    out.push('|');
                    out.push_str(&hira);
                    out.push('}');
                }
            }
            None => out.push_str(&t.surface),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hiragana_basic() {
        let tokens = vec![
            ReadingToken {
                surface: "灰桜".to_string(),
                reading: Some("ハイザクラ".to_string()),
            },
            ReadingToken {
                surface: "の".to_string(),
                reading: None,
            },
        ];
        assert_eq!(tokens_to_hiragana(&tokens), "はいざくらの");
    }

    #[test]
    fn ruby_basic() {
        let tokens = vec![
            ReadingToken {
                surface: "灰桜".to_string(),
                reading: Some("ハイザクラ".to_string()),
            },
            ReadingToken {
                surface: "の".to_string(),
                reading: None,
            },
        ];
        assert_eq!(tokens_to_ruby(&tokens), "{灰桜|はいざくら}の");
    }

    #[test]
    fn ruby_skips_when_kana_matches_surface() {
        // surface 「あ」, reading 「ア」 → ひらがな化で「あ」と一致 → ruby 不要
        let tokens = vec![ReadingToken {
            surface: "あ".to_string(),
            reading: Some("ア".to_string()),
        }];
        assert_eq!(tokens_to_ruby(&tokens), "あ");
    }

    /// アルファベット surface (loanword 等) は reading をカタカナのまま維持
    #[test]
    fn hiragana_keeps_katakana_for_alphabet_surface() {
        let tokens = vec![
            ReadingToken {
                surface: "Kubernetes".to_string(),
                reading: Some("クバネティス".to_string()),
            },
            ReadingToken {
                surface: "が安定".to_string(), // 漢字含むのでひらがな化
                reading: Some("ガアンテイ".to_string()),
            },
        ];
        // Kubernetes はカタカナ維持、 「が安定」 部分はひらがな化
        assert_eq!(tokens_to_hiragana(&tokens), "クバネティスがあんてい");
    }

    /// 漢字を含まない混在 (英字 + 記号) もカタカナ維持
    #[test]
    fn hiragana_keeps_katakana_for_symbol_surface() {
        let tokens = vec![ReadingToken {
            surface: "C++".to_string(),
            reading: Some("シープラスプラス".to_string()),
        }];
        assert_eq!(tokens_to_hiragana(&tokens), "シープラスプラス");
    }

    /// 漢字を含むなら従来通りひらがな化
    #[test]
    fn hiragana_lowers_when_surface_has_kanji() {
        let tokens = vec![ReadingToken {
            surface: "灰桜".to_string(),
            reading: Some("ハイザクラ".to_string()),
        }];
        assert_eq!(tokens_to_hiragana(&tokens), "はいざくら");
    }
}
