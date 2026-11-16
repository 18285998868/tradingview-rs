import {
  compareBacktests,
  compareCsvUrl,
  deleteBacktest,
  fetchBacktest,
  fetchBacktestConfig,
  fetchBacktestLog,
  fetchBacktests,
  retryBacktest,
  submitBacktest,
} from './api';
import type {
  BacktestConfigResponse,
  BacktestMetrics,
  BacktestRunDetail,
  BacktestRunSummary,
  CompareResponse,
  RunStatus,
} from './types';

export interface BacktestContext {
  marketType: string;
  pair: string;
  timeframe: string;
}

export interface BacktestPanelHost {
  currentContext(): BacktestContext | null;
  showMarkers(detail: BacktestRunDetail | null): void;
  notify(message: string): void;
}

const POLL_MS = 2500;
const MAX_TRADES_SHOWN = 50;
const COLORS = [
  '#89b4fa',
  '#a6e3a1',
  '#f9e2af',
  '#f38ba8',
  '#cba6f7',
  '#94e2d5',
  '#fab387',
  '#b4befe',
  '#f5c2e7',
  '#74c7ec',
];

const STATUS_LABEL: Record<RunStatus, string> = {
  queued: '排队中',
  running: '运行中',
  succeeded: '成功',
  failed: '失败',
};

function esc(value: unknown): string {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function num(value: number | null | undefined, digits = 2): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return 'n/a';
  return value.toFixed(digits);
}

function pct(value: number | null | undefined, digits = 2): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return 'n/a';
  return `${value >= 0 ? '+' : ''}${value.toFixed(digits)}%`;
}

function money(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value)) return 'n/a';
  return value.toLocaleString('zh-CN', { maximumFractionDigits: 2 });
}

function dateText(ms: number): string {
  return new Date(ms).toLocaleString('zh-CN', { hour12: false });
}

function dayText(ms: number | null): string {
  if (ms === null) return '未恢复';
  return new Date(ms).toLocaleString('zh-CN', { hour12: false });
}

function readNumber(input: HTMLInputElement, fallback: number): number {
  const value = Number.parseFloat(input.value);
  return Number.isFinite(value) ? value : fallback;
}

function readList(input: HTMLInputElement, fallback: number): number[] {
  const raw = input.value.trim();
  if (!raw) return [fallback];
  const values = raw
    .split(/[\s,]+/)
    .filter(Boolean)
    .map((part) => Number.parseInt(part, 10));
  if (values.some((value) => !Number.isFinite(value) || value <= 0)) {
    throw new Error(`参数列表包含非法值: ${raw}`);
  }
  return [...new Set(values)];
}

interface CurvePoint {
  time: number;
  value: number | null;
}

interface CurveSeries {
  key: string;
  label: string;
  color: string;
  points: CurvePoint[];
}

function renderLineSvg(series: CurveSeries[]): string {
  const visible = series.filter((item) => item.points.some((p) => p.value !== null));
  const all = visible.flatMap((item) => item.points);
  if (all.length === 0) return '<div class="bt-empty">无曲线数据</div>';
  const times = all.map((point) => point.time);
  const values = all
    .map((point) => point.value)
    .filter((value): value is number => value !== null && Number.isFinite(value));
  if (values.length === 0) return '<div class="bt-empty">无曲线数据</div>';
  const t0 = Math.min(...times);
  const t1 = Math.max(...times);
  const vmin = Math.min(...values);
  const vmax = Math.max(...values);
  const spanT = t1 - t0 || 1;
  const spanV = vmax - vmin || 1;
  const width = 640;
  const height = 150;
  const pad = 8;
  const x = (time: number) => pad + ((time - t0) / spanT) * (width - 2 * pad);
  const y = (value: number) =>
    pad + ((vmax - value) / spanV) * (height - 2 * pad);
  const paths = visible
    .map((item) => {
      let path = '';
      let pen = false;
      for (const point of item.points) {
        if (point.value === null || !Number.isFinite(point.value)) {
          pen = false;
          continue;
        }
        path += `${pen ? 'L' : 'M'}${x(point.time).toFixed(1)},${y(point.value).toFixed(1)}`;
        pen = true;
      }
      return `<path d="${path}" fill="none" stroke="${item.color}" stroke-width="1.4" />`;
    })
    .join('');
  return (
    `<svg class="bt-svg" viewBox="0 0 ${width} ${height}" preserveAspectRatio="none">${paths}</svg>` +
    `<div class="bt-axis"><span>${esc(dateText(t0))}</span><span>${esc(dateText(t1))}</span></div>`
  );
}

function metricsRows(metrics: BacktestMetrics | null): [string, string][] {
  if (!metrics) return [['指标', '暂无']];
  return [
    ['交易笔数', `${metrics.num_trades}`],
    ['胜率', pct(metrics.win_rate, 1)],
    ['总收益', pct(metrics.total_return_pct)],
    ['最大回撤', pct(metrics.max_drawdown_pct)],
    ['平均回撤', pct(metrics.avg_drawdown_pct)],
    ['回撤次数', `${metrics.drawdown_count}`],
    ['最长回撤', `${num(metrics.longest_drawdown_days, 1)} 天`],
    ['最长恢复', `${num(metrics.longest_recovery_days, 1)} 天`],
    ['水下时间', pct(metrics.drawdown_time_pct, 1)],
    ['Sharpe', num(metrics.sharpe_ratio)],
    ['Sortino', num(metrics.sortino_ratio)],
    ['盈亏比', num(metrics.profit_factor)],
    ['期望', metrics.expectancy === null ? 'n/a' : `${(metrics.expectancy * 100).toFixed(3)}%`],
    ['期末权益', money(metrics.final_equity)],
    ['bar 数', `${metrics.bars}`],
  ];
}

export class BacktestPanel {
  private host: BacktestPanelHost;
  private panel: HTMLElement;
  private config: BacktestConfigResponse = { enabled: false };
  private runs: BacktestRunSummary[] = [];
  private detail: BacktestRunDetail | null = null;
  private compare: CompareResponse | null = null;
  private selected = new Set<string>();
  private hiddenSeries = new Set<string>();
  private busy = false;
  private statusEl: HTMLElement;
  private runsEl: HTMLElement;
  private detailSection: HTMLElement;
  private detailEl: HTMLElement;
  private compareSection: HTMLElement;
  private compareEl: HTMLElement;
  private contextEl: HTMLElement;
  private fastInput: HTMLInputElement;
  private slowInput: HTMLInputElement;
  private feeInput: HTMLInputElement;
  private slipInput: HTMLInputElement;
  private balanceInput: HTMLInputElement;
  private startInput: HTMLInputElement;
  private endInput: HTMLInputElement;
  private gridToggle: HTMLInputElement;
  private gridLists: HTMLElement;
  private fastList: HTMLInputElement;
  private slowList: HTMLInputElement;

  constructor(panel: HTMLElement, host: BacktestPanelHost) {
    this.host = host;
    this.panel = panel;
    panel.innerHTML = `
      <div class="bt-head">
        <span>回测 · Binance 现货 · 只做多</span>
        <button id="bt-close" class="time-jump-btn">✕</button>
      </div>
      <div class="bt-body">
        <section class="bt-section">
          <div id="bt-context" class="bt-context">选择交易对和周期</div>
          <div class="bt-grid">
            <label>快线 <input id="bt-fast" type="number" min="1" value="10" /></label>
            <label>慢线 <input id="bt-slow" type="number" min="2" value="30" /></label>
            <label>手续费 bps <input id="bt-fee" type="number" min="0" step="0.1" value="10" /></label>
            <label>滑点 bps <input id="bt-slip" type="number" min="0" step="0.1" value="5" /></label>
            <label>初始资金 <input id="bt-balance" type="number" min="1" value="10000" /></label>
            <label>开始日期 <input id="bt-start" type="date" /></label>
            <label>结束日期（含） <input id="bt-end" type="date" /></label>
          </div>
          <label class="bt-check"><input id="bt-grid-enabled" type="checkbox" /> 参数扫描</label>
          <div class="bt-grid bt-grid-lists" id="bt-grid-lists" hidden>
            <label>快线列表 <input id="bt-fast-list" placeholder="5,10,20" /></label>
            <label>慢线列表 <input id="bt-slow-list" placeholder="20,30,60" /></label>
          </div>
          <button id="bt-submit" class="bt-primary">提交回测</button>
          <div id="bt-status" class="bt-status"></div>
        </section>
        <section class="bt-section">
          <div class="bt-section-title"><span>运行记录</span><button id="bt-refresh">刷新</button></div>
          <div id="bt-runs" class="bt-runs"></div>
        </section>
        <section class="bt-section" id="bt-detail-section" hidden>
          <div class="bt-section-title"><span>运行详情</span><button id="bt-clear-markers">清除买卖点</button></div>
          <div id="bt-detail"></div>
        </section>
        <section class="bt-section" id="bt-compare-section" hidden>
          <div class="bt-section-title"><span>对比</span></div>
          <div id="bt-compare-root"></div>
        </section>
      </div>
    `;

    this.statusEl = panel.querySelector('#bt-status')!;
    this.runsEl = panel.querySelector('#bt-runs')!;
    this.detailSection = panel.querySelector('#bt-detail-section')!;
    this.detailEl = panel.querySelector('#bt-detail')!;
    this.compareSection = panel.querySelector('#bt-compare-section')!;
    this.compareEl = panel.querySelector('#bt-compare-root')!;
    this.contextEl = panel.querySelector('#bt-context')!;
    this.fastInput = panel.querySelector('#bt-fast')!;
    this.slowInput = panel.querySelector('#bt-slow')!;
    this.feeInput = panel.querySelector('#bt-fee')!;
    this.slipInput = panel.querySelector('#bt-slip')!;
    this.balanceInput = panel.querySelector('#bt-balance')!;
    this.startInput = panel.querySelector('#bt-start')!;
    this.endInput = panel.querySelector('#bt-end')!;
    this.gridToggle = panel.querySelector('#bt-grid-enabled')!;
    this.gridLists = panel.querySelector('#bt-grid-lists')!;
    this.fastList = panel.querySelector('#bt-fast-list')!;
    this.slowList = panel.querySelector('#bt-slow-list')!;

    panel.querySelector('#bt-close')!.addEventListener('click', () => this.close());
    panel.querySelector('#bt-refresh')!.addEventListener('click', () => void this.refreshRuns());
    panel.querySelector('#bt-clear-markers')!.addEventListener('click', () => {
      this.host.showMarkers(null);
      this.statusEl.textContent = '已清除图表买卖点';
    });
    this.gridToggle.addEventListener('change', () => {
      this.gridLists.hidden = !this.gridToggle.checked;
    });
    panel.querySelector('#bt-submit')!.addEventListener('click', () => void this.submit());

    void this.loadConfig();
    void this.refreshRuns();
    window.setInterval(() => {
      if (panel.classList.contains('visible') && !document.hidden) void this.refreshRuns();
    }, POLL_MS);
  }

  open(): void {
    this.panel.classList.add('visible');
    this.syncContext();
    void this.refreshRuns();
  }

  close(): void {
    this.panel.classList.remove('visible');
  }

  toggle(): void {
    if (this.panel.classList.contains('visible')) this.close();
    else this.open();
  }

  syncContext(): void {
    const context = this.host.currentContext();
    this.contextEl.textContent = context
      ? `${context.marketType.toUpperCase()} · ${context.pair} · ${context.timeframe}`
      : '选择交易对和周期后提交回测';
  }

  private async loadConfig(): Promise<void> {
    try {
      this.config = await fetchBacktestConfig();
      if (!this.config.enabled) {
        this.statusEl.textContent = '后端配置已关闭回测';
        return;
      }
      const defaults = this.config.defaults;
      if (defaults) {
        this.fastInput.value = String(defaults.fast);
        this.slowInput.value = String(defaults.slow);
        this.feeInput.value = String(defaults.fee_bps);
        this.slipInput.value = String(defaults.slippage_bps);
        this.balanceInput.value = String(defaults.initial_balance);
      }
      if (this.config.limits) {
        this.statusEl.textContent = `对比上限 ${this.config.limits.max_compare_runs} 个运行，参数扫描上限 ${this.config.limits.max_grid_runs} 组`;
      }
    } catch (error) {
      this.statusEl.textContent = `读取回测配置失败: ${error}`;
    }
  }

  private async submit(): Promise<void> {
    if (this.busy) return;
    const context = this.host.currentContext();
    if (!context) {
      this.statusEl.textContent = '请先加载 K 线数据';
      return;
    }
    const fast = Math.trunc(readNumber(this.fastInput, 10));
    const slow = Math.trunc(readNumber(this.slowInput, 30));
    if (fast < 1 || slow < 2 || fast >= slow) {
      this.statusEl.textContent = '参数非法：需要 fast >= 1、slow >= 2、fast < slow';
      return;
    }
    let fastList: number[] | undefined;
    let slowList: number[] | undefined;
    try {
      if (this.gridToggle.checked) {
        fastList = readList(this.fastList, fast);
        slowList = readList(this.slowList, slow);
      }
    } catch (error) {
      this.statusEl.textContent = String(error);
      return;
    }
    const start = this.startInput.value ? Date.parse(this.startInput.value) : undefined;
    const endValue = this.endInput.value ? Date.parse(this.endInput.value) : undefined;
    const end = endValue === undefined ? undefined : endValue + 86_400_000;
    if (start !== undefined && end !== undefined && start >= end) {
      this.statusEl.textContent = '开始日期必须早于结束日期';
      return;
    }
    this.busy = true;
    this.statusEl.textContent = '提交中...';
    try {
      const outcome = await submitBacktest({
        market_type: context.marketType,
        pair: context.pair,
        timeframe: context.timeframe,
        params: { fast, slow },
        ...(fastList && slowList ? { param_grid: { fast: fastList, slow: slowList } } : {}),
        fee_bps: readNumber(this.feeInput, 10),
        slippage_bps: readNumber(this.slipInput, 5),
        initial_balance: readNumber(this.balanceInput, 10000),
        ...(start !== undefined ? { start_ms: start } : {}),
        ...(end !== undefined ? { end_ms: end } : {}),
      });
      this.statusEl.textContent = `已入队 ${outcome.runs.length} 个运行（批次 ${outcome.batch_id.slice(0, 8)}）`;
      this.host.notify(this.statusEl.textContent);
      await this.refreshRuns();
    } catch (error) {
      this.statusEl.textContent = `提交失败: ${error}`;
    } finally {
      this.busy = false;
    }
  }

  private async refreshRuns(): Promise<void> {
    try {
      this.runs = await fetchBacktests(100);
      this.renderRuns();
    } catch (error) {
      this.runsEl.innerHTML = `<div class="bt-empty">加载运行失败: ${esc(error)}</div>`;
    }
  }

  private renderRuns(): void {
    this.runsEl.innerHTML = '';
    if (this.runs.length === 0) {
      this.runsEl.innerHTML = '<div class="bt-empty">暂无运行</div>';
      return;
    }
    const maxCompare = this.config.limits?.max_compare_runs ?? 10;
    for (const run of this.runs) {
      const row = document.createElement('div');
      row.className = `bt-run ${run.status}`;

      const checkbox = document.createElement('input');
      checkbox.type = 'checkbox';
      checkbox.checked = this.selected.has(run.id);
      checkbox.title = '加入对比';
      checkbox.addEventListener('change', () => {
        if (checkbox.checked) {
          if (this.selected.size >= maxCompare) {
            checkbox.checked = false;
            this.statusEl.textContent = `最多选择 ${maxCompare} 个运行对比`;
            return;
          }
          this.selected.add(run.id);
        } else {
          this.selected.delete(run.id);
        }
        this.renderCompareActions();
      });

      const main = document.createElement('div');
      main.className = 'bt-run-main';
      const title = document.createElement('button');
      title.className = 'bt-run-title';
      title.textContent = `${run.params_text} · ${run.pair} ${run.timeframe}`;
      title.addEventListener('click', () => void this.showDetail(run.id));
      const meta = document.createElement('div');
      meta.className = 'bt-run-meta';
      const metric = run.metrics
        ? `收益 ${pct(run.metrics.total_return_pct)} · 回撤 ${pct(run.metrics.max_drawdown_pct)} · ${run.metrics.num_trades} 笔`
        : run.error ?? `${run.bars ?? 0} bars`;
      meta.textContent = `${STATUS_LABEL[run.status]} · ${metric}`;
      main.append(title, meta);

      const actions = document.createElement('div');
      actions.className = 'bt-run-actions';
      if (run.status === 'failed') {
        const logBtn = document.createElement('button');
        logBtn.textContent = '日志';
        logBtn.addEventListener('click', () => void this.showLog(run.id));
        actions.appendChild(logBtn);
      }
      const retryBtn = document.createElement('button');
      retryBtn.textContent = '重试';
      retryBtn.addEventListener('click', () => void this.retry(run.id));
      const deleteBtn = document.createElement('button');
      deleteBtn.textContent = '删除';
      deleteBtn.addEventListener('click', () => void this.remove(run.id));
      actions.append(retryBtn, deleteBtn);

      row.append(checkbox, main, actions);
      this.runsEl.appendChild(row);
    }
    this.renderCompareActions();
  }

  private renderCompareActions(): void {
    this.compareSection.hidden = false;
    const count = this.selected.size;
    const disabled = count < 2 ? 'disabled' : '';
    const csvDisabled = count < 2 ? 'disabled' : '';
    this.compareEl.innerHTML = `
      <div class="bt-compare-actions">
        <button id="bt-compare" class="bt-primary" ${disabled}>对比已选 ${count} 个</button>
        <button id="bt-csv" ${csvDisabled}>导出 CSV</button>
      </div>
      <div id="bt-compare-body">${this.compare ? '' : '<div class="bt-empty">勾选 2-10 个运行后对比水下曲线与回撤指标</div>'}</div>
    `;
    this.compareEl.querySelector('#bt-compare')?.addEventListener('click', () => void this.runCompare());
    this.compareEl.querySelector('#bt-csv')?.addEventListener('click', () => {
      const ids = [...this.selected];
      if (ids.length >= 2) window.open(compareCsvUrl(ids), '_blank');
    });
    if (this.compare) this.renderCompare();
  }

  private async runCompare(): Promise<void> {
    const ids = [...this.selected];
    if (ids.length < 2) {
      this.statusEl.textContent = '至少选择 2 个运行';
      return;
    }
    this.statusEl.textContent = '对比计算中...';
    try {
      this.compare = await compareBacktests(ids);
      this.hiddenSeries.clear();
      this.renderCompare();
      this.statusEl.textContent = `已对比 ${this.compare.series.length} 个运行`;
    } catch (error) {
      this.statusEl.textContent = `对比失败: ${error}`;
    }
  }

  private renderCompare(): void {
    const compare = this.compare;
    if (!compare) return;
    const body = this.compareEl.querySelector('#bt-compare-body');
    if (!body) return;
    const series: CurveSeries[] = compare.series
      .map((item, index) => ({
        key: item.run_id,
        label: item.label,
        color: COLORS[index % COLORS.length],
        points: item.points.map((point) => ({ time: point.time, value: point.drawdown_pct })),
      }))
      .filter((item) => !this.hiddenSeries.has(item.key));
    const warnings = compare.warnings.length
      ? `<ul class="bt-warnings">${compare.warnings
          .map((warning) => `<li>${esc(warning.message)}</li>`)
          .join('')}</ul>`
      : '<div class="bt-ok">无元数据差异警告</div>';
    const legend = compare.series
      .map((item, index) => {
        const color = COLORS[index % COLORS.length];
        const checked = this.hiddenSeries.has(item.run_id) ? '' : 'checked';
        return `<label class="bt-legend-item"><input type="checkbox" data-key="${esc(item.run_id)}" ${checked} /><span class="bt-swatch" style="background:${color}"></span>${esc(item.label)}</label>`;
      })
      .join('');
    const tableRows = compare.series
      .map((item, index) => {
        const run = compare.runs[index];
        const m = run.metrics;
        return `<tr>
          <td><span class="bt-swatch" style="background:${COLORS[index % COLORS.length]}"></span>${esc(item.label)}</td>
          <td>${esc(item.params_text)}</td>
          <td>${m ? m.num_trades : 'n/a'}</td>
          <td>${m ? pct(m.win_rate, 1) : 'n/a'}</td>
          <td>${m ? pct(m.total_return_pct) : 'n/a'}</td>
          <td>${m ? num(m.sharpe_ratio) : 'n/a'}</td>
          <td>${m ? pct(m.max_drawdown_pct) : 'n/a'}</td>
          <td>${m ? m.drawdown_count : 'n/a'}</td>
          <td>${item.missing_points}</td>
        </tr>`;
      })
      .join('');
    body.innerHTML = `
      ${warnings}
      <div class="bt-legend">${legend}</div>
      ${renderLineSvg(series)}
      <div class="bt-table-wrap">
        <table class="bt-table">
          <thead><tr><th>运行</th><th>参数</th><th>笔数</th><th>胜率</th><th>收益</th><th>Sharpe</th><th>最大回撤</th><th>回撤数</th><th>缺失点</th></tr></thead>
          <tbody>${tableRows}</tbody>
        </table>
      </div>
    `;
    body.querySelectorAll<HTMLInputElement>('input[data-key]').forEach((input) => {
      input.addEventListener('change', () => {
        const key = input.dataset.key!;
        if (input.checked) this.hiddenSeries.delete(key);
        else this.hiddenSeries.add(key);
        this.renderCompare();
      });
    });
  }

  private async showDetail(id: string): Promise<void> {
    this.statusEl.textContent = '加载运行详情...';
    try {
      const detail = await fetchBacktest(id);
      this.detail = detail;
      this.host.showMarkers(detail);
      this.renderDetail();
      this.detailSection.hidden = false;
      this.detailSection.scrollIntoView({ behavior: 'smooth', block: 'start' });
      this.statusEl.textContent = '详情已加载，买卖点已叠加到主图';
    } catch (error) {
      this.statusEl.textContent = `加载详情失败: ${error}`;
    }
  }

  private renderDetail(): void {
    const detail = this.detail;
    if (!detail) return;
    const run = detail.run;
    const metricsHtml = metricsRows(run.metrics)
      .map(([key, value]) => `<div class="bt-metric"><span>${esc(key)}</span><span>${esc(value)}</span></div>`)
      .join('');
    const equityCurve = detail.equity.map((point) => ({ time: point.time, value: point.value }));
    const underwaterCurve = detail.underwater.map((point) => ({
      time: point.time,
      value: point.drawdown_pct,
    }));
    const events = detail.drawdown_events
      .map(
        (event) => `<tr>
          <td>${esc(dayText(event.peak_time))}</td>
          <td>${esc(dayText(event.trough_time))}</td>
          <td>${esc(dayText(event.recovery_time))}</td>
          <td>${pct(-event.depth_pct)}</td>
          <td>${(event.duration_ms / 86_400_000).toFixed(2)}</td>
          <td>${event.recovered ? '已恢复' : '未恢复'}</td>
        </tr>`,
      )
      .join('');
    const trades = detail.trades
      .slice(0, MAX_TRADES_SHOWN)
      .map(
        (trade) => `<tr>
          <td>${esc(dayText(trade.entry_time))}</td>
          <td>${esc(dayText(trade.exit_time))}</td>
          <td>${trade.entry_price === null ? 'n/a' : trade.entry_price.toFixed(2)}</td>
          <td>${trade.exit_price === null ? 'n/a' : trade.exit_price.toFixed(2)}</td>
          <td>${trade.pnl === null ? 'n/a' : trade.pnl.toFixed(2)}</td>
          <td>${trade.pnl_pct === null ? 'n/a' : pct(trade.pnl_pct)}</td>
          <td>${trade.bars_held}</td>
        </tr>`,
      )
      .join('');
    this.detailEl.innerHTML = `
      <div class="bt-detail-head">${esc(run.pair)} ${esc(run.timeframe)} · ${esc(run.params_text)} · ${esc(STATUS_LABEL[run.status])}</div>
      <div class="bt-metrics">${metricsHtml}</div>
      ${run.error ? `<div class="bt-error">${esc(run.error)}</div>` : ''}
      <div class="bt-subtitle">权益曲线（降采样）</div>
      ${renderLineSvg([{ key: 'equity', label: '权益', color: COLORS[0], points: equityCurve }])}
      <div class="bt-subtitle">水下曲线</div>
      ${renderLineSvg([{ key: 'underwater', label: '回撤', color: COLORS[3], points: underwaterCurve }])}
      <div class="bt-subtitle">回撤事件（${detail.drawdown_events.length}）</div>
      <div class="bt-table-wrap">
        <table class="bt-table">
          <thead><tr><th>峰值</th><th>谷底</th><th>恢复</th><th>深度</th><th>持续(天)</th><th>状态</th></tr></thead>
          <tbody>${events || '<tr><td colspan="6">无回撤事件</td></tr>'}</tbody>
        </table>
      </div>
      <div class="bt-subtitle">交易明细（${detail.trades.length}${run.trades_truncated ? '，已截断' : ''}）</div>
      <div class="bt-table-wrap">
        <table class="bt-table">
          <thead><tr><th>入场</th><th>出场</th><th>入场价</th><th>出场价</th><th>盈亏</th><th>收益率</th><th>bars</th></tr></thead>
          <tbody>${trades || '<tr><td colspan="7">无交易</td></tr>'}</tbody>
        </table>
      </div>
    `;
  }

  private async showLog(id: string): Promise<void> {
    try {
      const log = await fetchBacktestLog(id);
      this.statusEl.textContent = log ? '日志已输出到控制台' : '该运行没有日志';
      if (log) console.info(`backtest ${id} log:\n${log}`);
      this.detailSection.hidden = false;
      this.detailEl.innerHTML = `<pre class="bt-log">${esc(log ?? '（无日志）')}</pre>`;
    } catch (error) {
      this.statusEl.textContent = `读取日志失败: ${error}`;
    }
  }

  private async retry(id: string): Promise<void> {
    try {
      const outcome = await retryBacktest(id);
      this.statusEl.textContent = `已重试，新运行 ${outcome.runs[0]?.id.slice(0, 8) ?? ''}`;
      await this.refreshRuns();
    } catch (error) {
      this.statusEl.textContent = `重试失败: ${error}`;
    }
  }

  private async remove(id: string): Promise<void> {
    if (!confirm('删除该回测运行及其交易/回撤数据？')) return;
    try {
      await deleteBacktest(id);
      this.selected.delete(id);
      if (this.detail?.run.id === id) {
        this.detail = null;
        this.detailSection.hidden = true;
        this.host.showMarkers(null);
      }
      this.statusEl.textContent = '运行已删除';
      await this.refreshRuns();
    } catch (error) {
      this.statusEl.textContent = `删除失败: ${error}`;
    }
  }
}
