//! 形態素解析器 (Lindera + IPADIC ラッパ)
//!
//! [`Analyzer::new`] で IPADIC を埋め込みロードし、`tokenize` で
//! テキストを [`MorphToken`] 列に分解する。
//!
//! Lindera 自体の `Tokenizer` はスレッドセーフではないため、
//! 内部で `Mutex` 保護している。複数スレッドから同時呼び出し可能だが
//! 直列実行になることに注意。

use crate::error::{FuriganaError, Result};
use lindera::dictionary::load_dictionary;
use lindera::mode::Mode;
use lindera::segmenter::Segmenter;
use lindera::tokenizer::Tokenizer;
use std::sync::Mutex;

/// 形態素解析の 1 トークン
///
/// IPADIC の details 配列から `*` または空文字を `None` に正規化済み。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MorphToken {
    /// 表層形
    pub surface: String,
    /// カタカナ読み (IPADIC details[7])
    pub reading: Option<String>,
    /// 品詞 (IPADIC details[0]) — 名詞 / 動詞 / 形容詞 等
    pub pos: Option<String>,
    /// 品詞細分類 1 (IPADIC details[1]) — 普通名詞 / 固有名詞 / 数 等
    pub pos_detail: Option<String>,
    /// 活用型 (IPADIC details[4]) — 五段・カ行イ音便 / 一段 等
    pub conjugation_type: Option<String>,
    /// 活用形 (IPADIC details[5]) — 基本形 / 連用形 等
    pub conjugation_form: Option<String>,
    /// 原形 (IPADIC details[6]) — 「食べた」→「食べる」
    pub base_form: Option<String>,
}

impl MorphToken {
    /// surface だけセットしたトークンを返す (フォールバック用)
    fn surface_only(text: &str) -> Self {
        Self {
            surface: text.to_string(),
            reading: None,
            pos: None,
            pos_detail: None,
            conjugation_type: None,
            conjugation_form: None,
            base_form: None,
        }
    }
}

/// 形態素解析器
pub struct Analyzer {
    tokenizer: Mutex<Tokenizer>,
}

/// 埋め込み辞書の URI ★alpha.17。 feature flag (= `dict-ipadic` / `dict-unidic`) で
/// 排他的に switch。 IPADIC と UniDic で details field の意味が違うので、
/// reading / base_form の field 番号も合わせて切り替える。
#[cfg(all(feature = "dict-ipadic", feature = "dict-unidic"))]
compile_error!("Enable exactly one of `dict-ipadic` / `dict-unidic`, not both.");

#[cfg(not(any(feature = "dict-ipadic", feature = "dict-unidic")))]
compile_error!("Enable exactly one of `dict-ipadic` (default) / `dict-unidic` features.");

#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const EMBEDDED_DICT_URI: &str = "embedded://ipadic";
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const EMBEDDED_DICT_URI: &str = "embedded://unidic";

/// details field 番号: reading (= 表層形のカタカナ発音)。
///
/// - IPADIC: details[7] (カタカナ reading)
/// - UniDic: details[9] (pron = 発音形出現形)
#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const FIELD_READING: usize = 7;
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const FIELD_READING: usize = 9;

/// details field 番号: base_form (= 原形 / 辞書形)。
///
/// - IPADIC: details[6] (原形)
/// - UniDic: details[10] (orthBase = 書字形基本形)
#[cfg(all(feature = "dict-ipadic", not(feature = "dict-unidic")))]
const FIELD_BASE_FORM: usize = 6;
#[cfg(all(feature = "dict-unidic", not(feature = "dict-ipadic")))]
const FIELD_BASE_FORM: usize = 10;

impl Analyzer {
    /// 埋め込み辞書で初期化 (feature flag で IPADIC / UniDic 切替)
    ///
    /// # Errors
    /// 辞書ロードに失敗した場合 [`FuriganaError::AnalyzerInit`]。
    pub fn new() -> Result<Self> {
        let dictionary = load_dictionary(EMBEDDED_DICT_URI)
            .map_err(|e| FuriganaError::AnalyzerInit(format!("dictionary load: {e}")))?;
        let segmenter = Segmenter::new(Mode::Normal, dictionary, None);
        let tokenizer = Tokenizer::new(segmenter);
        Ok(Self {
            tokenizer: Mutex::new(tokenizer),
        })
    }

    /// テキストを分解してトークン列を返す
    ///
    /// 形態素解析が失敗 / Mutex が poison された場合は、入力全体を 1 トークン
    /// (surface のみ) として返す — 呼び出し側が常に何らかの結果を扱える保証。
    #[must_use]
    pub fn tokenize(&self, text: &str) -> Vec<MorphToken> {
        if text.is_empty() {
            return Vec::new();
        }

        // poison 時は lock を握り潰して surface-only に倒すと、 以降全 request が
        // 恒久 degrade (形態素解析なし) に固定されてしまう。 poison は別 thread の
        // panic 由来で、 `Tokenizer::tokenize` は `&self` (read-only、 内部可変は
        // Mutex で排他) なので、 lock を奪い返して継続するのが安全 (恒久 degrade 回避)。
        let tokenizer = self.tokenizer.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("Tokenizer mutex poisoned; recovering lock and continuing");
            poisoned.into_inner()
        });

        match tokenizer.tokenize(text) {
            Ok(mut tokens) => tokens
                .iter_mut()
                .map(|t| {
                    let surface = t.surface.to_string();
                    let details = t.details();
                    let get_detail = |i: usize| -> Option<String> {
                        details
                            .get(i)
                            .filter(|v| **v != "*" && !v.is_empty())
                            .map(ToString::to_string)
                    };
                    // ★alpha.17: UniDic は pron が長音符 「ー」 で長音を表すので
                    // (例: 「学校=ガッコー」)、 表記読み (「ガッコウ」) に正規化する。
                    // IPADIC は元々 表記読み なので no-op (= ー が出ない pattern)。
                    let reading =
                        get_detail(FIELD_READING).map(|r| crate::kana::normalize_long_vowel(&r));
                    MorphToken {
                        surface,
                        reading,
                        pos: details.first().map(ToString::to_string),
                        pos_detail: get_detail(1),
                        conjugation_type: get_detail(4),
                        conjugation_form: get_detail(5),
                        base_form: get_detail(FIELD_BASE_FORM),
                    }
                })
                .collect(),
            Err(e) => {
                tracing::warn!("tokenize error: {e}");
                vec![MorphToken::surface_only(text)]
            }
        }
    }
}

impl std::fmt::Debug for Analyzer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Analyzer").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyzer() -> Analyzer {
        Analyzer::new().expect("Analyzer init failed")
    }

    #[test]
    fn tokenizes_basic_japanese() {
        let a = analyzer();
        let tokens = a.tokenize("私は学生です");
        // surface 列を完全固定 (「私 が含まれる」 程度だと分割崩れを見逃す)。
        let surfaces: Vec<&str> = tokens.iter().map(|t| t.surface.as_str()).collect();
        assert_eq!(surfaces, vec!["私", "は", "学生", "です"]);
        // 名詞「学生」 の品詞と読みも値で固定する。
        let gakusei = tokens
            .iter()
            .find(|t| t.surface == "学生")
            .expect("学生 token");
        assert_eq!(gakusei.pos.as_deref(), Some("名詞"));
        assert_eq!(gakusei.reading.as_deref(), Some("ガクセイ"));
    }

    #[test]
    fn returns_reading_for_known_kanji() {
        let a = analyzer();
        let tokens = a.tokenize("読書");
        // 「どこかに reading が付く」 だと誤読でも緑になる。surface と reading を
        // 値で固定する (IPADIC は 「読書」 を 1 token ドクショ で返す)。
        let dokusho = tokens
            .iter()
            .find(|t| t.surface == "読書")
            .expect("読書 token");
        assert_eq!(dokusho.reading.as_deref(), Some("ドクショ"));
    }

    #[test]
    fn empty_input_yields_empty() {
        let a = analyzer();
        assert!(a.tokenize("").is_empty());
    }

    #[test]
    fn handles_mixed_script() {
        let a = analyzer();
        let tokens = a.tokenize("Hello世界123");
        // 「世界」が含まれる
        assert!(tokens.iter().any(|t| t.surface.contains("世")));
    }

    #[test]
    fn details_filter_asterisks_to_none() {
        // 助詞 (e.g., は) は activation_form 等が "*" になることが多い
        let a = analyzer();
        let tokens = a.tokenize("私は");
        // if let Some だと 「は」 が取れなければ無条件 pass していた。token 存在を強制。
        let token = tokens
            .iter()
            .find(|t| t.surface == "は")
            .expect("は token");
        // 助詞「は」は活用しないので conjugation_type は "*" → None 正規化されるはず
        assert!(token.conjugation_type.is_none());
    }

    #[test]
    fn tokenize_recovers_from_poisoned_mutex() {
        use std::sync::Arc;
        let a = Arc::new(analyzer());
        // 別 thread で tokenizer lock 保持中に panic させ mutex を poison させる。
        let a2 = Arc::clone(&a);
        let handle = std::thread::spawn(move || {
            let _guard = a2.tokenizer.lock().unwrap();
            panic!("intentional panic while holding tokenizer lock");
        });
        assert!(handle.join().is_err(), "panic で poison したはず");
        // poison 後も tokenize は recover して継続する (恒久 degrade に陥らない)。
        // 旧実装は surface-only fallback (= 1 token) を返していた。
        let tokens = a.tokenize("私は学生です");
        assert!(
            tokens.len() > 1,
            "poison から回復して形態素解析が継続するはず (surface-only ではない): {tokens:?}"
        );
    }
}
