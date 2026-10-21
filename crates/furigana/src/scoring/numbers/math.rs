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

mod chars;
mod judge;
mod lexer;
mod parse;

use self::chars::fold;
use self::chars::greek;
use self::chars::is_op;
use self::chars::is_span_char;
use self::chars::is_start_char;
use self::chars::is_trigger_char;
use self::chars::FUNCS;
use self::chars::MAX_SPAN;
use self::judge::is_math;
use self::lexer::tokenize;
use self::parse::Parser;

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
mod tests;
