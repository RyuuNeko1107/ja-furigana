//! 数式 (`x²-5x+b=0` / `y=1/x` / `√(a+b)` / `a₁+a₂`) の読み。
//!
//! 数式らしい範囲 (英字 1〜2 字の変数・数・演算子・括弧・上付き / 下付き) を 1 つの候補にまとめて読む。
//! 数式と見なすのは、 変数 (または数) に `=` / 不等号 / 累乗 / `√` が付く時だけ。 `HP=100` や
//! `val=3`、 `AB=CD`、 `a==b` のような英字の語・コードは数式にしない (従来どおり)。
//!
//! 読み方
//! - 数は数として (`2x` → ニx)、 英字はそのまま (読み上げ側が英字を読む)
//! - 演算子: `+` プラス / `-` `−` マイナス / `×` `*` カケル / `÷` ワル / `=` イコール / `≠` ノットイコール /
//!   `≦` `≧` `<` `>` ショウナリ… / `±` プラスマイナス
//! - 累乗: `x²` `x^2` `x^(n+1)` → 「xのニジョウ」
//! - 分数: `a/b` の両辺が 1 つの項 (数・変数・括弧) なら 「bブンノa」。 `4/3πr³` や `(b²-4ac)/2a` のように
//!   かけ算が続く書き方は読み方が割れるので、 書かれたとおり 「ワル」 で読む
//! - 関数 (`sin` `cos` `log` …) と ギリシャ文字は片仮名に。 `log₂8` → ログニノハチ
//!
//! 1 つの式を 1 候補 (band [`BAND_SPECIAL`]) にするので、 path は式を 1 edge で覆える。

use crate::numbers::helpers::sokuonize_last;
use crate::numbers::number_to_katakana;

/// 式として読む最大の長さ (文字数)。 これを超える英数字の連なりは式にしない
const MAX_SPAN: usize = 160;

/// 英字の関数名 (長い順。 前方一致で分ける)
const FUNCS: [(&str, &str); 12] = [
    ("arcsin", "アークサイン"),
    ("arccos", "アークコサイン"),
    ("arctan", "アークタンジェント"),
    ("sin", "サイン"),
    ("cos", "コサイン"),
    ("tan", "タンジェント"),
    ("log", "ログ"),
    ("exp", "エクスポネンシャル"),
    ("lim", "リミット"),
    ("max", "マックス"),
    ("min", "ミニマム"),
    ("ln", "エルエヌ"),
];

/// 数の直後に付くと単位 (kg / km / ms / m / t / W …) に読まれる英字。 変数 (2x) とは区別する
const UNIT_RUNS: [&str; 35] = [
    "kg", "km", "cm", "mm", "mg", "ml", "ms", "hz", "gb", "mb", "kb", "tb", "pt", "px", "pc", "kw",
    "mw", "db", "ft", "lb", "oz", "ha", "cc", "m", "g", "t", "s", "h", "l", "L", "W", "V", "K",
    "D", "G",
];

/// 変数の積に見えるが英単語の 2 字 (これらが `=` の隣にあっても式にしない)
const WORDS: [&str; 25] = [
    "id", "is", "to", "in", "on", "or", "if", "do", "no", "ok", "so", "as", "be", "by", "go", "he",
    "me", "my", "of", "up", "us", "we", "it", "an", "ww",
];

fn greek(c: char) -> Option<&'static str> {
    Some(match c {
        'α' => "アルファ",
        'β' => "ベータ",
        'γ' => "ガンマ",
        'δ' | 'Δ' => "デルタ",
        'ε' => "イプシロン",
        'θ' | 'Θ' => "シータ",
        'λ' | 'Λ' => "ラムダ",
        'μ' => "ミュー",
        'π' | 'Π' => "パイ",
        'ρ' => "ロー",
        'σ' | 'Σ' => "シグマ",
        'τ' => "タウ",
        'φ' | 'Φ' => "ファイ",
        'ω' | 'Ω' => "オメガ",
        _ => return None,
    })
}

/// 全角の ASCII (！〜～) を半角に、 数学のマイナス (− U+2212) を `-` に
fn fold(c: char) -> char {
    let c = match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{2212}' => '-',
        _ => c,
    };
    // [0,1] / E[X] の角括弧は丸括弧と同じに読む
    match c {
        '[' => '(',
        ']' => ')',
        _ => c,
    }
}

fn sup_digit(c: char) -> Option<char> {
    "⁰¹²³⁴⁵⁶⁷⁸⁹"
        .chars()
        .position(|k| k == c)
        .and_then(|i| char::from_digit(u32::try_from(i).ok()?, 10))
}

fn sub_digit(c: char) -> Option<char> {
    "₀₁₂₃₄₅₆₇₈₉"
        .chars()
        .position(|k| k == c)
        .and_then(|i| char::from_digit(u32::try_from(i).ok()?, 10))
}

fn is_op(c: char) -> bool {
    matches!(
        c,
        '+' | '-'
            | '*'
            | '/'
            | '='
            | '<'
            | '>'
            | '×'
            | '÷'
            | '±'
            | '≦'
            | '≧'
            | '≠'
            | '≒'
            | '≈'
            | '≤'
            | '≥'
            | '∈'
            | '∉'
            | '∪'
            | '∩'
            | '⊥'
            | '∥'
            | '≡'
            | '→'
            | ':'
            | '⊂'
            | '⊆'
            | '¬'
            | '∧'
            | '∨'
            | '⇔'
            | '⇒'
            | '∀'
            | '∃'
    )
}

/// 式の中に現れうる字 (空白は別に扱う)
fn is_span_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || is_op(c)
        || matches!(
            c,
            '(' | ')'
                | '^'
                | ','
                | '.'
                | '√'
                | '°'
                | '△'
                | '∠'
                | '∫'
                | '∞'
                | '\''
                | '′'
                | '…'
                | '⋯'
        )
        || sup_digit(c).is_some()
        || sub_digit(c).is_some()
        || c == '⁻'
        || greek(c).is_some()
}

/// 式を始められる字
/// 式の根拠になりうる字か (前判定用。 `is_math` が要求する 等号・不等号・関係記号・累乗・√・∫・絶対値・引き算のどれか)
fn is_trigger_char(c: char) -> bool {
    matches!(
        c,
        '=' | '<'
            | '>'
            | '≦'
            | '≧'
            | '≠'
            | '≒'
            | '≈'
            | '≤'
            | '≥'
            | '∈'
            | '∉'
            | '∪'
            | '∩'
            | '⊥'
            | '∥'
            | '≡'
            | '⊂'
            | '⊆'
            | '∧'
            | '∨'
            | '^'
            | '√'
            | '²'
            | '³'
            | '∫'
            | '|'
            | '-'
            | '\u{207B}'
            | '\u{2070}'
            | '\u{00B9}'
            | '\u{2074}'..='\u{2079}'
    )
}

fn is_start_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            '(' | '√' | '-' | '+' | '²' | '³' | '^' | '△' | '∠' | '∫' | '|' | '¬' | '∀' | '∃'
        )
        || greek(c).is_some()
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(String),
    Var(String),
    Func(&'static str),
    Greek(&'static str),
    /// Σ ∫ (範囲を取る演算子。 σ とは別)
    BigOp(&'static str),
    Op(char),
    Open,
    Close,
    Sup(String),
    Sub(String),
    Caret,
    Sqrt,
    Deg,
    Comma,
    /// f'(x) の ' (ダッシュ)
    Prime,
    /// 20% の %
    Pct,
    /// |x| の開き / 閉じ (絶対値)
    BarOpen,
    BarClose,
    /// n! の ! (階乗)
    Fact,
    /// a_n / a_{n+1} の _ (添え字)
    SubMark,
}

/// 範囲を字に切る。 英字の語・コードらしいもの (`HP` / `val` / `a==b` / `i++`) があれば None
fn tokenize(chars: &[char]) -> Option<Vec<Tok>> {
    // 幾何・集合の記号や √ ^ がある式では、 大文字 2〜3 字の名前 (AB / BMI / PV) を変数として許す
    let geo = chars.iter().any(|c| "⊥∥≡△∠∈∉∪∩⊂⊆".contains(*c))
        || (chars.contains(&'=') && chars.iter().any(|c| matches!(c, '√' | '^')));
    // `n!/(n-2)!=n(n-1)` のように、 != でない ! が別にある式では、 変数・閉じ括弧の後の `!=` も階乗 + 等号
    let other_bang = chars
        .iter()
        .enumerate()
        .any(|(k, c)| *c == '!' && chars.get(k + 1) != Some(&'='));
    let mut toks: Vec<Tok> = Vec::new();
    let mut bar_open = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' => i += 1,
            '0'..='9' => {
                // 変数の直後にくっついた数字 (mc2 / x2 / a1) は添え字つきの名前で、 べき乗でも掛け算でもない。 式にしない
                // (関数名の直後 sin2θ / cos2θ と、 順列・組合せ 5P3 / 5C3 は従来どおり)
                if i > 0
                    && chars[i - 1].is_ascii_alphabetic()
                    && !matches!(chars[i - 1], 'P' | 'C')
                    && matches!(toks.last(), Some(Tok::Var(_)))
                {
                    return None;
                }
                let mut s = String::new();
                let mut grouped = false;
                while i < chars.len() {
                    if chars[i].is_ascii_digit() {
                        s.push(chars[i]);
                        i += 1;
                    } else if chars[i] == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)
                    {
                        s.push('.');
                        i += 1;
                    } else if chars[i] == ','
                        && s.chars().all(|d| d.is_ascii_digit())
                        && chars
                            .get(i + 1..i + 4)
                            .is_some_and(|g| g.iter().all(char::is_ascii_digit))
                        && !chars.get(i + 4).is_some_and(char::is_ascii_digit)
                        && (s.len() <= 3 || grouped)
                    {
                        // 1,234 / 1,000,000 (位取りのカンマ)
                        grouped = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
                // 先頭が 0 の数 (cv03 / 000601 / 0x1F の 0) は番号・識別子・16 進数なので式にしない
                let hex = s == "0" && matches!(chars.get(i), Some('x' | 'X' | 'b' | 'B'));
                if (s.len() > 1 && s.starts_with('0') && !s.contains('.')) || hex {
                    return None;
                }
                toks.push(Tok::Num(s));
            }
            'a'..='z' | 'A'..='Z' => {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let run: String = chars[start..i].iter().collect();
                // 5P3 / 5C3 (数 P 数) は順列・組合せ
                if (run == "P" || run == "C")
                    && start > 0
                    && chars[start - 1].is_ascii_digit()
                    && chars.get(i).is_some_and(char::is_ascii_digit)
                {
                    toks.push(Tok::Var(run));
                    continue;
                }
                // 大文字の直後に数が付く形 (H2 / O2 / C60 / A4) は化学式・型番・判型なので式にしない
                if run.len() == 1
                    && run.chars().all(|c| c.is_ascii_uppercase())
                    && chars.get(i).is_some_and(char::is_ascii_digit)
                {
                    return None;
                }
                // 数に直接付く単位 (54.5kg / 100W / 3t) は式にしない (単位は従来の読み方に任せる)
                // (添え字の数 v_0t の 0 に付く t は単位ではない)
                if start > 0
                    && chars[start - 1].is_ascii_digit()
                    && !matches!(toks.last(), Some(Tok::Sub(_)))
                    && !(matches!(toks.last(), Some(Tok::Num(_)))
                        && toks.len() >= 2
                        && toks[toks.len() - 2] == Tok::Caret)
                    && UNIT_RUNS.contains(&run.as_str())
                {
                    return None;
                }
                // 組合せ・順列 nCr / nPr は 1 つの変数として残す (読み上げ側が英字を読む)
                let b = run.as_bytes();
                if b.len() == 3
                    && b[0].is_ascii_lowercase()
                    && matches!(b[1], b'C' | b'P')
                    && b[2].is_ascii_lowercase()
                {
                    toks.push(Tok::Var(run));
                    continue;
                }
                // 幾何・集合の記号 (⊥ ∥ ≡ △ ∠ ∈ ∪ ∩) がある式では、 大文字 2〜3 字の図形・集合の名前 (AB / ABC) を許す
                // (数の直後の 2GB / 8GB のような単位は除く)
                if geo
                    && !(start > 0 && chars[start - 1].is_ascii_digit())
                    && (2..=3).contains(&run.len())
                    && run.chars().all(|c| c.is_ascii_uppercase())
                {
                    toks.push(Tok::Var(run));
                    continue;
                }
                split_letters(&run, &mut toks)?;
            }
            '²' | '³' | '¹' | '⁰' | '⁴'..='⁹' | '⁻' => {
                let mut s = String::new();
                while i < chars.len() && (chars[i] == '⁻' || sup_digit(chars[i]).is_some()) {
                    s.push(if chars[i] == '⁻' {
                        '-'
                    } else {
                        sup_digit(chars[i])?
                    });
                    i += 1;
                }
                toks.push(Tok::Sup(s));
            }
            '₀'..='₉' => {
                let mut s = String::new();
                while i < chars.len() && sub_digit(chars[i]).is_some() {
                    s.push(sub_digit(chars[i])?);
                    i += 1;
                }
                toks.push(Tok::Sub(s));
            }
            '√' => {
                toks.push(Tok::Sqrt);
                i += 1;
            }
            '°' => {
                toks.push(Tok::Deg);
                i += 1;
            }
            '^' => {
                toks.push(Tok::Caret);
                i += 1;
            }
            '(' => {
                toks.push(Tok::Open);
                i += 1;
            }
            ')' => {
                toks.push(Tok::Close);
                i += 1;
            }
            ',' => {
                toks.push(Tok::Comma);
                i += 1;
            }
            // 省略記号 1+2+...+n / 1,2,…,n
            '.' if chars.get(i + 1) == Some(&'.') && chars.get(i + 2) == Some(&'.') => {
                toks.push(Tok::Greek("テンテンテン"));
                i += 3;
            }
            '…' | '⋯' => {
                toks.push(Tok::Greek("テンテンテン"));
                i += 1;
            }
            '.' => return None,
            '|' => {
                toks.push(if bar_open {
                    Tok::BarClose
                } else {
                    Tok::BarOpen
                });
                bar_open = !bar_open;
                i += 1;
            }
            // ^{…} / _{…} の波括弧は括弧として読む (それ以外の { } は式にしない)
            '{' => {
                if !matches!(i.checked_sub(1).map(|k| chars[k]), Some('^' | '_')) {
                    return None;
                }
                toks.push(Tok::Open);
                i += 1;
            }
            '}' => {
                toks.push(Tok::Close);
                i += 1;
            }
            // a_1 (数) / a_n (1 字) / a_{n+1} (波括弧)
            '_' => {
                i += 1;
                if chars.get(i).is_some_and(char::is_ascii_digit) {
                    let mut s = String::new();
                    while chars.get(i).is_some_and(char::is_ascii_digit) {
                        s.push(chars[i]);
                        i += 1;
                    }
                    toks.push(Tok::Sub(s));
                } else if chars.get(i) == Some(&'{') {
                    toks.push(Tok::SubMark);
                } else if chars.get(i).is_some_and(char::is_ascii_alphabetic)
                    && !chars.get(i + 1).is_some_and(char::is_ascii_alphabetic)
                {
                    toks.push(Tok::SubMark);
                    toks.push(Tok::Var(chars[i].to_string()));
                    i += 1;
                } else {
                    return None;
                }
            }
            // 20%=1/5: 数の直後の % (後ろに英数字が続く 50%OFF は除く)
            '%' => {
                if !matches!(toks.last(), Some(Tok::Num(_))) {
                    return None;
                }
                toks.push(Tok::Pct);
                i += 1;
            }
            // != は ≠、 n! / (n-1)! は階乗 (数の直後の ! は 5!/3! のように式が続く時だけ。 感嘆符を除く)
            '!' => {
                // `5!=120` (数の階乗 = 数) は階乗。 `n!=0` のような英字の後は コードの != (≠)
                let factorial_eq = chars.get(i + 1) == Some(&'=')
                    && ((matches!(toks.last(), Some(Tok::Num(_)))
                        && chars.get(i + 2).is_some_and(char::is_ascii_digit))
                        || (other_bang && matches!(toks.last(), Some(Tok::Var(_) | Tok::Close))));
                if chars.get(i + 1) == Some(&'=') && !factorial_eq {
                    toks.push(Tok::Op('≠'));
                    i += 2;
                } else if factorial_eq {
                    toks.push(Tok::Fact);
                    i += 1;
                } else {
                    let ok = match toks.last() {
                        Some(Tok::Var(_) | Tok::Close) => true,
                        Some(Tok::Num(_)) => {
                            chars.get(i + 1).is_some_and(|c| "/*×)+-".contains(*c))
                        }
                        _ => false,
                    };
                    if !ok {
                        return None;
                    }
                    toks.push(Tok::Fact);
                    i += 1;
                }
            }
            // f'(x) / f′(x): 変数 1 字の直後で ( か = が続く時だけ微分のダッシュ (英語の ' は式にしない)
            '\'' | '′' => {
                let prev_ok = matches!(toks.last(), Some(Tok::Var(v)) if v.len() == 1)
                    || matches!(toks.last(), Some(Tok::Close));
                if !prev_ok || chars.get(i + 1).is_some_and(|c| c.is_alphabetic()) {
                    return None;
                }
                toks.push(Tok::Prime);
                i += 1;
            }
            '△' | '∠' | '∞' => {
                toks.push(Tok::Greek(match c {
                    '△' => "サンカク",
                    '∠' => "カク",
                    _ => "ムゲンダイ",
                }));
                i += 1;
            }
            'Σ' | '∫' => {
                toks.push(Tok::BigOp(if c == 'Σ' {
                    "シグマ"
                } else {
                    "インテグラル"
                }));
                i += 1;
            }
            c if greek(c).is_some() => {
                toks.push(Tok::Greek(greek(c)?));
                i += 1;
            }
            c if is_op(c) => {
                // a:b=2:3 の比の `:`。 変数・数の間だけで、 10:30 のような時刻 (後ろが 2 桁の数) は式にしない
                if c == ':' {
                    let prev_ok =
                        matches!(toks.last(), Some(Tok::Num(_) | Tok::Var(_) | Tok::Close));
                    let next_ok = chars
                        .get(i + 1)
                        .is_some_and(|n| n.is_ascii_alphanumeric() || *n == '(');
                    let time = matches!(toks.last(), Some(Tok::Num(_)))
                        && chars
                            .get(i + 1..i + 3)
                            .is_some_and(|g| g.iter().all(char::is_ascii_digit))
                        && !chars.get(i + 3).is_some_and(char::is_ascii_digit);
                    if !prev_ok || !next_ok || time {
                        return None;
                    }
                }
                let next = chars.get(i + 1).copied();
                let op = match (c, next) {
                    ('=', Some('=' | '>')) | ('-', Some('>')) => return None, // == => ->
                    ('<', Some('=')) => {
                        i += 1;
                        '≦'
                    }
                    ('>', Some('=')) => {
                        i += 1;
                        '≧'
                    }
                    ('≤', _) => '≦',
                    ('≥', _) => '≧',
                    ('≈', _) => '≒',
                    ('*', _) => '×',
                    _ => c,
                };
                if c == '/' {
                    toks.push(Tok::Op('/'));
                } else {
                    toks.push(Tok::Op(op));
                }
                i += 1;
            }
            _ => return None,
        }
    }
    Some(toks)
}

/// 英字の連なりを 関数名 / 変数 に分ける。 変数は 1 字、 または小文字 2 字 (`ac` `dx`)
fn split_letters(run: &str, toks: &mut Vec<Tok>) -> Option<()> {
    let mut rest = run;
    while !rest.is_empty() {
        if let Some((name, kata)) = FUNCS.iter().find(|(n, _)| rest.starts_with(n)) {
            toks.push(Tok::Func(kata));
            rest = &rest[name.len()..];
            continue;
        }
        // abcosC のように、 1〜2 字の変数の後に関数名が続く連なり
        if let Some(k) = (1..=2).find(|&k| {
            rest.len() > k
                && rest[..k].chars().all(|c| c.is_ascii_lowercase())
                && !WORDS.contains(&&rest[..k])
                && FUNCS.iter().any(|(n, _)| rest[k..].starts_with(n))
        }) {
            toks.push(Tok::Var(rest[..k].to_string()));
            rest = &rest[k..];
            continue;
        }
        let n = rest.chars().count();
        // 小文字 2 字は積 (ac / dx / mc)。 英単語 (id / is / to …) は変数にしない
        // 単独の w は笑い (草w / w=2) なので変数にしない
        if (n == 1 && rest != "w")
            || (n == 2 && rest.chars().all(|c| c.is_ascii_lowercase()) && !WORDS.contains(&rest))
        {
            toks.push(Tok::Var(rest.to_string()));
            return Some(());
        }
        return None;
    }
    Some(())
}

/// 読みの途中の項目 (演算子と、 ひとまとまりの項)
#[derive(Debug, Clone)]
enum Item {
    /// 項 (読み、 関数名か)
    Atom {
        r: String,
        func: bool,
        /// 数だけの項 (累乗・下付きなし)
        num: bool,
    },
    Op(char),
    Slash,
    Comma,
}

struct Parser<'a> {
    toks: &'a [Tok],
    i: usize,
}

impl Parser<'_> {
    /// 項 1 つ (累乗・下付き・度を付けて)。 取れなければ None
    fn atom(&mut self) -> Option<Item> {
        let start = self.i;
        let (mut r, func) = match self.toks.get(self.i)?.clone() {
            Tok::Num(s) => {
                self.i += 1;
                (number_to_katakana(&s), false)
            }
            Tok::Var(s) => {
                self.i += 1;
                (s, false)
            }
            Tok::Greek(g) => {
                self.i += 1;
                (g.to_string(), false)
            }
            Tok::Func(f) => {
                self.i += 1;
                (f.to_string(), true)
            }
            Tok::BigOp(g) => {
                self.i += 1;
                (g.to_string(), true)
            }
            Tok::Sqrt => {
                self.i += 1;
                let Item::Atom { r, .. } = self.atom()? else {
                    return None;
                };
                (format!("ルート{r}"), false)
            }
            Tok::Open => (self.group()?, false),
            // |x-1| = 絶対値
            Tok::BarOpen => {
                self.i += 1;
                let inner = self.seq(true)?;
                if self.toks.get(self.i) != Some(&Tok::BarClose) {
                    return None;
                }
                self.i += 1;
                (format!("ゼッタイチ({inner})"), false)
            }
            // ² や ^2 だけが先頭に来た (x が切れている) 場合は 「ニジョウ」 から読む
            Tok::Sup(_) | Tok::Caret => (String::new(), false),
            _ => return None,
        };
        // 後置: 下付き / 上付き / ^ / 度
        loop {
            match self.toks.get(self.i) {
                Some(Tok::Sub(s)) => {
                    let n = number_to_katakana(s);
                    // log₂8 → ログニノハチ。 変数の下付き (a₁) はそのまま続ける
                    if func && is_big_operator(&r) {
                        // ∫₀¹ / Σ₁ⁿ の範囲: インテグラルゼロ (カライチマデ)
                        r = format!("{r}{n}");
                    } else if func {
                        r = format!("{r}{n}ノ");
                    } else {
                        r.push_str(&n);
                    }
                    self.i += 1;
                }
                Some(Tok::Sup(s)) => {
                    let neg = s.starts_with('-');
                    let digits = s.trim_start_matches('-');
                    let n = if digits.is_empty() {
                        String::new()
                    } else {
                        number_to_katakana(digits)
                    };
                    let sign = if neg { "マイナス" } else { "" };
                    // g⁻¹(x) は逆関数: 1 字の変数の ⁻¹ の直後が ( の時だけ 「インバース」
                    let inverse_fn = s == "-1"
                        && r.len() == 1
                        && r.chars().all(|c| c.is_ascii_alphabetic())
                        && matches!(self.toks.get(self.i + 1), Some(Tok::Open));
                    if inverse_fn {
                        r.push_str("インバース");
                    } else if r.is_empty() {
                        r = format!("{sign}{n}ジョウ");
                    } else {
                        r = format!("{r}ノ{sign}{n}ジョウ");
                    }
                    self.i += 1;
                }
                Some(Tok::Caret) => {
                    self.i += 1;
                    let neg = self.toks.get(self.i) == Some(&Tok::Op('-'));
                    if neg {
                        self.i += 1;
                    }
                    let Item::Atom { r: e, .. } = self.atom_no_pow()? else {
                        return None;
                    };
                    let sign = if neg { "マイナス" } else { "" };
                    // g^-1(x) (= g⁻¹(x)) は逆関数: 1 字の変数の ^-1 の直後が ( の時だけ 「インバース」
                    let inverse_fn = neg
                        && e == "イチ"
                        && r.len() == 1
                        && r.chars().all(|c| c.is_ascii_alphabetic())
                        && matches!(self.toks.get(self.i), Some(Tok::Open));
                    r = if inverse_fn {
                        format!("{r}インバース")
                    } else if func && is_big_operator(&r) {
                        // Σ_{k=1}^{n} → シグマ(kイコールイチ)カラ(n)マデ
                        format!("{r}カラ{sign}{e}マデ")
                    } else if r.is_empty() {
                        format!("{sign}{e}ジョウ")
                    } else {
                        format!("{r}ノ{sign}{e}ジョウ")
                    };
                }
                Some(Tok::Deg) => {
                    r.push('ド');
                    self.i += 1;
                }
                Some(Tok::Prime) => {
                    r.push_str("ダッシュ");
                    self.i += 1;
                }
                // 20% → ニジュッパーセント (6% → ロッパーセント、 63% → ロクジュウサンパーセント)
                Some(Tok::Pct) => {
                    r = format!("{}パーセント", sokuonize_last(&r));
                    self.i += 1;
                }
                Some(Tok::Fact) => {
                    r.push_str("ノカイジョウ");
                    self.i += 1;
                }
                // a_n → aサブn / a_{n+1} → aサブ(nプラスイチ)。 Σ ∫ lim の下付きは範囲 (Σ_{k=1}^{n})
                Some(Tok::SubMark) => {
                    self.i += 1;
                    let Item::Atom { r: g, .. } = self.atom_no_pow()? else {
                        return None;
                    };
                    r = if func && is_big_operator(&r) {
                        format!("{r}{g}")
                    } else {
                        format!("{r}サブ{g}")
                    };
                }
                _ => break,
            }
        }
        let num = matches!(self.toks.get(start), Some(Tok::Num(_))) && self.i == start + 1;
        Some(Item::Atom { r, func, num })
    }

    /// 括弧 `( … )` を読む (後置の累乗・下付きは付けない)。 現在位置は `(`
    /// (Σ ∫ lim は範囲を取る: [`is_big_operator`])
    fn group(&mut self) -> Option<String> {
        self.i += 1;
        let inner = self.seq(true)?;
        if self.toks.get(self.i) != Some(&Tok::Close) {
            return None;
        }
        self.i += 1;
        Some(format!("({inner})"))
    }

    /// 指数の項 (累乗の連鎖 x^y^z は読まない)
    fn atom_no_pow(&mut self) -> Option<Item> {
        match self.toks.get(self.i)?.clone() {
            Tok::Num(s) => {
                self.i += 1;
                Some(Item::Atom {
                    r: number_to_katakana(&s),
                    func: false,
                    num: true,
                })
            }
            Tok::Var(s) => {
                self.i += 1;
                Some(Item::Atom {
                    r: s,
                    func: false,
                    num: false,
                })
            }
            Tok::Open => Some(Item::Atom {
                r: self.group()?,
                func: false,
                num: false,
            }),
            _ => None,
        }
    }

    /// 項と演算子の並び。 `in_group` なら `)` の手前で止める
    fn seq(&mut self, in_group: bool) -> Option<String> {
        let mut items: Vec<Item> = Vec::new();
        while let Some(t) = self.toks.get(self.i) {
            match t {
                Tok::Close | Tok::BarClose if in_group => break,
                Tok::Close | Tok::BarClose => return None,
                Tok::Comma => {
                    items.push(Item::Comma);
                    self.i += 1;
                }
                Tok::Op('/') => {
                    items.push(Item::Slash);
                    self.i += 1;
                }
                Tok::Op(c) => {
                    items.push(Item::Op(*c));
                    self.i += 1;
                }
                _ => items.push(self.atom()?),
            }
        }
        Some(render(&items))
    }
}

/// 範囲を取る演算子の読みか (Σ ∫ lim)
fn is_big_operator(r: &str) -> bool {
    r.starts_with("シグマ") || r.starts_with("インテグラル") || r.starts_with("リミット")
}

/// 演算子の読み
fn op_reading(c: char) -> &'static str {
    match c {
        '⊂' | '⊆' => "ブブンシュウゴウ",
        '¬' => "ノット",
        '∧' => "カツ",
        '∨' => "マタハ",
        '⇔' => "ドウチ",
        '⇒' => "ナラバ",
        '∀' => "スベテノ",
        '∃' => "ソンザイスル",
        '→' => "ヘ",
        ':' => "タイ",
        '+' => "プラス",
        '-' => "マイナス",
        '×' => "カケル",
        '÷' => "ワル",
        '=' => "イコール",
        '≠' => "ノットイコール",
        '≒' => "ニアリーイコール",
        '≦' => "ショウナリイコール",
        '≧' => "ダイナリイコール",
        '<' => "ショウナリ",
        '>' => "ダイナリ",
        '±' => "プラスマイナス",
        '∈' => "ゾクスル",
        '∉' => "ゾクサナイ",
        '∪' => "カップ",
        '∩' => "キャップ",
        '⊥' => "スイチョク",
        '∥' => "ヘイコウ",
        '≡' => "ゴウドウ",
        _ => "",
    }
}

/// 項と演算子を読みにする。 `/` は両辺が 1 まとまりの項なら 「分母ブンノ分子」、 そうでなければ 「ワル」
fn render(items: &[Item]) -> String {
    // まず連続する項 (暗黙の掛け算) を run にまとめる
    let n = items.len();
    let mut used = vec![false; n];
    let mut out = String::new();
    let mut k = 0;
    while k < n {
        if used[k] {
            k += 1;
            continue;
        }
        match &items[k] {
            Item::Atom { .. } => {
                // k から始まる項の run
                let mut e = k;
                while e < n && matches!(items[e], Item::Atom { .. }) {
                    e += 1;
                }
                // x→0 は 「x ゼロ へ」 の順に読む (「リミット x ゼロへ」)
                if e < n
                    && matches!(items[e], Item::Op('→'))
                    && e + 1 < n
                    && matches!(items[e + 1], Item::Atom { .. })
                {
                    let mut r_end = e + 1;
                    while r_end < n && matches!(items[r_end], Item::Atom { .. }) {
                        r_end += 1;
                    }
                    for item in items[k..e].iter().chain(&items[e + 1..r_end]) {
                        if let Item::Atom { r, .. } = item {
                            out.push_str(r);
                        }
                    }
                    out.push('ヘ');
                    for u in used.iter_mut().take(r_end).skip(k) {
                        *u = true;
                    }
                    k = r_end;
                    continue;
                }
                // run の直後が `/` で、 その次にも run があれば分数を試す
                if e < n
                    && matches!(items[e], Item::Slash)
                    && e + 1 < n
                    && matches!(items[e + 1], Item::Atom { .. })
                {
                    let mut d_end = e + 1;
                    while d_end < n && matches!(items[d_end], Item::Atom { .. }) {
                        d_end += 1;
                    }
                    let chained = d_end < n && matches!(items[d_end], Item::Slash);
                    let single = |run: &[Item]| matches!(run, [Item::Atom { func: false, .. }]);
                    let join = |run: &[Item]| {
                        run.iter()
                            .map(|i| match i {
                                Item::Atom { r, .. } => r.as_str(),
                                _ => "",
                            })
                            .collect::<String>()
                    };
                    // lim / ∫ は式全体にかかるので (Σx/n は (Σx)/n なので外に出さず ワル)、 分数の外に出す (lim 1/x → リミット xブンノイチ)
                    let lead = items[k..e]
                        .iter()
                        .take_while(|i| {
                            matches!(i, Item::Atom { r, func: true, .. }
                                    if is_big_operator(r) && !r.starts_with("シグマ"))
                        })
                        .count();
                    // 数 / 数 + 項 (4/3πr³ / 1/2at²) は 先頭の数だけを分母にする: (4/3)πr³
                    let left_num = matches!(&items[k + lead..e], [Item::Atom { num: true, .. }]);
                    if !chained
                        && left_num
                        && d_end - (e + 1) >= 2
                        && matches!(items[e + 1], Item::Atom { num: true, .. })
                    {
                        out.push_str(&join(&items[k..k + lead]));
                        out.push_str(&format!(
                            "{}ブンノ{}",
                            join(&items[e + 1..e + 2]),
                            join(&items[k + lead..e])
                        ));
                        out.push_str(&join(&items[e + 2..d_end]));
                        for u in used.iter_mut().take(d_end).skip(k) {
                            *u = true;
                        }
                        k = d_end;
                        continue;
                    }
                    if !chained && single(&items[k + lead..e]) && single(&items[e + 1..d_end]) {
                        out.push_str(&join(&items[k..k + lead]));
                        out.push_str(&format!(
                            "{}ブンノ{}",
                            join(&items[e + 1..d_end]),
                            join(&items[k + lead..e])
                        ));
                        for u in used.iter_mut().take(d_end).skip(k) {
                            *u = true;
                        }
                        k = d_end;
                        continue;
                    }
                }
                for item in &items[k..e] {
                    if let Item::Atom { r, .. } = item {
                        out.push_str(r);
                    }
                }
                k = e;
            }
            Item::Slash => {
                out.push_str("ワル");
                k += 1;
            }
            Item::Op(c) => {
                out.push_str(op_reading(*c));
                k += 1;
            }
            Item::Comma => {
                out.push(',');
                k += 1;
            }
        }
    }
    out
}

/// 数式として読む価値があるか (変数・数に `=` / 不等号 / 累乗 / `√` が付く)
fn is_math(toks: &[Tok]) -> bool {
    let has_var = toks.iter().any(|t| {
        matches!(
            t,
            Tok::Var(_) | Tok::Func(_) | Tok::Greek(_) | Tok::BigOp(_)
        )
    });
    let has_num = toks.iter().any(|t| matches!(t, Tok::Num(_)));
    // ギリシャ文字だけ (顔文字 ΦωΦ / φ=φ) は式にしない: 英字の変数か数が要る
    if !has_num && !toks.iter().any(|t| matches!(t, Tok::Var(_) | Tok::Func(_))) {
        return false;
    }
    let eq = toks.contains(&Tok::Op('='));
    let strong_rel = toks.iter().any(|t| {
        matches!(
            t,
            Tok::Op(
                '≦' | '≧'
                    | '≠'
                    | '≒'
                    | '∈'
                    | '∉'
                    | '∪'
                    | '∩'
                    | '⊥'
                    | '∥'
                    | '≡'
                    | '⊂'
                    | '⊆'
                    | '∧'
                    | '∨'
            )
        )
    });
    // < > は、 `x>0 , y<0` や `-3<x<5` のように変数 1 字に接する不等号が 2 つ以上ある時だけ不等号にする
    // (`i<10` / `I<3` / 矢印・顔文字を除く)
    let single = |t: &Tok| matches!(t, Tok::Var(v) if v.len() == 1);
    let operand = |t: &Tok| matches!(t, Tok::Num(_) | Tok::Var(_));
    let weak_count = toks
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            matches!(t, Tok::Op('<' | '>'))
                && *i > 0
                && i + 1 < toks.len()
                && ((single(&toks[i - 1]) && operand(&toks[i + 1]))
                    || (single(&toks[i + 1]) && operand(&toks[i - 1])))
        })
        .count();
    let pow = toks.iter().enumerate().any(|(i, t)| match t {
        Tok::Sup(_) => true,
        Tok::Caret => {
            i > 0
                && matches!(
                    toks[i - 1],
                    Tok::Num(_) | Tok::Var(_) | Tok::Greek(_) | Tok::Func(_) | Tok::Close
                )
                && matches!(
                    toks.get(i + 1),
                    Some(Tok::Num(_) | Tok::Op('-') | Tok::Open)
                )
        }
        _ => false,
    });
    let sqrt = toks.contains(&Tok::Sqrt);
    let arith = toks
        .iter()
        .any(|t| matches!(t, Tok::Op('+' | '-' | '×' | '÷' | '/')));
    // 不等号が 1 つでも、 +-×÷ を含む式 (2x-3>5) なら不等式
    let lt_gt = toks.iter().any(|t| matches!(t, Tok::Op('<' | '>')));
    let weak_rel = weak_count >= 2 || (lt_gt && toks.iter().any(single) && arith);
    // → は極限 (lim(x→0)) のときだけ数式。 A→B / q→r / 手順の矢印は式にしない (読み上げは従来どおり)
    let arrow_ok =
        !toks.contains(&Tok::Op('→')) || toks.iter().any(|t| matches!(t, Tok::Func("リミット")));
    // ⇔ ⇒ は単独では式にしない (A⇔B = 交換 / L⇔R = バンド名)。 = などと一緒の式の中でだけ読む
    // 構文がおかしい (記号どうしの並び) ものは式にしない
    let ops_ok = toks.windows(2).all(|w| match (&w[0], &w[1]) {
        (Tok::Op(a), Tok::Op(b)) => {
            matches!(b, '-' | '+' | '±' | '¬' | '∀' | '∃') && !matches!(a, '-' | '+')
        }
        (Tok::Caret, Tok::Caret) => false,
        _ => true,
    });
    // ^ は、 底 (数・変数・閉じ括弧) と指数 (数・変数・括弧・負号) の間だけ。 先頭の ^ (底が切れた ²) は数が続く時だけ。
    // 顔文字 (^v^) (^^) を式にしない
    let caret_ok = toks.iter().enumerate().all(|(i, t)| {
        if *t != Tok::Caret {
            return true;
        }
        let next = toks.get(i + 1);
        if i == 0 {
            matches!(next, Some(Tok::Num(_)))
        } else {
            matches!(
                toks[i - 1],
                Tok::Num(_) | Tok::Var(_) | Tok::Greek(_) | Tok::Func(_) | Tok::Close | Tok::Sub(_)
            ) && matches!(
                next,
                Some(Tok::Num(_) | Tok::Var(_) | Tok::Op('-') | Tok::Open)
            )
        }
    });
    // 顔文字 (=-ω-) (≧v≦) の目・口: 括弧の直後の等号・不等号、 括弧の直前の等号・不等号
    let kaomoji = toks.windows(2).any(|w| {
        let rel = |t: &Tok| matches!(t, Tok::Op('=' | '≦' | '≧' | '<' | '>' | '≠' | '≒'));
        (w[0] == Tok::Open && rel(&w[1])) || (rel(&w[0]) && w[1] == Tok::Close)
    });
    // <b> のような HTML のタグ (< 変数1字 >) は不等号にしない
    let html_tag = toks
        .windows(3)
        .any(|w| matches!(w, [Tok::Op('<'), Tok::Var(v), Tok::Op('>')] if v.len() == 1));
    // 数をハイフンで 3 つ以上つないだ並び (0-1-0 = CSS の詳細度 / 100-200-300 = 番号) は引き算の式にしない
    // `R:N=1:3-4` のように : (比) がある式の 3-4 は範囲 (引き算ではない)
    let colon_range = toks.contains(&Tok::Op(':'))
        && toks
            .windows(3)
            .any(|w| matches!(w, [Tok::Num(_), Tok::Op('-'), Tok::Num(_)]));
    let hyphen_chain = toks.windows(5).any(|w| {
        matches!(
            w,
            [
                Tok::Num(_),
                Tok::Op('-'),
                Tok::Num(_),
                Tok::Op('-'),
                Tok::Num(_)
            ]
        )
    });
    // 先頭が演算子だけ・末尾が演算子は取り除いてあるが念のため
    let structure_ok = !matches!(toks.first(), Some(Tok::Op('=' | '/' | '×' | '÷')))
        && arrow_ok
        && ops_ok
        && caret_ok
        && !kaomoji
        && !html_tag
        && !colon_range
        && !hyphen_chain;
    if !structure_ok {
        return false;
    }
    // |x-1|<2 (絶対値と不等号)
    let abs_ineq =
        toks.contains(&Tok::BarOpen) && toks.iter().any(|t| matches!(t, Tok::Op('<' | '>')));
    // n-1 / x-y (添え字の変数 1 字 - 数・変数)。 ハイフンが読まれず nいち になるのを防ぐ
    let idx_minus = matches!(
        toks,
        [Tok::Var(v), Tok::Op('-'), Tok::Num(_) | Tok::Var(_)] if v.len() == 1 && "nmkijxyz".contains(v.as_str())
    );
    // ∫x dx / Σk のように、 積分・総和に変数が続く
    let bigop_var = toks.iter().any(|t| matches!(t, Tok::BigOp("インテグラル")))
        && toks.iter().any(|t| matches!(t, Tok::Var(_)));
    // p<0.05 (検定の有意水準): 変数 1 字と小数の間の不等号
    let p_value = matches!(
        toks,
        [Tok::Var(v), Tok::Op('<' | '>'), Tok::Num(n)] if v.len() == 1 && n.contains('.')
    );
    (has_var
        && (eq || strong_rel || weak_rel || p_value || pow || abs_ineq || idx_minus || bigop_var))
        || sqrt
        || (!has_var
            && has_num
            && (eq || strong_rel)
            && (arith || pow || toks.contains(&Tok::Fact)))
}

/// 末尾が 「+...」 「,…」 のような、 演算子・カンマの後の省略記号か
fn ends_with_ellipsis_after_separator(chars: &[char]) -> bool {
    let n = chars.len();
    let sep = |c: char| is_op(c) || c == ',';
    if n >= 2 && matches!(chars[n - 1], '…' | '⋯') {
        return sep(chars[n - 2]);
    }
    n >= 4 && chars[n - 3..] == ['.', '.', '.'] && sep(chars[n - 4])
}

/// `pos` から始まる数式を読む。 数式でなければ None。 戻りは (終端の byte 位置、 片仮名の読み)
pub(super) fn read_math(input: &str, pos: usize) -> Option<(usize, String)> {
    let rest = &input[pos..];
    let first = fold(rest.chars().next()?);
    if !is_start_char(first) {
        return None;
    }
    // 式の途中・語の途中・URL / クエリ (`?p=1` `&x=3` `/a=b`) の途中からは始めない
    if let Some(p) = input[..pos].chars().next_back() {
        let p = fold(p);
        if matches!(
            p,
            '_' | '\'' | '\u{2019}' | '?' | '&' | '#' | '@' | '%' | '~' | '\\'
        ) {
            return None;
        }
        // 直前が式の字: 直前が等号・不等号で、 その連なりが演算子だけなら (`利益=売上-原価=1000-700` の
        // 1000 の前の `=`) 日本語に付いた記号なので、 ここから始める。 それ以外 (英数字・括弧がある連なり、
        // `--v=0` のような `-`) は式の途中・顔文字・コマンドの一部なので始めない
        if is_span_char(p) && p != ' ' {
            let dangling_relation = matches!(p, '=' | '≒' | '≠' | '≦' | '≧')
                && input[..pos]
                    .chars()
                    .rev()
                    .map(fold)
                    .take_while(|c| is_span_char(*c))
                    .all(is_op);
            if !dangling_relation {
                return None;
            }
        }
    }
    // 前判定: 式になるには、 範囲の中に 「式の根拠になる記号」 (= 不等号 ^ √ ² ∫ | - …) が要る。 無ければ範囲を取らずに戻る
    // (数式の無い大半の行で、 英数字の位置ごとに残りの全文字を集め直すのを避ける)
    let mut has_trigger = false;
    for c in rest.chars().take(MAX_SPAN) {
        let f = fold(c);
        if is_trigger_char(f) {
            has_trigger = true;
            break;
        }
        // 状況によって式に含める字 (! % | { } _) と空白も通す
        if !(is_span_char(f) || matches!(f, ' ' | '!' | '%' | '|' | '{' | '}' | '_')) {
            break;
        }
    }
    if !has_trigger {
        return None;
    }
    // 範囲を取る (折りたたんだ字と、 各字の終端 byte)
    let mut chars: Vec<char> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    // 状態を持つ字 (| { } _ !) は、 式の形になる時だけ受ける
    let mut bar_open_at: Option<usize> = None;
    let mut brace_opens: Vec<usize> = Vec::new();
    let rest_chars: Vec<(usize, char)> = rest.char_indices().take(MAX_SPAN + 4).collect();
    for (n, &(i, c)) in rest_chars.iter().enumerate() {
        let f = fold(c);
        let next = rest_chars.get(n + 1).map(|&(_, c)| fold(c));
        let prev = chars
            .last()
            .copied()
            .or_else(|| input[..pos].chars().next_back().map(fold));
        let operand_end = |p: Option<char>| {
            p.is_some_and(|p| p.is_ascii_alphanumeric() || matches!(p, ')' | '°'))
        };
        let accepted = match f {
            // |x-1| (絶対値): 開きは前が英数字でなく後ろが式の頭、 閉じは前が英数字・閉じ括弧
            '|' if bar_open_at.is_none() => {
                !operand_end(prev)
                    && next
                        .is_some_and(|x| x.is_ascii_alphanumeric() || matches!(x, '(' | '-' | '√'))
            }
            '|' => operand_end(prev),
            // ^{…} と _{…} の波括弧だけ
            '{' => matches!(chars.last(), Some('^' | '_')),
            '}' => !brace_opens.is_empty(),
            // a_1 / a_n / a_{n}
            '_' => {
                (operand_end(chars.last().copied())
                    || chars
                        .last()
                        .is_some_and(|p| greek(*p).is_some() || *p == '∫'))
                    && next.is_some_and(|x| x.is_ascii_alphanumeric() || x == '{')
            }
            // n! / (n-1)! / x!=y
            '!' => operand_end(chars.last().copied()),
            // 20%=1/5 の % (数の直後で、 後ろに英数字が続かない時。 50%OFF は除く)
            '%' => {
                // (後ろが ` や " などの記号だと、 形態素解析の記号の塊の途中で式が終わるので % を含めない)
                chars.last().is_some_and(char::is_ascii_digit)
                    && next.is_none_or(|x| {
                        x == ' '
                            || matches!(
                                x,
                                ')' | ',' | '=' | '+' | '-' | '*' | '/' | '×' | '÷' | '<' | '>'
                            )
                            || ('\u{3000}'..='\u{9FFF}').contains(&x)
                    })
            }
            _ => is_span_char(f) || f == ' ',
        };
        if !accepted || chars.len() >= MAX_SPAN {
            break;
        }
        match f {
            '|' => {
                bar_open_at = if bar_open_at.is_none() {
                    Some(chars.len())
                } else {
                    None
                }
            }
            '{' => brace_opens.push(chars.len()),
            '}' => {
                brace_opens.pop();
            }
            _ => {}
        }
        chars.push(f);
        ends.push(pos + i + c.len_utf8());
    }
    // 閉じていない | { は、 その手前まで
    if let Some(&b) = brace_opens.first() {
        let cut = b.saturating_sub(1);
        chars.truncate(cut);
        ends.truncate(cut);
    }
    if let Some(b) = bar_open_at {
        if b < chars.len() {
            chars.truncate(b);
            ends.truncate(b);
        }
    }
    // 空白は演算子・カンマ・括弧の隣だけ許す (英単語の並びを式にしない)
    let is_glue = |c: char| is_op(c) || matches!(c, ',' | '(' | ')' | '}');
    let mut k = 0;
    while k < chars.len() {
        if chars[k] == ' ' {
            let prev = chars[..k].iter().rev().find(|c| **c != ' ').copied();
            let next = chars[k + 1..].iter().find(|c| **c != ' ').copied();
            // `sin x` のように関数名の後の空白も許す
            let after_func = {
                let before: String = chars[..k].iter().collect();
                FUNCS.iter().any(|(name, _)| before.ends_with(name))
            };
            // ∫ や Σ で始まる式は、 被積分関数・被和関数との間の空白も許す (∫_0^1 x dx)
            let big_start = matches!(chars.first(), Some('∫' | 'Σ'));
            if !(prev.is_some_and(is_glue) || next.is_some_and(is_glue) || after_func || big_start)
            {
                chars.truncate(k);
                ends.truncate(k);
                break;
            }
        }
        k += 1;
    }
    // 括弧の釣り合い: 余分な ) の手前まで
    let mut depth = 0usize;
    let mut cut = chars.len();
    for (idx, c) in chars.iter().enumerate() {
        match c {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    cut = idx;
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    chars.truncate(cut);
    ends.truncate(cut);
    // 閉じていない ( は、 その手前まで
    loop {
        let mut stack: Vec<usize> = Vec::new();
        for (idx, c) in chars.iter().enumerate() {
            match c {
                '(' => stack.push(idx),
                ')' => {
                    stack.pop();
                }
                _ => {}
            }
        }
        if let Some(&open) = stack.first() {
            chars.truncate(open);
            ends.truncate(open);
        } else {
            break;
        }
    }
    // 末尾の数の直後が助数詞・単位らしい日本語 (`=61人` / `√57話` / `=70%`) なら、 その数は式に含めない
    // (数 + 助数詞は従来の読み方に任せる。 `=` などの演算子までを式にする)
    let mut keep_tail = false;
    if chars.last().is_some_and(char::is_ascii_digit) {
        let end_byte = *ends.last()?;
        let counter_next = input[end_byte..].chars().next().is_some_and(
            |c| matches!(c, '\u{30A0}'..='\u{30FF}' | '\u{4E00}'..='\u{9FFF}' | 'つ' | '%' | '％'),
        );
        if counter_next {
            let mut j = chars.len();
            while j > 0 && (chars[j - 1].is_ascii_digit() || matches!(chars[j - 1], '.' | ',')) {
                j -= 1;
            }
            let before = chars[..j].iter().rev().find(|c| **c != ' ').copied();
            match before {
                // 数だけ・√ の直後の数は、 従来の読み (るーと + 数詞) に任せる
                None | Some('√') => return None,
                Some(op) if is_op(op) && op != '/' => {
                    chars.truncate(j);
                    ends.truncate(j);
                    keep_tail = true;
                    while chars.last() == Some(&' ') {
                        chars.pop();
                        ends.pop();
                    }
                }
                _ => {}
            }
        }
    }
    // 末尾の空白・記号は式に含めない (x=3, / x+ / 2^)
    while let Some(&last) = chars.last().filter(|_| !keep_tail) {
        // 1+x+x²/2!+... / 2,3,5,... のように演算子・カンマの後の省略記号は式に残す
        if ends_with_ellipsis_after_separator(&chars) {
            break;
        }
        if last == ' '
            || last == ','
            || last == '.'
            || last == '^'
            || last == '√'
            // 末尾の ! は、 数の後だけ感嘆符 (x=3!)。 n! / (n-r)! は階乗として残す
            || (last == '!'
                && !chars
                    .len()
                    .checked_sub(2)
                    .is_some_and(|k| chars[k].is_ascii_alphabetic() || chars[k] == ')'))
            || last == '_'
            || (is_op(last))
        {
            chars.pop();
            ends.pop();
        } else {
            break;
        }
    }
    if chars.is_empty() {
        return None;
    }
    // 日付・電話の区切り (2024-10-19 / 090-1234-5678) は式にしない
    let text: String = chars.iter().collect();
    if text.split('-').count() >= 3 && text.chars().all(|c| c.is_ascii_digit() || c == '-') {
        return None;
    }
    let toks = tokenize(&chars)?;
    if !is_math(&toks) {
        return None;
    }
    let mut parser = Parser { toks: &toks, i: 0 };
    let reading = parser.seq(false)?;
    if parser.i != toks.len() || reading.is_empty() {
        return None;
    }
    Some((*ends.last()?, reading))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(s: &str) -> Option<String> {
        read_math(s, 0).map(|(end, r)| {
            assert!(end <= s.len());
            r
        })
    }

    #[test]
    fn p_value_threshold_is_read() {
        assert_eq!(read("p<0.05").unwrap(), "pショウナリゼロテンゼロゴ");
        assert!(read("i<10").is_none());
    }

    #[test]
    fn digits_glued_to_a_variable_are_a_name_not_math() {
        // E=mc2 の mc2 / x2 / a1 は添え字つきの名前。 数として読むと mcに になる
        assert!(read("E=mc2").is_none());
        assert!(read("y=x2+1").is_none());
        // 関数名の直後 (cos2θ) や、 数が先 (2x) は従来どおり式
        assert!(read("cos2θ=1-2sin²θ").is_some());
        assert!(read("2x-3>5").is_some());
    }

    #[test]
    fn swap_marks_and_plain_arrows_are_not_math() {
        // A⇔B = 交換 (トレード募集) / L⇔R = バンド名 / 手順の矢印 → は式にしない
        assert!(read("A⇔B").is_none());
        assert!(read("L⇔R").is_none());
        assert!(read("(q→r)∧(q→p)").is_none());
        assert!(read("A→B").is_none());
        // 極限の → は従来どおり
        assert!(read("lim(x→0)sin x/x=1").is_some());
    }

    #[test]
    fn sigma_sum_over_n_is_a_division_of_the_sum() {
        // Σx/n は (Σx)/n。 Σ だけを分数の外に出して n分のx にはしない
        assert_eq!(read("μ=Σx/n").unwrap(), "ミューイコールシグマxワルn");
    }

    #[test]
    fn quadratic_equation_is_read_with_every_sign() {
        assert_eq!(
            read("x²-5x+b=0の式").unwrap(),
            "xノニジョウマイナスゴxプラスbイコールゼロ"
        );
        assert_eq!(
            read("a²+b²=c²").unwrap(),
            "aノニジョウプラスbノニジョウイコールcノニジョウ"
        );
        assert_eq!(
            read("(a+b)(a-b)=a²-b²").unwrap(),
            "(aプラスb)(aマイナスb)イコールaノニジョウマイナスbノニジョウ"
        );
    }

    #[test]
    fn span_end_stops_at_japanese_and_trailing_symbols() {
        let (end, _) = read_math("y=2x+3,と書く", 0).unwrap();
        assert_eq!(&"y=2x+3,と書く"[..end], "y=2x+3");
        let (end, _) = read_math("x=3。", 0).unwrap();
        assert_eq!(&"x=3。"[..end], "x=3");
    }

    #[test]
    fn fractions_read_denominator_first_when_both_sides_are_simple() {
        assert_eq!(
            read("1/2+1/3=5/6").unwrap(),
            "ニブンノイチプラスサンブンノイチイコールロクブンノゴ"
        );
        assert_eq!(read("y=1/x").unwrap(), "yイコールxブンノイチ");
        assert_eq!(read("y=x/2").unwrap(), "yイコールニブンノx");
        assert_eq!(
            read("V=4/3πr³").unwrap(),
            "Vイコールサンブンノヨンパイrノサンジョウ"
        );
        // かけ算が続く書き方は読み方が割れるので、 書かれたとおり割り算で読む
        assert_eq!(read("S=n(n+1)/2").unwrap(), "Sイコールn(nプラスイチ)ワルニ");
        assert_eq!(
            read("x=(b²-4ac)/2a").unwrap(),
            "xイコール(bノニジョウマイナスヨンac)ワルニa"
        );
    }

    #[test]
    fn powers_roots_functions_and_subscripts() {
        assert_eq!(
            read("x^2+3x-4=0").unwrap(),
            "xノニジョウプラスサンxマイナスヨンイコールゼロ"
        );
        assert_eq!(
            read("(x+1)^2=0").unwrap(),
            "(xプラスイチ)ノニジョウイコールゼロ"
        );
        assert_eq!(read("x^-1=y").unwrap(), "xノマイナスイチジョウイコールy");
        assert_eq!(read("√2").unwrap(), "ルートニ");
        assert_eq!(
            read("sinθ=1/2").unwrap(),
            "サインシータイコールニブンノイチ"
        );
        assert_eq!(read("log₂8=3").unwrap(), "ログニノハチイコールサン");
        assert_eq!(read("a₁+a₂=b").unwrap(), "aイチプラスaニイコールb");
        assert_eq!(
            read("cos60°=1/2").unwrap(),
            "コサインロクジュウドイコールニブンノイチ"
        );
    }

    #[test]
    fn relations() {
        assert_eq!(read("x≧3").unwrap(), "xダイナリイコールサン");
        assert_eq!(read("x>=3").unwrap(), "xダイナリイコールサン");
        assert_eq!(read("x>0 , y<0").unwrap(), "xダイナリゼロ,yショウナリゼロ");
        assert_eq!(read("x≠0").unwrap(), "xノットイコールゼロ");
    }

    #[test]
    fn abs_factorial_combination_arrows_braces_and_big_operators() {
        assert_eq!(
            read("|x-1|<2").unwrap(),
            "ゼッタイチ(xマイナスイチ)ショウナリニ"
        );
        assert_eq!(
            read("nCr=n!/(r!(n-r)!)").unwrap(),
            "nCrイコール(rノカイジョウ(nマイナスr)ノカイジョウ)ブンノnノカイジョウ"
        );
        assert_eq!(read("n!=24").unwrap(), "nノットイコールニジュウヨン");
        assert_eq!(
            read("e^{iπ}+1=0").unwrap(),
            "eノ(iパイ)ジョウプラスイチイコールゼロ"
        );
        assert_eq!(
            read("x^{n+1}=y").unwrap(),
            "xノ(nプラスイチ)ジョウイコールy"
        );
        assert_eq!(read("a_1+a_n=b").unwrap(), "aイチプラスaサブnイコールb");
        assert_eq!(
            read("a_{n+1}=2a_n").unwrap(),
            "aサブ(nプラスイチ)イコールニaサブn"
        );
        assert_eq!(
            read("Σ_{k=1}^{n}k=n(n+1)/2").unwrap(),
            "シグマ(kイコールイチ)カラ(n)マデkイコールn(nプラスイチ)ワルニ"
        );
        assert_eq!(
            read("lim(x→0)sin x/x=1").unwrap(),
            "リミット(xゼロヘ)サインxワルxイコールイチ"
        );
        assert_eq!(
            read("lim_{x→∞}1/x=0").unwrap(),
            "リミット(xムゲンダイヘ)xブンノイチイコールゼロ"
        );
        // 感嘆符・表の罫線・論理和・波括弧のコードは式にしない
        for s in [
            "x=3!", "a | b", "a||b", "a|b=1", "{a=1}", "wow!!", "my_var=3",
        ] {
            let r = read(s);
            assert!(r.is_none() || !r.unwrap().contains("カイジョウ"), "{s}");
        }
        assert_eq!(read("x=3!").unwrap(), "xイコールサン");
    }

    #[test]
    fn plus_minus_double_inequality_sets_geometry_and_primes() {
        assert_eq!(read("x=±3").unwrap(), "xイコールプラスマイナスサン");
        assert_eq!(read("N(μ,σ^2)").unwrap(), "N(ミュー,シグマノニジョウ)");
        assert_eq!(
            read("g^-1(x)=x/2").unwrap(),
            "gインバース(x)イコールニブンノx"
        );
        assert_eq!(
            read("5!=120").unwrap(),
            "ゴノカイジョウイコールヒャクニジュウ"
        );
        assert_eq!(
            read("v=v_0t+1/2at^2").unwrap(),
            "vイコールvゼロtプラスニブンノイチatノニジョウ"
        );
        assert_eq!(read("a:b=2:3").unwrap(), "aタイbイコールニタイサン");
        assert_eq!(
            read("n!/(n-2)!=n(n-1)").unwrap(),
            "(nマイナスニ)ノカイジョウブンノnノカイジョウイコールn(nマイナスイチ)"
        );
        assert_eq!(read("n!=0"), Some("nノットイコールゼロ".to_string()));
        assert_eq!(
            read("1-(99/100)^100≒63").unwrap(),
            "イチマイナス(ヒャクブンノキュウジュウキュウ)ノヒャクジョウニアリーイコールロクジュウサン"
        );
        assert_eq!(read("E[X]=np").unwrap(), "E(X)イコールnp");
        assert_eq!(
            read("1+2+3+...+n=n(n+1)/2").unwrap(),
            "イチプラスニプラスサンプラステンテンテンプラスnイコールn(nプラスイチ)ワルニ"
        );
        assert_eq!(read("n-1").unwrap(), "nマイナスイチ");
        assert_eq!(read("2x-3>5").unwrap(), "ニxマイナスサンダイナリゴ");
        assert_eq!(
            read("V=πr²h").unwrap(),
            "VイコールパイrノニジョウH".replace('H', "h")
        );
        assert_eq!(
            read("cos2θ=1-2sin²θ").unwrap(),
            "コサインニシータイコールイチマイナスニサインノニジョウシータ"
        );
        assert_eq!(
            read("c²=a²+b²-2abcosC").unwrap(),
            "cノニジョウイコールaノニジョウプラスbノニジョウマイナスニabコサインC"
        );
        assert_eq!(read("5P3=60").unwrap(), "ゴPサンイコールロクジュウ");
        assert_eq!(
            read("(x^n)'=nx^{n-1}").unwrap(),
            "(xノnジョウ)ダッシュイコールnxノ(nマイナスイチ)ジョウ"
        );
        assert_eq!(
            read("∫f g' dx=fg-∫f' g dx").unwrap(),
            "インテグラルfgダッシュdxイコールfgマイナスインテグラルfダッシュgdx"
        );
        assert_eq!(
            read("∫_0^1 x dx=1/2").unwrap(),
            "インテグラルゼロカライチマデxdxイコールニブンノイチ"
        );
        assert_eq!(
            read("e^x=1+x+x²/2!+...").unwrap(),
            "eノxジョウイコールイチプラスxプラスニノカイジョウブンノxノニジョウプラステンテンテン"
        );
        assert_eq!(
            read("¬(A∧B)=¬A∨¬B").unwrap(),
            "ノット(AカツB)イコールノットAマタハノットB"
        );
        assert_eq!(read("B-52"), None);
        assert_eq!(
            read("BMI=70/1.75^2").unwrap(),
            "BMIイコールイチテンナナゴノニジョウブンノナナジュウ"
        );
        // 日本語に付いた = の後の式 (利益=売上-原価=1000-700) は、 1000 から始める
        let s = "利益=売上-原価=1000-700=300円";
        let at = s.find("1000").unwrap();
        assert_eq!(
            read_math(s, at).unwrap().1,
            "センマイナスナナヒャクイコール"
        );
        assert_eq!(read("x∈[0,1]").unwrap(), "xゾクスル(ゼロ,イチ)");
        assert_eq!(read("A⊂B").unwrap(), "AブブンシュウゴウB");
        assert_eq!(
            read("∫₀^1x^2dx=1/3").unwrap(),
            "インテグラルゼロカライチマデxノニジョウdxイコールサンブンノイチ"
        );
        assert_eq!(read("4K=3840x2160"), None);
        assert_eq!(read("a<b>c"), None);
        assert_eq!(read("t=10:30"), None);
        assert_eq!(
            read("-3<x<5").unwrap(),
            "マイナスサンショウナリxショウナリゴ"
        );
        assert_eq!(read("A∪B").unwrap(), "AカップB");
        assert_eq!(read("x∈R").unwrap(), "xゾクスルR");
        assert_eq!(read("△ABC≡△DEF").unwrap(), "サンカクABCゴウドウサンカクDEF");
        assert_eq!(read("AB⊥CD").unwrap(), "ABスイチョクCD");
        assert_eq!(read("∠A=90°").unwrap(), "カクAイコールキュウジュウド");
        assert_eq!(read("f'(x)=2x").unwrap(), "fダッシュ(x)イコールニx");
        assert_eq!(read("f′(x)=2x").unwrap(), "fダッシュ(x)イコールニx");
        // 英語の ' や、 変数でない語の後の ' は式にしない
        assert_eq!(read("it's x=3"), None);
        assert_eq!(read("don't=1"), None);
        // 図形の名前 (大文字 2〜3 字) は図形・集合の記号がある式だけ
        assert_eq!(read("AB=CD"), None);
    }

    #[test]
    fn a_last_number_followed_by_a_counter_is_left_to_the_number_reader() {
        // `=61人` の 61 は 「ろくじゅういちにん」 と読ませるため式に含めない (演算子までを式にする)
        let s = "a+b=61人";
        let (end, r) = read_math(s, 0).unwrap();
        assert_eq!(&s[..end], "a+b=");
        assert_eq!(r, "aプラスbイコール");
        let s = "9×7=63人";
        let (end, r) = read_math(s, 0).unwrap();
        assert_eq!(&s[..end], "9×7=");
        assert_eq!(r, "キュウカケルナナイコール");
        // % は式に含めて 「ナナジュッパーセント」 と読む (50%OFF のように英数字が続く時は % の手前まで)。
        // √ の直後の数 (√57話) は式にしない
        assert_eq!(read("x=70%").unwrap(), "xイコールナナジュッパーセント");
        assert_eq!(
            read("20%=1/5").unwrap(),
            "ニジュッパーセントイコールゴブンノイチ"
        );
        let s = "x=50%OFF";
        assert_eq!(&s[..read_math(s, 0).unwrap().0], "x=");
        // ` が続く % は含めない (形態素解析の記号の塊の途中で式が終わってしまうため)
        let s = "`x = 50%` と";
        assert_eq!(&s[..read_math(s, 1).unwrap().0], "`x =");
        assert_eq!(read_math("√57話", 0), None);
        // 後ろが助数詞でなければ (ひらがなの助詞 / 英数字 / 文末) 数も読む
        assert_eq!(read("a+b=61").unwrap(), "aプラスbイコールロクジュウイチ");
    }

    #[test]
    fn thousands_separators_and_caret_kaomoji() {
        assert_eq!(read("1y=1,000,000").unwrap(), "イチyイコールヒャクマン");
        assert_eq!(read("e^x=3").unwrap(), "eノxジョウイコールサン");
        for s in ["(T^T)", "(*T^T)", "Y^Y^Y^Y", "T^T"] {
            assert_eq!(read(s), None, "{s}");
        }
    }

    #[test]
    fn leading_square_without_its_base_is_still_read() {
        assert_eq!(
            read("²-5x+b=0").unwrap(),
            "ニジョウマイナスゴxプラスbイコールゼロ"
        );
        assert_eq!(
            read("^2-5x+b=0").unwrap(),
            "ニジョウマイナスゴxプラスbイコールゼロ"
        );
    }

    #[test]
    fn words_and_code_are_not_math() {
        for s in [
            "HP=100",
            // 化学式・型番 (大文字の直後に数)
            "2H2+O2=2H2O",
            "Z8=Z9",
            // CSS の詳細度・16 進数・番号
            "0-1-0 + 0-0-1 = 0-1-1",
            "0x000601 = 1536",
            "cv03=1",
            // 数に直接付く単位
            "54.5kg=120lb",
            "100W*2=200W",
            "(3t)525+(3t)300=1200",
            // 顔文字・英単語・不等号が 1 つだけのコード
            "(=-ω-)",
            r"(\/)O=O(\/)",
            // 顔文字 (o∀o) (Σd)
            "(o∀o)",
            "Σd(≧▽≦)",
            // CLI のフラグ・単位つきの数
            "--v=0",
            "--min=2 --max=10",
            "2GB-1 (2^32-1)",
            "R:N=1:3-4",
            "ΦωΦ = ΦωΦ",
            "w=2",
            "ww=3",
            "φ=φ",
            "(≧v≦)",
            "(^v^)",
            "id=5",
            "i<10",
            "I<3",
            "^^",
            "val=3",
            "a==b",
            "i++",
            "x=>y",
            "AB=CD",
            "2024-10-19",
            "090-1234-5678",
            "hello world",
            "3-1",
            "A/B",
            "10/5",
            "x",
            "w/o",
            "1.2.3",
            "(^^)",
            "foo=bar",
            "key=value",
        ] {
            assert_eq!(read(s), None, "{s}");
        }
    }

    #[test]
    fn does_not_start_in_the_middle_of_a_word_or_expression() {
        assert_eq!(read_math("abx=3", 1), None);
        assert_eq!(read_math("x=3+y", 2), None);
        // HTML のタグ (<p>1 / 20) の中から始めない
        assert_eq!(read_math("<p>1 / 20 ページ", 1), None);
        // URL のクエリ (?p=1 / &x=3) の途中からは始めない
        assert_eq!(read_math("a.com?p=348424", 7), None);
        assert_eq!(read_math("a.com?q=1&x=3", 11), None);
        let s = "の式 x=3";
        assert!(read_math(s, s.find('x').unwrap()).is_some());
    }

    #[test]
    fn spaces_join_only_around_operators() {
        let (end, _) = read_math("x + y = 3 です", 0).unwrap();
        assert_eq!(&"x + y = 3 です"[..end], "x + y = 3");
        assert_eq!(read("x is 3=y"), None);
    }

    #[test]
    fn full_width_input_is_folded() {
        assert_eq!(read("ｘ＝２ｙ＋３").unwrap(), "xイコールニyプラスサン");
    }
}
