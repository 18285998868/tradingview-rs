import type {
  ApiResponse,
  BacktestConfigResponse,
  BacktestRunDetail,
  BacktestRunSummary,
  CompareResponse,
  CreateBacktestRequest,
  KlinesData,
  MarketTypesData,
  PairsData,
  QualityReport,
  SubmitOutcome,
  TimeframesData,
} from './types';

const BASE = '/api';

async function requestJson<T>(url: string, init?: RequestInit): Promise<T> {
  const res = await fetch(url, init);
  if (!res.ok) {
    let message = `HTTP ${res.status}: ${res.statusText}`;
    try {
      const body = (await res.json()) as { error?: string };
      if (body?.error) message = body.error;
    } catch {
      // 非 JSON 错误体（例如代理返回的 HTML）保留状态码信息。
    }
    throw new Error(message);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

function jsonInit(method: string, body: unknown): RequestInit {
  return {
    method,
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  };
}

async function fetchJson<T>(url: string): Promise<T> {
  return requestJson<T>(url);
}

export async function fetchMarketTypes(): Promise<string[]> {
  const json = await fetchJson<ApiResponse<MarketTypesData>>(`${BASE}/market-types`);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.types;
}

export async function fetchPairs(marketType: string): Promise<string[]> {
  const json = await fetchJson<ApiResponse<PairsData>>(`${BASE}/pairs?market_type=${marketType}`);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.pairs;
}

export async function fetchTimeframes(marketType: string, pair: string): Promise<string[]> {
  const json = await fetchJson<ApiResponse<TimeframesData>>(
    `${BASE}/timeframes?market_type=${marketType}&pair=${pair}`,
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.timeframes;
}

export interface KlinesParams {
  marketType: string;
  pair: string;
  timeframe: string;
  limit?: number;
  before?: number;
  after?: number;
}

export async function fetchKlines(params: KlinesParams): Promise<KlinesData> {
  let url = `${BASE}/klines?market_type=${params.marketType}&pair=${params.pair}&timeframe=${params.timeframe}`;
  if (params.limit !== undefined) url += `&limit=${params.limit}`;
  if (params.before !== undefined) url += `&before=${params.before}`;
  if (params.after !== undefined) url += `&after=${params.after}`;
  const json = await fetchJson<ApiResponse<KlinesData>>(url);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export interface QualityParams {
  marketType: string;
  pair: string;
  timeframe: string;
}

export async function fetchQuality(params: QualityParams): Promise<QualityReport> {
  const url = `${BASE}/quality?market_type=${params.marketType}&pair=${params.pair}&timeframe=${params.timeframe}`;
  const json = await fetchJson<ApiResponse<QualityReport>>(url);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function fetchBacktestConfig(): Promise<BacktestConfigResponse> {
  const json = await fetchJson<ApiResponse<BacktestConfigResponse>>(`${BASE}/backtests/config`);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function submitBacktest(input: CreateBacktestRequest): Promise<SubmitOutcome> {
  const json = await requestJson<ApiResponse<SubmitOutcome>>(
    `${BASE}/backtests`,
    jsonInit('POST', input),
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function fetchBacktests(limit = 100): Promise<BacktestRunSummary[]> {
  const json = await fetchJson<ApiResponse<BacktestRunSummary[]>>(`${BASE}/backtests?limit=${limit}`);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function fetchBacktest(id: string): Promise<BacktestRunDetail> {
  const json = await fetchJson<ApiResponse<BacktestRunDetail>>(
    `${BASE}/backtests/${encodeURIComponent(id)}`,
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function fetchBacktestLog(id: string): Promise<string | null> {
  const json = await fetchJson<ApiResponse<{ log: string | null }>>(
    `${BASE}/backtests/${encodeURIComponent(id)}/log`,
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.log;
}

export async function retryBacktest(id: string): Promise<SubmitOutcome> {
  const json = await requestJson<ApiResponse<SubmitOutcome>>(
    `${BASE}/backtests/${encodeURIComponent(id)}/retry`,
    { method: 'POST' },
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function deleteBacktest(id: string): Promise<void> {
  const json = await requestJson<ApiResponse<{ deleted: boolean }>>(
    `${BASE}/backtests/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  );
  if (!json.success) throw new Error(json.error ?? 'unknown error');
}

export async function compareBacktests(ids: string[]): Promise<CompareResponse> {
  const json = await requestJson<ApiResponse<CompareResponse>>(
    `${BASE}/backtests/compare`,
    jsonInit('POST', { ids }),
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

/** 对比 CSV 下载地址（2..=10 个运行 ID）。 */
export function compareCsvUrl(ids: string[]): string {
  return `${BASE}/backtests/compare.csv?ids=${ids.map(encodeURIComponent).join(',')}`;
}

// ---- 本地状态：命名布局 / 绘图 / 偏好 ----

/** 布局摘要（列表项，不含 payload）。 */
export interface LayoutSummary {
  id: string;
  name: string;
  market_type: string;
  pair: string;
  timeframe: string;
  created_at: number;
  updated_at: number;
}

/** 完整布局。 */
export interface Layout extends LayoutSummary {
  payload: unknown;
}

/** 创建/更新布局的输入。 */
export interface LayoutInput {
  name: string;
  market_type: string;
  pair: string;
  timeframe: string;
  payload: unknown;
}

/** lightweight-charts-drawing 导出的单个绘图对象。 */
export type DrawingRecord = Record<string, unknown>;
/** 某交易对的完整绘图数组。 */
export type DrawingDocument = DrawingRecord[];

export async function fetchLayouts(): Promise<LayoutSummary[]> {
  const json = await fetchJson<ApiResponse<{ layouts: LayoutSummary[] }>>(`${BASE}/layouts`);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.layouts;
}

export async function fetchLayout(id: string): Promise<Layout> {
  const json = await fetchJson<ApiResponse<Layout>>(`${BASE}/layouts/${encodeURIComponent(id)}`);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function createLayout(input: LayoutInput): Promise<Layout> {
  const json = await requestJson<ApiResponse<Layout>>(`${BASE}/layouts`, jsonInit('POST', input));
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function updateLayout(id: string, input: LayoutInput): Promise<Layout> {
  const json = await requestJson<ApiResponse<Layout>>(
    `${BASE}/layouts/${encodeURIComponent(id)}`,
    jsonInit('PUT', input),
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data;
}

export async function deleteLayout(id: string): Promise<void> {
  const json = await requestJson<ApiResponse<{ deleted: boolean }>>(
    `${BASE}/layouts/${encodeURIComponent(id)}`,
    { method: 'DELETE' },
  );
  if (!json.success) throw new Error(json.error ?? 'unknown error');
}

export async function fetchDrawings(marketType: string, pair: string): Promise<DrawingDocument> {
  const url = `${BASE}/drawings?market_type=${encodeURIComponent(marketType)}&pair=${encodeURIComponent(pair)}`;
  const json = await fetchJson<ApiResponse<{ drawings: DrawingDocument }>>(url);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.drawings;
}

export async function replaceDrawings(
  marketType: string,
  pair: string,
  drawings: DrawingDocument,
): Promise<DrawingDocument> {
  const url = `${BASE}/drawings?market_type=${encodeURIComponent(marketType)}&pair=${encodeURIComponent(pair)}`;
  const json = await requestJson<ApiResponse<{ drawings: DrawingDocument }>>(
    url,
    jsonInit('PUT', { drawings }),
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.drawings;
}

export async function fetchPreferences(): Promise<Record<string, unknown>> {
  const json = await fetchJson<ApiResponse<{ preferences: Record<string, unknown> }>>(`${BASE}/preferences`);
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.preferences;
}

export async function putPreferences(patch: Record<string, unknown>): Promise<Record<string, unknown>> {
  const json = await requestJson<ApiResponse<{ preferences: Record<string, unknown> }>>(
    `${BASE}/preferences`,
    jsonInit('PUT', patch),
  );
  if (!json.success || !json.data) throw new Error(json.error ?? 'unknown error');
  return json.data.preferences;
}
