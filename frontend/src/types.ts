export interface ApiResponse<T> {
  success: boolean;
  data: T | null;
  error: string | null;
}

export interface MarketTypesData {
  types: string[];
}

export interface PairsData {
  market_type: string;
  pairs: string[];
}

export interface TimeframesData {
  market_type: string;
  pair: string;
  timeframes: string[];
}

export interface Kline {
  time: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

export interface KlinesData {
  market_type: string;
  pair: string;
  timeframe: string;
  count: number;
  klines: Kline[];
}

export interface QualityGap {
  after: number;
  before: number;
  missing_bars: number;
}

export interface QualityReport {
  market_type: string;
  pair: string;
  timeframe: string;
  bar_count: number;
  first_time: number | null;
  last_time: number | null;
  expected_bars: number;
  missing_bars: number;
  coverage: number;
  gap_count: number;
  longest_gap_bars: number;
  gaps: QualityGap[];
  gaps_truncated: boolean;
  zero_volume_bars: number;
  negative_volume_bars: number;
  bad_ohlc_bars: number;
  non_positive_price_bars: number;
  duplicate_rows: number;
  non_finite_rows: number;
}

/** 回测运行状态。 */
export type RunStatus = 'queued' | 'running' | 'succeeded' | 'failed';

export interface DualMaParams {
  fast: number;
  slow: number;
}

/** 参数扫描轴；缺省轴使用基础参数。 */
export interface ParamGrid {
  fast?: number[] | null;
  slow?: number[] | null;
}

export interface CreateBacktestRequest {
  market_type?: string;
  pair: string;
  timeframe: string;
  strategy?: string;
  direction?: string;
  params?: DualMaParams;
  param_grid?: ParamGrid;
  fee_bps?: number;
  slippage_bps?: number;
  initial_balance?: number;
  start_ms?: number;
  end_ms?: number;
}

export interface BacktestMetrics {
  num_trades: number;
  win_rate: number | null;
  total_return_pct: number | null;
  sharpe_ratio: number | null;
  sortino_ratio: number | null;
  max_drawdown_pct: number | null;
  avg_drawdown_pct: number | null;
  drawdown_count: number;
  longest_drawdown_days: number | null;
  longest_recovery_days: number | null;
  drawdown_time_pct: number | null;
  profit_factor: number | null;
  expectancy: number | null;
  final_equity: number | null;
  bars: number;
  periods_per_year: number;
}

export interface BacktestRunSummary {
  id: string;
  batch_id: string;
  status: RunStatus;
  market_type: string;
  pair: string;
  timeframe: string;
  strategy: string;
  strategy_version: string;
  params: DualMaParams;
  params_text: string;
  fee_bps: number;
  slippage_bps: number;
  initial_balance: number;
  start_ms: number | null;
  end_ms: number | null;
  created_at: number;
  started_at: number | null;
  finished_at: number | null;
  error: string | null;
  log_path: string | null;
  bars: number | null;
  data_first_ms: number | null;
  data_last_ms: number | null;
  engine: string | null;
  worker_version: string | null;
  metrics: BacktestMetrics | null;
  trades_truncated: boolean;
  retention: string;
}

export interface BacktestTrade {
  seq: number;
  entry_time: number;
  exit_time: number;
  side: string;
  status: string;
  entry_price: number | null;
  exit_price: number | null;
  qty: number | null;
  pnl: number | null;
  pnl_pct: number | null;
  bars_held: number;
}

export interface DrawdownEvent {
  seq: number;
  peak_time: number;
  trough_time: number;
  recovery_time: number | null;
  depth_pct: number;
  duration_ms: number;
  recovery_ms: number | null;
  recovered: boolean;
}

export interface EquityPoint {
  time: number;
  value: number;
}

export interface UnderwaterPoint {
  time: number;
  drawdown_pct: number;
}

export interface StrategyMarker {
  time: number;
  side: string;
  price: number | null;
  kind: string;
}

export interface BacktestRunDetail {
  run: BacktestRunSummary;
  trades: BacktestTrade[];
  drawdown_events: DrawdownEvent[];
  equity: EquityPoint[];
  underwater: UnderwaterPoint[];
  markers: StrategyMarker[];
}

export interface SubmitOutcome {
  batch_id: string;
  runs: BacktestRunSummary[];
}

export interface CompareWarning {
  kind: string;
  message: string;
  run_ids: string[];
}

export interface AlignedPoint {
  time: number;
  drawdown_pct: number | null;
}

export interface CompareSeries {
  run_id: string;
  label: string;
  params_text: string;
  points: AlignedPoint[];
  missing_points: number;
}

export interface CompareResponse {
  runs: BacktestRunSummary[];
  warnings: CompareWarning[];
  series: CompareSeries[];
  union_points: number;
  max_compare_runs: number;
}

export interface BacktestConfigResponse {
  enabled: boolean;
  market_type?: string;
  strategy?: string;
  strategy_version?: string;
  direction?: string;
  timeframes?: string[];
  defaults?: {
    fast: number;
    slow: number;
    fee_bps: number;
    slippage_bps: number;
    initial_balance: number;
  };
  limits?: {
    max_compare_runs: number;
    max_grid_runs: number;
    max_curve_points: number;
    max_trades: number;
    keep_equity: boolean;
  };
}
