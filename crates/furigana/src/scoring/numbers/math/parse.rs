//! 字句の列を項 ([`Item`]) にまとめ、 読み (カタカナ) に直す

use super::lexer::Tok;
use crate::numbers::helpers::sokuonize_last;
use crate::numbers::number_to_katakana;

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

pub(super) struct Parser<'a> {
    pub(super) toks: &'a [Tok],
    pub(super) i: usize,
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
    pub(super) fn seq(&mut self, in_group: bool) -> Option<String> {
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
