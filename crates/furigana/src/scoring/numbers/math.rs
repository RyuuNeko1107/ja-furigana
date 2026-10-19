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
const UNIT_RUNS: [&str; 32] = [
    "kg", "km", "cm", "mm", "mg", "ml", "ms", "hz", "gb", "mb", "kb", "tb", "pt", "px", "pc", "kw",
    "mw", "db", "ft", "lb", "oz", "ha", "cc", "m", "g", "t", "s", "h", "l", "L", "W", "V",
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
    match c {
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
        '\u{2212}' => '-',
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
    )
}

/// 式の中に現れうる字 (空白は別に扱う)
fn is_span_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || is_op(c)
        || matches!(c, '(' | ')' | '^' | ',' | '.' | '√' | '°')
        || sup_digit(c).is_some()
        || sub_digit(c).is_some()
        || c == '⁻'
        || greek(c).is_some()
}

/// 式を始められる字
fn is_start_char(c: char) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(c, '(' | '√' | '-' | '+' | '²' | '³' | '^')
        || greek(c).is_some()
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(String),
    Var(String),
    Func(&'static str),
    Greek(&'static str),
    Op(char),
    Open,
    Close,
    Sup(String),
    Sub(String),
    Caret,
    Sqrt,
    Deg,
    Comma,
}

/// 範囲を字に切る。 英字の語・コードらしいもの (`HP` / `val` / `a==b` / `i++`) があれば None
fn tokenize(chars: &[char]) -> Option<Vec<Tok>> {
    let mut toks: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' => i += 1,
            '0'..='9' => {
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
                // 大文字の直後に数が付く形 (H2 / O2 / C60 / A4) は化学式・型番・判型なので式にしない
                if run.len() == 1
                    && run.chars().all(|c| c.is_ascii_uppercase())
                    && chars.get(i).is_some_and(char::is_ascii_digit)
                {
                    return None;
                }
                // 数に直接付く単位 (54.5kg / 100W / 3t) は式にしない (単位は従来の読み方に任せる)
                if start > 0
                    && chars[start - 1].is_ascii_digit()
                    && UNIT_RUNS.contains(&run.as_str())
                {
                    return None;
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
            '.' => return None,
            c if greek(c).is_some() => {
                toks.push(Tok::Greek(greek(c)?));
                i += 1;
            }
            c if is_op(c) => {
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
            Tok::Sqrt => {
                self.i += 1;
                let Item::Atom { r, .. } = self.atom()? else {
                    return None;
                };
                (format!("ルート{r}"), false)
            }
            Tok::Open => {
                self.i += 1;
                let inner = self.seq(true)?;
                if self.toks.get(self.i) != Some(&Tok::Close) {
                    return None;
                }
                self.i += 1;
                (format!("({inner})"), false)
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
                    if func {
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
                    if r.is_empty() {
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
                    r = if r.is_empty() {
                        format!("{sign}{e}ジョウ")
                    } else {
                        format!("{r}ノ{sign}{e}ジョウ")
                    };
                }
                Some(Tok::Deg) => {
                    r.push('ド');
                    self.i += 1;
                }
                _ => break,
            }
        }
        Some(Item::Atom { r, func })
    }

    /// 指数の項 (累乗の連鎖 x^y^z は読まない)
    fn atom_no_pow(&mut self) -> Option<Item> {
        match self.toks.get(self.i)?.clone() {
            Tok::Num(s) => {
                self.i += 1;
                Some(Item::Atom {
                    r: number_to_katakana(&s),
                    func: false,
                })
            }
            Tok::Var(s) => {
                self.i += 1;
                Some(Item::Atom { r: s, func: false })
            }
            Tok::Open => self.atom(),
            _ => None,
        }
    }

    /// 項と演算子の並び。 `in_group` なら `)` の手前で止める
    fn seq(&mut self, in_group: bool) -> Option<String> {
        let mut items: Vec<Item> = Vec::new();
        while let Some(t) = self.toks.get(self.i) {
            match t {
                Tok::Close if in_group => break,
                Tok::Close => return None,
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

/// 演算子の読み
fn op_reading(c: char) -> &'static str {
    match c {
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
                    if !chained && single(&items[k..e]) && single(&items[e + 1..d_end]) {
                        out.push_str(&format!(
                            "{}ブンノ{}",
                            join(&items[e + 1..d_end]),
                            join(&items[k..e])
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
    let has_var = toks
        .iter()
        .any(|t| matches!(t, Tok::Var(_) | Tok::Func(_) | Tok::Greek(_)));
    let has_num = toks.iter().any(|t| matches!(t, Tok::Num(_)));
    // ギリシャ文字だけ (顔文字 ΦωΦ / φ=φ) は式にしない: 英字の変数か数が要る
    if !has_num && !toks.iter().any(|t| matches!(t, Tok::Var(_) | Tok::Func(_))) {
        return false;
    }
    let eq = toks.contains(&Tok::Op('='));
    let strong_rel = toks
        .iter()
        .any(|t| matches!(t, Tok::Op('≦' | '≧' | '≠' | '≒')));
    // < > は、 `x>0 , y<0` のように 2 つ以上並ぶ時だけ不等号にする (`i<10` / `I<3` / 矢印・顔文字を除く)
    let weak_rel = toks
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            matches!(t, Tok::Op('<' | '>'))
                && *i > 0
                && i + 1 < toks.len()
                && matches!(toks[i - 1], Tok::Var(ref v) if v.len() == 1)
                && matches!(toks[i + 1], Tok::Num(_) | Tok::Var(_))
        })
        .count()
        >= 2;
    let pow = toks.iter().enumerate().any(|(i, t)| match t {
        Tok::Sup(_) => true,
        Tok::Caret => {
            i > 0
                && matches!(toks[i - 1], Tok::Num(_) | Tok::Var(_) | Tok::Close)
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
    // 構文がおかしい (記号どうしの並び) ものは式にしない
    let ops_ok = toks.windows(2).all(|w| match (&w[0], &w[1]) {
        (Tok::Op(a), Tok::Op(b)) => matches!(b, '-' | '+') && !matches!(a, '-' | '+'),
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
                Tok::Num(_) | Tok::Var(_) | Tok::Close | Tok::Sub(_)
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
    // 数をハイフンで 3 つ以上つないだ並び (0-1-0 = CSS の詳細度 / 100-200-300 = 番号) は引き算の式にしない
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
        && ops_ok
        && caret_ok
        && !kaomoji
        && !hyphen_chain;
    if !structure_ok {
        return false;
    }
    (has_var && (eq || strong_rel || weak_rel || pow))
        || sqrt
        || (!has_var && has_num && eq && (arith || pow))
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
        if (is_span_char(p) && p != ' ')
            || matches!(
                p,
                '_' | '\'' | '\u{2019}' | '?' | '&' | '#' | '@' | '%' | '~' | '\\'
            )
        {
            return None;
        }
    }
    // 範囲を取る (折りたたんだ字と、 各字の終端 byte)
    let mut chars: Vec<char> = Vec::new();
    let mut ends: Vec<usize> = Vec::new();
    for (i, c) in rest.char_indices() {
        let f = fold(c);
        if !(is_span_char(f) || f == ' ') || chars.len() >= MAX_SPAN {
            break;
        }
        chars.push(f);
        ends.push(pos + i + c.len_utf8());
    }
    // 空白は演算子・カンマ・括弧の隣だけ許す (英単語の並びを式にしない)
    let is_glue = |c: char| is_op(c) || matches!(c, ',' | '(' | ')');
    let mut k = 0;
    while k < chars.len() {
        if chars[k] == ' ' {
            let prev = chars[..k].iter().rev().find(|c| **c != ' ').copied();
            let next = chars[k + 1..].iter().find(|c| **c != ' ').copied();
            if !(prev.is_some_and(is_glue) || next.is_some_and(is_glue)) {
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
        if last == ' ' || last == ',' || last == '.' || last == '^' || last == '√' || (is_op(last))
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
            "Vイコールヨンワルサンパイrノサンジョウ"
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
        // % の前も同じ。 √ の直後の数 (√57話) は式にしない
        let s = "x=70%";
        assert_eq!(&s[..read_math(s, 0).unwrap().0], "x=");
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
            "a_b=1",
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
