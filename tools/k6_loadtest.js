import http from "k6/http";
import { check, sleep } from "k6";
import { Rate, Trend, Counter } from "k6/metrics";

const errorRate = new Rate("errors");
const furiganaLatency = new Trend("furigana_latency", true);
const statusErrors = new Counter("status_errors");

const BASE_URL = __ENV.BASE_URL || "http://127.0.0.1:8000";

const payloads = [
  // 短文 (配信コメント想定)
  { text: "こんにちは", mode: "tts" },
  { text: "草", mode: "tts" },
  { text: "888888", mode: "tts" },
  // 中文 (一般的な文)
  { text: "東京都渋谷区で大規模なイベントが開催されました", mode: "tts" },
  { text: "田中太郎さんが新しいプロジェクトを発表した", mode: "tts" },
  { text: "今日の天気は晴れのち曇りでしょう", mode: "hiragana" },
  // 長文
  {
    text: "日本語の漢字には音読みと訓読みがあり、文脈によって読み方が変わることがあります。例えば「生」という漢字は「せい」「しょう」「い」「う」「なま」など多くの読み方を持っています。",
    mode: "tts",
  },
  // ruby モード
  { text: "吾輩は猫である。名前はまだ無い。", mode: "ruby" },
  // romaji モード
  { text: "桜の花が咲きました", mode: "romaji" },
  // analyze モード
  { text: "日本語変換テスト", mode: "analyze" },
];

// --- シナリオ定義 ---
export const options = {
  scenarios: {
    // 1) smoke: 正常動作確認 (1 VU)
    smoke: {
      executor: "constant-vus",
      vus: 1,
      duration: "10s",
      startTime: "0s",
      tags: { scenario: "smoke" },
    },
    // 2) baseline: 通常負荷 (5 VU, 30s)
    baseline: {
      executor: "constant-vus",
      vus: 5,
      duration: "30s",
      startTime: "15s",
      tags: { scenario: "baseline" },
    },
    // 3) ramp: 段階負荷 (0→10→0 VU)
    ramp: {
      executor: "ramping-vus",
      startVUs: 0,
      stages: [
        { duration: "15s", target: 5 },
        { duration: "30s", target: 10 },
        { duration: "15s", target: 0 },
      ],
      startTime: "50s",
      tags: { scenario: "ramp" },
    },
  },
  thresholds: {
    http_req_duration: ["p(95)<200", "p(99)<500"],
    errors: ["rate<0.01"],
    furigana_latency: ["p(95)<200"],
  },
};

export default function () {
  const payload = payloads[Math.floor(Math.random() * payloads.length)];

  const res = http.post(`${BASE_URL}/furigana`, JSON.stringify(payload), {
    headers: { "Content-Type": "application/json" },
    timeout: "10s",
  });

  const passed = check(res, {
    "status is 200": (r) => r.status === 200,
    "has result field": (r) => {
      try {
        return JSON.parse(r.body).result !== undefined;
      } catch {
        return false;
      }
    },
  });

  if (!passed) {
    statusErrors.add(1, { status: String(res.status) });
    if (__ITER < 3) {
      console.warn(
        `[VU=${__VU}] status=${res.status} body=${(res.body || "").substring(0, 200)}`
      );
    }
  }

  errorRate.add(!passed);
  if (res.timings && res.timings.duration > 0) {
    furiganaLatency.add(res.timings.duration);
  }

  sleep(0.5);
}

export function setup() {
  const res = http.get(`${BASE_URL}/healthz`);
  check(res, {
    "healthz returns 200": (r) => r.status === 200,
  });
  if (res.status !== 200) {
    throw new Error(`server not ready: ${res.status}`);
  }
  const body = JSON.parse(res.body);
  console.log(`server ready — dict_size: ${body.dict_size}`);
}
