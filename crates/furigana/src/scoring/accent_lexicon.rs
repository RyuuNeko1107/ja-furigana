//! アクセント専用の表 (dict の `role = "accent"`)。
//!
//! 読みの辞書 entry に bracket notation が無い token に、 **表記 + 読みが一致する時だけ** accent を付ける。
//! 表は dict repo 側で UniDic の人手監修アクセント (aType) から offline 生成したもの
//! (ADR-0002 の 「accent は dict data 由来」 の範囲、 実行時は表を引くだけ = 決定的)。
//! 読み・区切りには一切触れない (読みが一致しなければ何もしない)。
//!
//! 適用順: dict bracket (真値) → この表 → rule-based 推定 ([`crate::scoring::accent_estimate`]、 opt-in)。
//!
//! 形式 (bracket notation は [`crate::scoring::bracket::parse_bracket_notation`] と同じ、 1 語 = 1 句):
//!
//! ```toml
//! [meta]
//! schema_version = "2"
//! role = "accent"
//!
//! [entries]
//! "天気" = "[テ]ンキ"
//! "今日" = ["[キョ]ウ", "[コ]ンニチ"]   # 読みごとに 1 つ
//! ```
//!
//! 表は 10 万件規模になるので、 メモリは 1 件 = 「表記 \u{1} 読み」 の key 1 本 + 核位置 / モーラ数だけ持ち、
//! sort 済み配列を二分探索で引く。 読み込みも serde で表全体を展開せず行単位で読む。

use crate::scoring::analyze::Token;
use crate::scoring::bracket::{parse_bracket_notation, AccentPhrase};
use std::path::Path;

/// 1 件 = (「表記 \u{1} 読み (カタカナ)」, 核位置 (0 = 平板), モーラ数)。 key 昇順
#[derive(Debug, Default)]
pub(crate) struct AccentLexicon {
    entries: Vec<(Box<str>, u8, u8)>,
}

const SEP: char = '\u{1}';

fn key(surface: &str, reading: &str) -> String {
    let mut k = String::with_capacity(surface.len() + 1 + reading.len() * 3 / 2);
    k.push_str(surface);
    k.push(SEP);
    k.push_str(&crate::kana::hira_to_kata(reading));
    k
}

/// `"..."` の中身を取り出す (`\"` `\\` だけ解く)。 戻り値 = (中身, 閉じ引用符の次の位置)
fn quoted(s: &str) -> Option<(String, usize)> {
    let mut out = String::new();
    let mut it = s.char_indices();
    if it.next()?.1 != '"' {
        return None;
    }
    let mut esc = false;
    for (i, c) in it {
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return Some((out, i + 1));
        } else {
            out.push(c);
        }
    }
    None
}

impl AccentLexicon {
    /// 1 件足す (1 句の bracket 付き読みだけ採る)
    fn push(&mut self, surface: &str, bracketed: &str) {
        let parsed = parse_bracket_notation(bracketed);
        let [phrase] = parsed.accent_phrases.as_slice() else {
            return;
        };
        let Some(accent) = phrase.accent else { return };
        self.entries.push((
            key(surface, &parsed.reading).into_boxed_str(),
            accent,
            phrase.mora,
        ));
    }

    fn finish(&mut self) {
        // 同じ key は先勝ち (stable sort + dedup で最初の 1 件を残す)
        self.entries.sort_by(|a, b| a.0.cmp(&b.0));
        self.entries.dedup_by(|b, a| a.0 == b.0);
        self.entries.shrink_to_fit();
    }

    /// `[entries]` の `"表記" = "読み"` / `"表記" = ["読み", ...]` 行を読む (行単位、 表全体を展開しない)
    fn load_str(&mut self, content: &str) {
        let mut in_entries = false;
        for line in content.lines() {
            let t = line.trim();
            if t.starts_with('[')
                && !t.starts_with("[[")
                && t.ends_with(']')
                && !t.starts_with("[\"")
            {
                in_entries = t == "[entries]";
                continue;
            }
            if !in_entries || !t.starts_with('"') {
                continue;
            }
            let Some((surface, end)) = quoted(t) else {
                continue;
            };
            let rest = t[end..].trim_start();
            let Some(rest) = rest.strip_prefix('=') else {
                continue;
            };
            let mut rest = rest.trim_start();
            if let Some(list) = rest.strip_prefix('[') {
                rest = list;
                while let Some((v, e)) = quoted(rest.trim_start()) {
                    self.push(&surface, &v);
                    let after = &rest.trim_start()[e..];
                    rest = after.trim_start().strip_prefix(',').unwrap_or(after);
                }
            } else if let Some((v, _)) = quoted(rest) {
                self.push(&surface, &v);
            }
        }
    }

    /// `dir` 配下の `role = "accent"` の TOML を取り込む
    pub(crate) fn load_dir(&mut self, dir: &Path, excludes: &[String]) -> crate::error::Result<()> {
        if !dir.exists() {
            return Ok(());
        }
        crate::loader::for_each_toml_in_dir_excluding(dir, excludes, |content, _from, role| {
            if role == Some("accent") {
                self.load_str(content);
            }
            Ok(())
        })?;
        self.finish();
        Ok(())
    }

    /// accent の無い token に、 表記 + 読みが一致する表の accent を付ける
    pub(crate) fn fill(&self, tokens: &mut [Token]) {
        if self.entries.is_empty() {
            return;
        }
        for t in tokens.iter_mut() {
            if !t.accent_phrases.is_empty() || t.reading.is_empty() {
                continue;
            }
            let k = key(&t.surface, &t.reading);
            if let Ok(i) = self.entries.binary_search_by(|e| (*e.0).cmp(k.as_str())) {
                let (_, accent, mora) = self.entries[i];
                t.accent_phrases = vec![AccentPhrase {
                    reading: crate::kana::hira_to_kata(&t.reading),
                    mora,
                    accent: Some(accent),
                    estimated: false,
                }];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(surface: &str, reading: &str) -> Token {
        Token {
            surface: surface.to_string(),
            reading: reading.to_string(),
            range: 0..surface.len(),
            accent_phrases: Vec::new(),
            ambiguous: false,
            alternatives: Vec::new(),
            is_name: false,
        }
    }

    fn lexicon(toml: &str) -> AccentLexicon {
        let mut lx = AccentLexicon::default();
        lx.load_str(toml);
        lx.finish();
        lx
    }

    const TABLE: &str = "[meta]\nschema_version = \"2\"\nrole = \"accent\"\n\n[entries]\n\"今日\" = [\"[キョ]ウ\", \"[コ]ンニチ\"]\n\"私\" = \"[ワタシ\"\n\"天気\" = \"[テンキ\"\n";

    #[test]
    fn fills_only_when_surface_and_reading_match() {
        let lx = lexicon(TABLE);
        let mut ts = vec![
            token("今日", "きょう"),
            token("今日", "こんにち"),
            token("今日", "けふ"),
            token("明日", "あした"),
        ];
        lx.fill(&mut ts);
        assert_eq!(ts[0].accent_phrases[0].accent, Some(1));
        assert_eq!(ts[0].accent_phrases[0].mora, 2);
        assert_eq!(ts[1].accent_phrases[0].accent, Some(1));
        assert!(ts[2].accent_phrases.is_empty(), "読みが違えば付けない");
        assert!(ts[3].accent_phrases.is_empty(), "表に無い語は付けない");
    }

    #[test]
    fn heiban_entry_and_katakana_reading() {
        let lx = lexicon(TABLE);
        let mut ts = vec![token("私", "わたし"), token("私", "ワタシ")];
        lx.fill(&mut ts);
        assert_eq!(ts[0].accent_phrases[0].accent, Some(0));
        assert_eq!(ts[1].accent_phrases[0].accent, Some(0));
    }

    #[test]
    fn does_not_override_existing_accent() {
        let lx = lexicon(TABLE);
        let mut ts = vec![token("天気", "てんき")];
        ts[0].accent_phrases = parse_bracket_notation("[テ]ンキ").accent_phrases;
        lx.fill(&mut ts);
        assert_eq!(
            ts[0].accent_phrases[0].accent,
            Some(1),
            "dict bracket (真値) を優先"
        );
    }

    #[test]
    fn ignores_other_sections_and_first_wins() {
        let lx = lexicon("[meta]\nrole = \"accent\"\n[entries]\n\"雨\" = \"[ア]メ\"\n\"雨\" = \"[アメ\"\n[other]\n\"雪\" = \"[ユ]キ\"\n");
        let mut ts = vec![token("雨", "あめ"), token("雪", "ゆき")];
        lx.fill(&mut ts);
        assert_eq!(ts[0].accent_phrases[0].accent, Some(1));
        assert!(ts[1].accent_phrases.is_empty());
    }
}
