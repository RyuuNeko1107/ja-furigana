//! 字句の列が「数式として読んでよいか」 の判定 ([`is_math`])

use super::lexer::Tok;

/// 数式として読む価値があるか (変数・数に `=` / 不等号 / 累乗 / `√` が付く)
pub(super) fn is_math(toks: &[Tok]) -> bool {
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
