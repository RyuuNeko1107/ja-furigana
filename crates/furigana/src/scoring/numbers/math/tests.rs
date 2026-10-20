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
