//! 数式らしい範囲を字句 ([`Tok`]) に分ける

use super::chars::greek;
use super::chars::is_op;
use super::chars::sub_digit;
use super::chars::sup_digit;
use super::chars::FUNCS;
use super::chars::UNIT_RUNS;
use super::chars::WORDS;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tok {
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
pub(super) fn tokenize(chars: &[char]) -> Option<Vec<Tok>> {
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
