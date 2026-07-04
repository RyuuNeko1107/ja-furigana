#!/usr/bin/env node
/**
 * VOICEVOX reading bridge — ja-furigana で誤読した単語だけ読みを直して
 * VOICEVOX に喋らせる、 棒読みちゃん互換のローカル読み上げサーバー (サンプル)。
 *
 * ## 設計: 韻律はエンジン、 読みは辞書
 *
 * TTS エンジンの抑揚 (アクセント・句のまとまり・自然な下降) は、 エンジンに
 * **漢字のままテキストを渡した時** に最も良くなる。 全文をひらがな化すると読みは
 * 正しくなるが抑揚が平坦になり、 アクセント記法 (`is_kana=true`) での全上書きは
 * 句が細切れになる (いずれも実聴比較で確認)。
 *
 * そこで本ブリッジは節 (句読点区切り) ごとに:
 *
 *   ① ja-furigana の読みと VOICEVOX 自身の読みを phoneme 列で比較
 *   ② 一致 → 漢字のまま合成 (エンジンの韻律をフル活用)
 *   ③ 不一致 → phoneme 列を diff し、 **ズレた単語だけ** 読みカナに置換して合成
 *       例: 「峠道は険しい」 → VOICEVOX は 「とうげどう」 と誤読
 *           → 「とうげみちは険しい」 (峠道だけカナ、 韻律の犠牲は最小)
 *
 * ## 使い方
 *
 *   furigana dict pull            # 辞書取得 (初回のみ)
 *   furigana serve                # ja-furigana HTTP server (localhost:8000)
 *   VOICEVOX を起動               # localhost:50021
 *   node bridge.js                # 本ブリッジ (localhost:50080)
 *
 * あとは棒読みちゃん互換クライアント (わんコメ等) を localhost:50080 に向けるだけ。
 * `GET /talk?text=...` で受けて VOICEVOX で読み上げる。
 *
 * ## 環境変数 (全部 optional)
 *
 *   BRIDGE_PORT       default 50080
 *   FURIGANA_API      default http://127.0.0.1:8000/furigana (furigana serve)
 *   FURIGANA_API_KEY  serve を認証付きで立てた場合の X-API-Key
 *   VOICEVOX_URL      default http://127.0.0.1:50021
 *   VOICEVOX_SPEAKER  default 3 (ずんだもん ノーマル)
 *   API_TIMEOUT_MS    default 1500
 *   READING_FIX       0 で読み修正を切って完全素通し
 *
 * 依存: Node.js 18+ のみ (npm install 不要)。 再生は PowerShell SoundPlayer
 * (Windows 前提。 他 OS は playWav を aplay / afplay 等に差し替え)。
 *
 * License: MIT (ja-furigana と同じ)
 */

const http = require('http')
const { spawn } = require('child_process')
const fs = require('fs')
const os = require('os')
const path = require('path')

const PORT = parseInt(process.env.BRIDGE_PORT || '50080', 10)
const FURIGANA_API = (process.env.FURIGANA_API || 'http://127.0.0.1:8000/furigana').replace(/\/+$/, '')
const FURIGANA_API_KEY = process.env.FURIGANA_API_KEY || ''
const VOICEVOX_URL = (process.env.VOICEVOX_URL || 'http://127.0.0.1:50021').replace(/\/+$/, '')
const SPEAKER = parseInt(process.env.VOICEVOX_SPEAKER || '3', 10)
const API_TIMEOUT_MS = parseInt(process.env.API_TIMEOUT_MS || '1500', 10)
const READING_FIX = process.env.READING_FIX !== '0'

let taskSeq = 0
const queue = []
let playing = false
let currentPlayer = null

function log(...args) {
  const ts = new Date().toLocaleTimeString('ja-JP', { hour12: false })
  console.log(`[${ts}]`, ...args)
}

// ─── ja-furigana API ─────────────────────────────────────────────────────────
function toB64(text) {
  return Buffer.from(text, 'utf8').toString('base64')
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

async function apiGet(params) {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), API_TIMEOUT_MS)
  try {
    const headers = FURIGANA_API_KEY ? { 'X-API-Key': FURIGANA_API_KEY } : {}
    const res = await fetch(`${FURIGANA_API}?${params}`, { signal: controller.signal, headers })
    if (!res.ok) throw new Error(`furigana api ${res.status}`)
    return await res.json()
  } finally {
    clearTimeout(timer)
  }
}

/// 読み (カタカナ)。 mode=voicevox-aques の記法から marker を除去したもの
/// (は/へ/を の発音変換 ワ/エ/オ が入っており VOICEVOX の発音カナと比較できる)。
async function fetchReading(text) {
  try {
    const data = await apiGet(`text_b64=${toB64(text)}&mode=voicevox-aques`)
    const kana = data?.result
    if (typeof kana !== 'string' || kana.length === 0) return null
    return kana.replace(/['/？]/g, '')
  } catch (e) {
    log('furigana api 失敗 (素通し fallback):', e?.message || e)
    return null
  }
}

/// token 列 (surface + reading)。 `furigana serve` は accent フィールド、
/// カスタム wrapper では result 直下に来る場合があるため両対応。
async function fetchTokens(text) {
  try {
    const data = await apiGet(`text_b64=${toB64(text)}&mode=accent`)
    return data?.accent?.tokens || data?.result?.tokens || null
  } catch (e) {
    log('  fetchTokens 失敗:', e?.message || e)
    return null
  }
}

// ─── カタカナ → phoneme 列 ───────────────────────────────────────────────────
const DIGRAPH = {
  'キャ': 'ky a', 'キュ': 'ky u', 'キョ': 'ky o', 'ギャ': 'gy a', 'ギュ': 'gy u', 'ギョ': 'gy o',
  'シャ': 'sh a', 'シュ': 'sh u', 'ショ': 'sh o', 'シェ': 'sh e',
  'ジャ': 'j a', 'ジュ': 'j u', 'ジョ': 'j o', 'ジェ': 'j e',
  'チャ': 'ch a', 'チュ': 'ch u', 'チョ': 'ch o', 'チェ': 'ch e',
  'ニャ': 'ny a', 'ニュ': 'ny u', 'ニョ': 'ny o',
  'ヒャ': 'hy a', 'ヒュ': 'hy u', 'ヒョ': 'hy o',
  'ビャ': 'by a', 'ビュ': 'by u', 'ビョ': 'by o',
  'ピャ': 'py a', 'ピュ': 'py u', 'ピョ': 'py o',
  'ミャ': 'my a', 'ミュ': 'my u', 'ミョ': 'my o',
  'リャ': 'ry a', 'リュ': 'ry u', 'リョ': 'ry o',
  'ファ': 'f a', 'フィ': 'f i', 'フェ': 'f e', 'フォ': 'f o',
  'ティ': 't i', 'トゥ': 't u', 'ディ': 'd i', 'ドゥ': 'd u',
  'ウィ': 'w i', 'ウェ': 'w e', 'ウォ': 'w o',
  'ツァ': 'ts a', 'ツィ': 'ts i', 'ツェ': 'ts e', 'ツォ': 'ts o',
  'ヴァ': 'v a', 'ヴィ': 'v i', 'ヴェ': 'v e', 'ヴォ': 'v o',
  'デュ': 'dy u', 'テュ': 'ty u', 'フュ': 'fy u', 'ヴュ': 'by u',
}
const MONO = {
  'ア': 'a', 'イ': 'i', 'ウ': 'u', 'エ': 'e', 'オ': 'o',
  'カ': 'k a', 'キ': 'k i', 'ク': 'k u', 'ケ': 'k e', 'コ': 'k o',
  'ガ': 'g a', 'ギ': 'g i', 'グ': 'g u', 'ゲ': 'g e', 'ゴ': 'g o',
  'サ': 's a', 'シ': 'sh i', 'ス': 's u', 'セ': 's e', 'ソ': 's o',
  'ザ': 'z a', 'ジ': 'j i', 'ズ': 'z u', 'ゼ': 'z e', 'ゾ': 'z o',
  'タ': 't a', 'チ': 'ch i', 'ツ': 'ts u', 'テ': 't e', 'ト': 't o',
  'ダ': 'd a', 'ヂ': 'j i', 'ヅ': 'z u', 'デ': 'd e', 'ド': 'd o',
  'ナ': 'n a', 'ニ': 'n i', 'ヌ': 'n u', 'ネ': 'n e', 'ノ': 'n o',
  'ハ': 'h a', 'ヒ': 'h i', 'フ': 'f u', 'ヘ': 'h e', 'ホ': 'h o',
  'バ': 'b a', 'ビ': 'b i', 'ブ': 'b u', 'ベ': 'b e', 'ボ': 'b o',
  'パ': 'p a', 'ピ': 'p i', 'プ': 'p u', 'ペ': 'p e', 'ポ': 'p o',
  'マ': 'm a', 'ミ': 'm i', 'ム': 'm u', 'メ': 'm e', 'モ': 'm o',
  'ヤ': 'y a', 'ユ': 'y u', 'ヨ': 'y o',
  'ラ': 'r a', 'リ': 'r i', 'ル': 'r u', 'レ': 'r e', 'ロ': 'r o',
  'ワ': 'w a', 'ヲ': 'o', 'ン': 'N', 'ッ': 'cl', 'ヴ': 'v u',
  'ァ': 'a', 'ィ': 'i', 'ゥ': 'u', 'ェ': 'e', 'ォ': 'o',
}
const O_ROW = 'オコソトノホモヨロヲゴゾドボポョォ'
const E_ROW = 'エケセテネヘメレゲゼデベペェ'
const VOWELS = new Set(['a', 'i', 'u', 'e', 'o'])
const KANJI_RE = /[㐀-䶿一-鿿豈-﫿]/

function hiraToKata(s) {
  let out = ''
  for (const c of s) {
    const cp = c.codePointAt(0)
    out += (cp >= 0x3041 && cp <= 0x3096) ? String.fromCodePoint(cp + 0x60) : c
  }
  return out
}

/// 表記読み (トウゲ / カンリョウ) と発音カナ (トオゲ / カンリョオ) の差を吸収する
/// ため、 オ段+ウ / エ段+イ / 長音符は直前母音の連打に畳んで phoneme 化する。
function kanaToPhonemes(reading) {
  const kata = hiraToKata(reading)
  const out = []
  let i = 0
  const chars = [...kata]
  const lastVowel = () => {
    for (let j = out.length - 1; j >= 0; j--) if (VOWELS.has(out[j])) return out[j]
    return null
  }
  while (i < chars.length) {
    const two = chars[i] + (chars[i + 1] || '')
    const c = chars[i]
    if (DIGRAPH[two]) {
      out.push(...DIGRAPH[two].split(' '))
      i += 2
      continue
    }
    if (out.length > 0) {
      const prev = chars[i - 1] || ''
      if ((c === 'ウ' && O_ROW.includes(prev)) || (c === 'イ' && E_ROW.includes(prev))) {
        const v = lastVowel()
        if (v) { out.push(v); i += 1; continue }
      }
    }
    if (c === 'ー') {
      const v = lastVowel()
      if (v) out.push(v)
      i += 1
      continue
    }
    if (MONO[c]) out.push(...MONO[c].split(' '))
    i += 1
  }
  return out.map((p) => p.toLowerCase())
}

// ─── 誤読判定 + token 単位部分置換 ───────────────────────────────────────────
const MISMATCH_LOG = path.join(__dirname, 'mismatch_log.jsonl')

/// VOICEVOX 自身の読み (accent_phrases の moras 連結、 発音カナ)
async function voicevoxOwnReading(text) {
  const q = new URLSearchParams({ speaker: String(SPEAKER), text })
  const res = await fetch(`${VOICEVOX_URL}/accent_phrases?${q}`, { method: 'POST' })
  if (!res.ok) throw new Error(`accent_phrases ${res.status}`)
  const phrases = await res.json()
  return phrases.map((p) => p.moras.map((m) => m.text).join('')).join('')
}

async function detectMisread(clause, reading) {
  if (!reading) return { misread: false, theirs: [] }
  try {
    const theirs = kanaToPhonemes(await voicevoxOwnReading(clause))
    const ours = kanaToPhonemes(reading)
    if (ours.join(' ') === theirs.join(' ')) return { misread: false, theirs }
    log(`  誤読検出: VV=[${theirs.join(' ')}] → [${ours.join(' ')}]`)
    // 不一致 log は辞書改善の材料になる (エンジンとの読み比較 = どちらかが誤読)
    fs.appendFile(MISMATCH_LOG, JSON.stringify({
      ts: new Date().toISOString(), clause, api: ours.join(' '), voicevox: theirs.join(' '),
    }) + '\n', () => {})
    return { misread: true, theirs }
  } catch (e) {
    return { misread: true, theirs: [] } // 比較不能 → 読み優先に倒す
  }
}

/// token 単位に phoneme 列を並べ、 各 token の位置 (span) を控える。
/// 助詞 は/へ/を は発音 (ワ/エ/オ) に変換して発音カナ側と比較可能にする。
function tokensToPhonemeSpans(tokens) {
  const stream = []
  const spans = []
  for (const t of tokens) {
    let reading = t.reading || ''
    if (t.surface === 'は') reading = 'ワ'
    else if (t.surface === 'へ') reading = 'エ'
    else if (t.surface === 'を') reading = 'オ'
    const ph = kanaToPhonemes(reading)
    spans.push({ start: stream.length, len: ph.length })
    stream.push(...ph)
  }
  return { stream, spans }
}

/// LCS diff: a の各位置が共通部分列に入っているか (false = 差分区間)。
function lcsMembership(a, b) {
  const n = a.length
  const m = b.length
  const dp = Array.from({ length: n + 1 }, () => new Uint16Array(m + 1))
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1])
    }
  }
  const inLcs = new Array(n).fill(false)
  let i = 0
  let j = 0
  while (i < n && j < m) {
    if (a[i] === b[j]) {
      inLcs[i] = true
      i++
      j++
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      i++
    } else {
      j++
    }
  }
  return inLcs
}

/// 誤読 token だけ読みに置換した mixed text を作る (他は漢字のまま韻律維持)。
///
/// token 単体でエンジンに読みを聞き直す方式は不可: エンジンは 「峠道」 単体なら
/// 正読するのに 「峠道は」 では誤読する等、 文脈で分かち書きと読みが変わる。
/// そのため文全体の phoneme 列同士を diff し、 ズレた区間に重なる token を特定する。
async function buildMixedText(clause, theirsStream) {
  const tokens = await fetchTokens(clause)
  if (!tokens) return null
  const { stream, spans } = tokensToPhonemeSpans(tokens)
  if (stream.length === 0) return null
  const inLcs = lcsMembership(stream, theirsStream)
  let out = ''
  let replaced = 0
  for (let k = 0; k < tokens.length; k++) {
    const t = tokens[k]
    const { start, len } = spans[k]
    let mismatched = false
    for (let p = start; p < start + len; p++) {
      if (!inLcs[p]) { mismatched = true; break }
    }
    if (mismatched && KANJI_RE.test(t.surface || '') && t.reading) {
      out += t.reading
      replaced++
    } else {
      out += t.surface || ''
    }
  }
  return { text: out, replaced }
}

// ─── VOICEVOX 合成 + 再生 ────────────────────────────────────────────────────
async function voicevoxSynthesize(text, opts) {
  const q = new URLSearchParams({ speaker: String(SPEAKER), text })
  const aqRes = await fetch(`${VOICEVOX_URL}/audio_query?${q}`, { method: 'POST' })
  if (!aqRes.ok) throw new Error(`audio_query ${aqRes.status}`)
  const query = await aqRes.json()
  // 棒読みちゃんパラメータの反映 (来ていれば): speed 100=等速, volume 0-100
  if (opts.speed > 0) query.speedScale = Math.min(3, Math.max(0.5, opts.speed / 100))
  if (opts.volume >= 0) query.volumeScale = Math.min(2, Math.max(0, opts.volume / 100))
  const synthRes = await fetch(`${VOICEVOX_URL}/synthesis?speaker=${SPEAKER}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(query),
  })
  if (!synthRes.ok) throw new Error(`synthesis ${synthRes.status}`)
  return Buffer.from(await synthRes.arrayBuffer())
}

function playWav(wavPath) {
  return new Promise((resolve) => {
    const ps = spawn('powershell', [
      '-NoProfile', '-NonInteractive', '-Command',
      `(New-Object Media.SoundPlayer '${wavPath.replace(/'/g, "''")}').PlaySync()`,
    ], { stdio: 'ignore', windowsHide: true })
    currentPlayer = ps
    ps.on('close', () => { currentPlayer = null; resolve() })
    ps.on('error', () => { currentPlayer = null; resolve() })
  })
}

// ─── 節分割 + queue ──────────────────────────────────────────────────────────
function splitClauses(text) {
  const out = []
  const re = /([^、。．.！!？?，,…\n]+)([、。．.！!？?，,…\n]*)/g
  let m
  while ((m = re.exec(text)) !== null) {
    const body = m[1].trim()
    if (body) out.push({ body, punct: m[2] || '' })
  }
  return out
}

/// 漢字 (または英字語) を含まない節は読み修正の対象外。
/// かな文は直すものが無い上、 助詞 は/へ/を の発音差で偽陽性が出て改悪する。
function isFixCandidate(body) {
  return /[㐀-䶿一-鿿豈-﫿]|[A-Za-z]{2,}/.test(body)
}

async function processQueue() {
  if (playing) return
  playing = true
  while (queue.length > 0) {
    const item = queue.shift()
    try {
      const wavs = []
      for (const cl of splitClauses(item.text)) {
        try {
          const fixable = READING_FIX && isFixCandidate(cl.body)
          const reading = fixable ? await fetchReading(cl.body) : null
          const { misread, theirs } = fixable
            ? await detectMisread(cl.body, reading)
            : { misread: false, theirs: [] }
          let speakText
          if (misread && reading) {
            const mixed = await buildMixedText(cl.body, theirs)
            if (mixed && mixed.replaced > 0) {
              log(`  ▶ 部分修正 (${mixed.replaced} 語): ${mixed.text}`)
              speakText = mixed.text + cl.punct
            } else {
              log(`  ▶ 読み修正 (全体): ${reading}`)
              speakText = reading + cl.punct
            }
          } else {
            log(`  ▶ 素 (正読): ${cl.body}`)
            speakText = cl.body + cl.punct
          }
          wavs.push(await voicevoxSynthesize(speakText, item))
        } catch (e) {
          log(`  節 [${cl.body}] 合成失敗:`, e?.message || e)
        }
      }
      for (const [i, wav] of wavs.entries()) {
        const wavPath = path.join(os.tmpdir(), `vv_bridge_${item.taskId}_${i}.wav`)
        fs.writeFileSync(wavPath, wav)
        await playWav(wavPath)
        fs.unlink(wavPath, () => {})
      }
    } catch (e) {
      log(`task ${item.taskId} 失敗 (VOICEVOX 停止中?):`, e?.message || e)
    }
  }
  playing = false
}

// ─── 棒読みちゃん互換 HTTP ────────────────────────────────────────────────────
const server = http.createServer((req, res) => {
  const u = new URL(req.url, `http://127.0.0.1:${PORT}`)
  const route = u.pathname.toLowerCase()
  const json = (obj) => {
    res.writeHead(200, { 'Content-Type': 'application/json; charset=utf-8' })
    res.end(JSON.stringify(obj))
  }
  if (route === '/talk') {
    const text = (u.searchParams.get('text') || '').trim()
    if (!text) return json({ taskId: 0, error: 'no text' })
    const taskId = ++taskSeq
    const item = {
      taskId,
      text,
      voice: parseInt(u.searchParams.get('voice') || '-1', 10),
      speed: parseInt(u.searchParams.get('speed') || '-1', 10),
      volume: parseInt(u.searchParams.get('volume') || '-1', 10),
    }
    log(`TALK #${taskId} [${text}]`)
    queue.push(item)
    processQueue()
    return json({ taskId })
  }
  if (route === '/clear') {
    queue.length = 0
    return json({ result: 'ok' })
  }
  if (route === '/skip') {
    if (currentPlayer) currentPlayer.kill()
    return json({ result: 'ok' })
  }
  if (route === '/gettalktaskcount') {
    return json({ count: queue.length + (playing ? 1 : 0) })
  }
  if (route === '/getnowplaying' || route === '/pause' || route === '/resume' || route === '/getpause') {
    return json({ result: 'ok' }) // 互換 no-op stub
  }
  res.writeHead(404, { 'Content-Type': 'application/json' })
  res.end('{"error":"not found"}')
})

server.listen(PORT, '127.0.0.1', () => {
  log(`VOICEVOX reading bridge listening on http://127.0.0.1:${PORT}`)
  log(`  furigana api : ${FURIGANA_API}`)
  log(`  voicevox     : ${VOICEVOX_URL} (speaker=${SPEAKER})`)
  log(`  読み修正     : ${READING_FIX ? 'ON (誤読 token のみ部分置換)' : 'OFF (完全素通し)'}`)
})
