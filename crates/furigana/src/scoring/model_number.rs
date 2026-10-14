//! 型番パターンの照合と読みの生成 (`models.toml`、 [`crate::rules::ModelsData`])。
//!
//! `RTX 4070 Ti` / `Ryzen 7 5800X3D` / `i9-13900K` のような 「ブランド + 数字 + 接尾辞」 を
//! 1 つの候補として読む。 パターンは先頭の英字で索引して、 英字の語頭でだけ正規表現を試す。

use crate::rules::models::{ModelSpec, ModelsData};
use crate::scoring::digit_reading::DigitStyle;
use regex::Regex;
use std::collections::HashMap;

/// 雛形の部品。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Lit(String),
    Group { index: usize, style: Style },
}

/// 括弧で取った部分の読み方。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// 数字ならそのまま 1 桁ずつ、 それ以外は part
    Auto,
    Digits(DigitStyle),
    /// 5.0 = ゴテンゼロ
    Dotted,
    /// `[parts]` の読み、 無ければ spell
    Part,
    /// 英字 1 字ずつ + 数字は英語
    Spell,
}

impl Style {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "dotted" => Some(Self::Dotted),
            "part" => Some(Self::Part),
            "spell" => Some(Self::Spell),
            _ => DigitStyle::parse(s).map(Self::Digits),
        }
    }
}

/// 1 件分の用意済みパターン。
#[derive(Debug, Clone)]
pub struct CompiledModel {
    re: Regex,
    pieces: Vec<Piece>,
    first: Vec<char>,
}

impl CompiledModel {
    /// pattern と雛形を検査して用意する。
    ///
    /// # Errors
    /// 正規表現が不正 / 雛形が不正 / 絞り込み用の先頭英字が取れない時
    pub fn compile(spec: &ModelSpec) -> Result<Self, String> {
        let re = Regex::new(&format!("^(?i:{})", spec.pattern)).map_err(|e| e.to_string())?;
        let pieces = parse_template(&spec.reading, re.captures_len() - 1)?;
        let first = first_chars(spec)?;
        Ok(Self { re, pieces, first })
    }
}

/// 絞り込み用の先頭英字 (小文字)。 `first` 指定が優先、 無ければ pattern の頭の英字。
fn first_chars(spec: &ModelSpec) -> Result<Vec<char>, String> {
    let src = if let Some(f) = &spec.first {
        f.as_str()
    } else {
        match spec.pattern.chars().next() {
            Some(c) if c.is_ascii_alphabetic() => &spec.pattern[..1],
            _ => return Err("先頭が英字ではないので first = \"...\" が必要".to_string()),
        }
    };
    let chars: Vec<char> = src
        .chars()
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if chars.is_empty() {
        return Err("first に英字が無い".to_string());
    }
    Ok(chars)
}

fn parse_template(t: &str, groups: usize) -> Result<Vec<Piece>, String> {
    let mut pieces = Vec::new();
    let mut lit = String::new();
    let mut chars = t.chars();
    while let Some(c) = chars.next() {
        if c != '{' {
            lit.push(c);
            continue;
        }
        let mut inner = String::new();
        loop {
            match chars.next() {
                Some('}') => break,
                Some(x) => inner.push(x),
                None => return Err(format!("'{{' が閉じていない: {t}")),
            }
        }
        let (num, style) = match inner.split_once(':') {
            Some((n, s)) => (n, Some(s)),
            None => (inner.as_str(), None),
        };
        let index: usize = num
            .parse()
            .map_err(|_| format!("{{{inner}}} の番号が数字ではない"))?;
        if index == 0 || index > groups {
            return Err(format!("{{{inner}}} は括弧の数 ({groups}) を超える"));
        }
        let style = match style {
            None => Style::Auto,
            Some(s) => Style::parse(s).ok_or_else(|| format!("未知の style: {s}"))?,
        };
        if !lit.is_empty() {
            pieces.push(Piece::Lit(std::mem::take(&mut lit)));
        }
        pieces.push(Piece::Group { index, style });
    }
    if !lit.is_empty() {
        pieces.push(Piece::Lit(lit));
    }
    Ok(pieces)
}

/// 英字の読み (A〜Z)。
pub(crate) const LETTER_NAMES: [&str; 26] = [
    "エー",
    "ビー",
    "シー",
    "ディー",
    "イー",
    "エフ",
    "ジー",
    "エイチ",
    "アイ",
    "ジェイ",
    "ケー",
    "エル",
    "エム",
    "エヌ",
    "オー",
    "ピー",
    "キュー",
    "アール",
    "エス",
    "ティー",
    "ユー",
    "ブイ",
    "ダブリュー",
    "エックス",
    "ワイ",
    "ゼット",
];

/// 英字 1 字ずつ + 数字は英語 1 桁ずつ (X3D = エックススリーディー)。 他の字は読まない。
fn spell(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_alphabetic() {
            out.push_str(LETTER_NAMES[usize::from(c.to_ascii_uppercase() as u8 - b'A')]);
        } else if c.is_ascii_digit() {
            out.push_str(
                &DigitStyle::EachEn
                    .render(&c.to_string())
                    .unwrap_or_default(),
            );
        } else if c == '+' {
            out.push_str("プラス");
        }
    }
    out
}

/// 空白をひとつに、 小文字にして [`parts`](ModelsData::parts) の引き用に整える。
fn part_key(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

/// 用意済みの型番パターン一式。
#[derive(Debug, Clone, Default)]
pub struct CompiledModels {
    by_first: HashMap<char, Vec<CompiledModel>>,
    parts: HashMap<String, String>,
}

impl CompiledModels {
    /// [`ModelsData`] から用意する。 不正なパターンは飛ばす (load 時に [`ModelsData::validate`] で弾く)。
    #[must_use]
    pub fn new(data: &ModelsData) -> Self {
        let mut by_first: HashMap<char, Vec<CompiledModel>> = HashMap::new();
        for spec in &data.model {
            if let Ok(m) = CompiledModel::compile(spec) {
                for &c in &m.first {
                    by_first.entry(c).or_default().push(m.clone());
                }
            }
        }
        let parts = data
            .parts
            .iter()
            .map(|(k, v)| (part_key(k), v.clone()))
            .collect();
        Self { by_first, parts }
    }

    /// 型番パターンが 1 つも無いか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_first.is_empty()
    }

    /// `input[pos..]` から型番が始まるなら (終端 byte 位置, 読み)。
    ///
    /// 語頭 (直前が ASCII 英数字でない) で、 直後も ASCII 英数字でない時だけ。 複数当たれば最も長いもの。
    #[must_use]
    pub fn match_at(&self, input: &str, pos: usize) -> Option<(usize, String)> {
        // 語の途中 (直前が英数字) は索引を引く前に除く (英文では大半の位置がここで終わる)
        if input[..pos]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        {
            return None;
        }
        let rest = &input[pos..];
        let lead = rest.chars().next()?.to_ascii_lowercase();
        let candidates = self.by_first.get(&lead)?;
        let mut best: Option<(usize, String)> = None;
        for m in candidates {
            // 当たらない時 (ほとんど) は captures の確保を避けるため、 先に find で見る
            let Some(found) = m.re.find(rest) else {
                continue;
            };
            let end = found.end();
            if rest[end..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                continue;
            }
            if best.as_ref().is_some_and(|(e, _)| *e >= end) {
                continue;
            }
            let Some(caps) = m.re.captures(rest) else {
                continue;
            };
            best = Some((pos + end, self.render(m, &caps)));
        }
        best
    }

    fn render(&self, m: &CompiledModel, caps: &regex::Captures<'_>) -> String {
        let mut out = String::new();
        for p in &m.pieces {
            match p {
                Piece::Lit(s) => out.push_str(s),
                Piece::Group { index, style } => {
                    if let Some(g) = caps.get(*index) {
                        out.push_str(&self.read_group(g.as_str(), *style));
                    }
                }
            }
        }
        out
    }

    fn read_group(&self, text: &str, style: Style) -> String {
        let is_digits = !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit());
        match style {
            Style::Digits(d) => d.render(text).unwrap_or_else(|| spell(text)),
            Style::Dotted => text
                .split('.')
                .map(|p| DigitStyle::Each.render(p).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("テン"),
            Style::Spell => spell(text),
            Style::Part => self.part(text),
            Style::Auto if is_digits => DigitStyle::Each.render(text).unwrap_or_default(),
            Style::Auto => self.part(text),
        }
    }

    fn part(&self, text: &str) -> String {
        self.parts
            .get(&part_key(text))
            .cloned()
            .unwrap_or_else(|| spell(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models(toml_str: &str) -> CompiledModels {
        let d: ModelsData = toml::from_str(toml_str).unwrap();
        d.validate().unwrap();
        CompiledModels::new(&d)
    }

    fn sample() -> CompiledModels {
        models(
            r#"
            [parts]
            "Ti" = "ティーアイ"
            "Super" = "スーパー"
            "Pro Max" = "プロマックス"
            "X3D" = "エックススリーディー"

            [[model]]
            pattern = 'RTX\s?([0-9]{4})(?:\s?(Ti|Super))?'
            reading = "アールティーエックス{1:each}{2:part}"

            [[model]]
            pattern = 'Ryzen\s?([3579])\s([0-9]{4})([A-Z0-9]{0,3})'
            reading = "ライゼン{1:english}{2:each}{3:part}"

            [[model]]
            pattern = 'i([3579])-?([0-9]{4,5})([A-Z]{0,2})'
            reading = "アイ{1:english}{2:each}{3:part}"

            [[model]]
            pattern = 'iPhone\s?([0-9]{1,2})(?:\s?(Pro Max|Pro|Plus))?'
            reading = "アイフォン{1:place}{2:part}"

            [[model]]
            pattern = '([ZBX])([5-9][0-9]0)([EM]?)'
            first = "ZBX"
            reading = "{1:spell}{2}{3}"

            [[model]]
            pattern = 'PCIe\s?([0-9]\.[0-9])'
            reading = "ピーシーアイイー{1:dotted}"
            "#,
        )
    }

    fn read(m: &CompiledModels, s: &str) -> Option<(usize, String)> {
        m.match_at(s, 0)
    }

    #[test]
    fn reads_brand_digits_and_suffix() {
        let m = sample();
        let got = |s: &str| read(&m, s).map(|(_, r)| r);
        assert_eq!(
            got("RTX4090").as_deref(),
            Some("アールティーエックスヨンゼロキュウゼロ")
        );
        assert_eq!(
            got("RTX 4070 Ti").as_deref(),
            Some("アールティーエックスヨンゼロナナゼロティーアイ")
        );
        assert_eq!(
            got("rtx 4080 super").as_deref(),
            Some("アールティーエックスヨンゼロハチゼロスーパー")
        );
        assert_eq!(
            got("Ryzen 7 5800X3D").as_deref(),
            Some("ライゼンセブンゴハチゼロゼロエックススリーディー")
        );
        assert_eq!(
            got("i9-13900K").as_deref(),
            Some("アイナインイチサンキュウゼロゼロケー")
        );
        assert_eq!(got("i5 12400F").as_deref(), None); // 空白は許していない
        assert_eq!(
            got("i7-12700KF").as_deref(),
            Some("アイセブンイチニナナゼロゼロケーエフ")
        );
        assert_eq!(
            got("iPhone 15 Pro Max").as_deref(),
            Some("アイフォンジュウゴプロマックス")
        );
        assert_eq!(got("iPhone15").as_deref(), Some("アイフォンジュウゴ"));
        assert_eq!(got("Z790").as_deref(), Some("ゼットナナキュウゼロ"));
        assert_eq!(got("B650E").as_deref(), Some("ビーロクゴゼロイー"));
        assert_eq!(
            got("PCIe 5.0").as_deref(),
            Some("ピーシーアイイーゴテンゼロ")
        );
    }

    #[test]
    fn match_end_is_returned_in_input_bytes() {
        let m = sample();
        let s = "あRTX 4090です";
        let pos = "あ".len();
        let (end, _) = m.match_at(s, pos).unwrap();
        assert_eq!(&s[pos..end], "RTX 4090");
    }

    #[test]
    fn needs_word_boundaries_on_both_sides() {
        let m = sample();
        // 直前が英数字なら語の途中
        assert!(m.match_at("xRTX4090", 1).is_none());
        assert!(m.match_at("9RTX4090", 1).is_none());
        // 直後が英数字なら型番の途中 (長い型番は別のパターンの担当)
        assert!(read(&m, "RTX40901").is_none());
        assert!(read(&m, "RTX4090a").is_none());
        // 日本語は境界
        assert!(m.match_at("つよいRTX4090", "つよい".len()).is_some());
        assert!(read(&m, "RTX4090は強い").is_some());
    }

    #[test]
    fn longest_match_wins_and_misses_are_none() {
        let m = sample();
        let (end, _) = read(&m, "RTX4090 Tiを買う").unwrap();
        assert_eq!(end, "RTX4090 Ti".len());
        assert!(read(&m, "GTX1660").is_none());
        assert!(read(&m, "").is_none());
        assert!(m.match_at("あ", 0).is_none());
        // 先頭が違えば試さない
        assert!(read(&m, "Z79").is_none());
    }

    #[test]
    fn unknown_parts_are_spelled_and_digits_in_parts_are_english() {
        let m = sample();
        // Ti のように parts にあれば表の読み、 無ければ 1 字ずつ (大文字小文字は問わない)
        let (_, r) = read(&m, "Ryzen 9 7950X").unwrap();
        assert!(r.ends_with("エックス"), "{r}");
        let (_, r) = read(&m, "Ryzen 9 7950X3d").unwrap();
        assert!(r.ends_with("エックススリーディー"), "{r}");
        // 大小文字を区別しない
        let (_, r) = read(&m, "iphone 15 pro max").unwrap();
        assert!(r.ends_with("プロマックス"), "{r}");
    }

    #[test]
    fn longer_match_wins_regardless_of_pattern_order() {
        let short = r#"[[model]]
pattern = 'GT\s?([0-9]{3})'
reading = "ジーティー{1:each}"
"#;
        let long = r#"[[model]]
pattern = 'GT\s?([0-9]{3})\s?Ti'
reading = "ジーティー{1:each}ティーアイ"
"#;
        for order in [
            format!(
                "{short}
{long}"
            ),
            format!(
                "{long}
{short}"
            ),
        ] {
            let m = models(&order);
            let (end, r) = read(&m, "GT 710 Ti").unwrap();
            assert_eq!(end, "GT 710 Ti".len());
            assert!(r.ends_with("ティーアイ"), "{r}");
            // 短い方だけ当たる入力では短い方
            let (end, r) = read(&m, "GT 710").unwrap();
            assert_eq!(end, "GT 710".len());
            assert!(!r.ends_with("ティーアイ"), "{r}");
        }
        // 同じ長さなら先に書いた方
        let a = r#"[[model]]
pattern = 'GT([0-9]{3})'
reading = "ア{1:each}"
"#;
        let b = r#"[[model]]
pattern = 'GT([0-9]{3})'
reading = "イ{1:each}"
"#;
        let m = models(&format!(
            "{a}
{b}"
        ));
        assert!(read(&m, "GT710").unwrap().1.starts_with('ア'));
    }

    #[test]
    fn empty_models_match_nothing() {
        let m = CompiledModels::new(&ModelsData::default());
        assert!(m.is_empty());
        assert!(m.match_at("RTX4090", 0).is_none());
        assert!(!sample().is_empty());
    }

    #[test]
    fn spell_names_letters_and_english_digits() {
        assert_eq!(spell("X3D"), "エックススリーディー");
        assert_eq!(spell("abcz"), "エービーシーゼット");
        assert_eq!(spell("K+"), "ケープラス");
        assert_eq!(spell("-"), "");
        for (i, c) in ('A'..='Z').enumerate() {
            assert_eq!(spell(&c.to_string()), LETTER_NAMES[i]);
        }
    }

    #[test]
    fn template_rejects_non_numeric_or_zero_indexes() {
        assert!(parse_template("ア{x}", 1).is_err());
        assert!(parse_template("ア{0}", 1).is_err());
        assert!(parse_template("ア{1}{1:part}", 1).is_ok());
        assert_eq!(
            parse_template("ア{1}イ", 1).unwrap(),
            vec![
                Piece::Lit("ア".into()),
                Piece::Group {
                    index: 1,
                    style: Style::Auto
                },
                Piece::Lit("イ".into())
            ]
        );
    }

    #[test]
    fn dotted_reads_each_part_between_ten() {
        let m = models(
            r#"
            [[model]]
            pattern = 'HDMI\s?([0-9]\.[0-9])'
            reading = "エイチディーエムアイ{1:dotted}"
            "#,
        );
        assert_eq!(
            read(&m, "HDMI 2.1").unwrap().1,
            "エイチディーエムアイニテンイチ"
        );
    }

    #[test]
    fn first_chars_default_and_explicit() {
        let spec = |p: &str, f: Option<&str>| ModelSpec {
            pattern: p.to_string(),
            reading: "ア".to_string(),
            first: f.map(str::to_string),
        };
        assert_eq!(first_chars(&spec("RTX", None)).unwrap(), vec!['r']);
        assert_eq!(
            first_chars(&spec("([A-Z])", Some("ZB-"))).unwrap(),
            vec!['z', 'b']
        );
        // 先頭が英字でない時は、 first の指定が無いと 「先頭が英字ではない」 で弾く
        let err = first_chars(&spec("([A-Z])", None)).unwrap_err();
        assert!(err.contains("先頭が英字ではない"), "{err}");
        let err = first_chars(&spec("1RTX", None)).unwrap_err();
        assert!(err.contains("先頭が英字ではない"), "{err}");
        assert!(first_chars(&spec("([A-Z])", Some("12"))).is_err());
    }
}
