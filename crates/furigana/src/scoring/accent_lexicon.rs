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

    fn get(&self, surface: &str, reading: &str) -> Option<(u8, u8)> {
        let k = key(surface, reading);
        self.entries
            .binary_search_by(|e| (*e.0).cmp(k.as_str()))
            .ok()
            .map(|i| (self.entries[i].1, self.entries[i].2))
    }

    /// accent の無い token に、 表記 + 読みが一致する表の accent を付ける。
    /// 動詞の活用途中 (食べ + た / 書か + ない) は、 辞書形の accent から活用規則で決める ([`Self::conjugated`])
    pub(crate) fn fill(&self, tokens: &mut [Token]) {
        if self.entries.is_empty() {
            return;
        }
        for i in 0..tokens.len() {
            if !tokens[i].accent_phrases.is_empty() || tokens[i].reading.is_empty() {
                continue;
            }
            let next = tokens.get(i + 1).map_or("", |n| n.surface.as_str());
            let reading = crate::kana::hira_to_kata(&tokens[i].reading);
            let (accent, mora, estimated) =
                if let Some((a, m)) = self.conjugated(&tokens[i].surface, &reading, next) {
                    (a, m, true)
                } else if let Some((a, m)) = self.get(&tokens[i].surface, &tokens[i].reading) {
                    (a, m, false)
                } else {
                    continue;
                };
            tokens[i].accent_phrases = vec![AccentPhrase {
                reading,
                mora,
                accent: Some(accent),
                estimated,
            }];
        }
    }

    /// 動詞の活用途中の accent (東京式の規則、 推定扱い)。 2026-09-27
    ///
    /// - 後ろが た / て / だ / で (過去・て形): 平板動詞は平板のまま。 起伏式は 一段 = 辞書形の核を 1 つ前へ
    ///   (タベ'ル → タ'ベタ / オキ'ル → オ'キテ)、 五段 = 核の位置そのまま (ハナ'ス → ハナ'シタ / カ'ク → カ'イタ)
    /// - 後ろが ない (否定): 平板動詞は平板のまま、 起伏式は ない の直前に核 (タベ'ナイ / カカ'ナイ)
    ///
    /// 辞書形は表から引く (一段 = 語幹 + ル、 五段 = 語尾の段を u 段に戻す)。 引けなければ `None`
    fn conjugated(&self, surface: &str, reading: &str, next: &str) -> Option<(u8, u8)> {
        let past = ["た", "て", "だ", "で"].iter().any(|p| next.starts_with(p));
        let neg = next.starts_with("な") && !next.starts_with("なら");
        if !(past || neg) {
            return None;
        }
        let last = surface.chars().next_back()?;
        let stem = surface.strip_suffix(last)?;
        if stem.is_empty() || !stem.chars().any(crate::kana::is_kanji_char) {
            // 見 / 寝 のような送り仮名の無い一段 (surface 全体が語幹)
            if !crate::kana::is_kanji_char(last) {
                return None;
            }
        }
        let mora = u8::try_from(count_mora(reading)).ok()?;
        let r_last = reading.chars().next_back()?;
        let r_stem = reading.strip_suffix(r_last)?;
        // (辞書形の表記, 読み, 一段か) の候補
        let mut cands: Vec<(String, String, bool)> = Vec::new();
        if crate::kana::is_kanji_char(last) || is_ie_row(r_last) {
            // 一段: 食べ / 起き / 見 + る
            cands.push((format!("{surface}る"), format!("{reading}ル"), true));
        }
        if !crate::kana::is_kanji_char(last) {
            let godan_endings: &[char] = match (past, r_last) {
                (true, 'ッ') => &['ル', 'ツ', 'ウ'],
                (true, 'イ') => &['ク', 'グ'],
                (true, 'ン') => &['ム', 'ブ', 'ヌ'],
                (true, 'シ') => &['ス'],
                (false, c) => a_to_u(c),
                _ => &[],
            };
            for &u in godan_endings {
                let hira_u = crate::kana::kata_to_hira(&u.to_string());
                cands.push((format!("{stem}{hira_u}"), format!("{r_stem}{u}"), false));
            }
        }
        let mut found: Option<(u8, bool)> = None;
        for (s, r, ichidan) in &cands {
            if let Some((a, _)) = self.get(s, r) {
                if found.is_some_and(|(fa, fi)| fa != a || fi != *ichidan) {
                    return None; // 候補が割れる (帰る / 返る 等) = 決めない
                }
                found = Some((a, *ichidan));
            }
        }
        let (base, ichidan) = found?;
        if base == 0 {
            return Some((0, mora));
        }
        let accent = if neg {
            mora
        } else if ichidan {
            base.saturating_sub(1).max(1)
        } else {
            base
        };
        (accent <= mora).then_some((accent, mora))
    }
}

/// モーラ数 (小書きの ャュョァィゥェォ は前に含める)
fn count_mora(kata: &str) -> usize {
    kata.chars()
        .filter(|c| !matches!(c, 'ャ' | 'ュ' | 'ョ' | 'ァ' | 'ィ' | 'ゥ' | 'ェ' | 'ォ'))
        .count()
}

/// イ段・エ段 (一段動詞の語幹末)
fn is_ie_row(c: char) -> bool {
    "イキギシジチヂニヒビピミリエケゲセゼテデネヘベペメレ".contains(c)
}

/// 五段の未然形 (ア段) → 終止形 (ウ段) の候補
fn a_to_u(c: char) -> &'static [char] {
    match c {
        'カ' => &['ク'],
        'ガ' => &['グ'],
        'サ' => &['ス'],
        'タ' => &['ツ'],
        'ナ' => &['ヌ'],
        'バ' => &['ブ'],
        'マ' => &['ム'],
        'ラ' => &['ル'],
        'ワ' => &['ウ'],
        _ => &[],
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
    fn conjugated_verb_accent_from_dictionary_form() {
        let lx = lexicon(
            "[meta]
role = \"accent\"
[entries]
\"食べる\" = \"[タベ]ル\"
\"話す\" = \"[ハナ]ス\"
\"書く\" = \"[カ]ク\"
\"遊ぶ\" = \"[アソブ\"
",
        );
        let mut ts = vec![
            token("食べ", "たべ"),
            token("た", "た"),
            token("話し", "はなし"),
            token("た", "た"),
            token("書か", "かか"),
            token("ない", "ない"),
            token("遊ん", "あそん"),
            token("だ", "だ"),
            token("食べ", "たべ"),
            token("ない", "ない"),
        ];
        lx.fill(&mut ts);
        let acc = |i: usize| ts[i].accent_phrases.first().and_then(|p| p.accent);
        assert_eq!(acc(0), Some(1), "一段 た形 = 核が 1 つ前 (タ'ベタ)");
        assert_eq!(acc(2), Some(2), "五段 た形 = 核そのまま (ハナ'シタ)");
        assert_eq!(acc(4), Some(2), "ない形 = ない の直前 (カカ'ナイ)");
        assert_eq!(acc(6), Some(0), "平板動詞は平板のまま");
        assert_eq!(acc(8), Some(2), "タベ'ナイ");
        assert!(ts[0].accent_phrases[0].estimated, "規則由来は推定扱い");
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
