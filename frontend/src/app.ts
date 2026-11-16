import {
  createLayout,
  deleteLayout,
  fetchDrawings,
  fetchKlines,
  fetchLayout,
  fetchLayouts,
  fetchMarketTypes,
  fetchPairs,
  fetchPreferences,
  fetchQuality,
  fetchTimeframes,
  putPreferences,
  replaceDrawings,
  updateLayout,
  type DrawingDocument,
  type LayoutInput,
} from './api';
import {
  createChartManager,
  type ChartManager,
  type DrawingTool,
  type LoadDirection,
  type RangeSelection,
  type Viewport,
} from './chart';
import { findIndicator, getAllIndicators, type ActiveIndicator, type IndicatorDef } from './indicators';
import { qualityBadge } from './quality';
import type { Kline } from './types';
import { computeRangeStats, mergeKlines, timeframeMs, windowForViewport } from './viewport';
import { BacktestPanel } from './backtest';

/** 边缘补加载的单页根数。 */
const PAGE_SIZE = 500;
/** 首屏加载根数：可视 500 根 + 左侧缓冲 1000 根（最新端无未来数据）。 */
const INITIAL_LOAD_BARS = 1500;
/** 同区间切换周期时视窗两侧各保留的根数。 */
const PREFETCH_BARS = 500;
/** 单次周期切换最多拉取的根数（防御 1d 视窗切 1m 的极端放大）。 */
const MAX_WINDOW_BARS = 100_000;
/** 后端 `/api/klines` 单页上限，与 `MAX_KLINE_LIMIT` 保持一致。 */
const MAX_API_PAGE = 10_000;
const REPLAY_LIMIT = 2000;
const PAIR_DROPDOWN_LIMIT = 100;
const APP_VERSION = 'p7';
const SESSION_SAVE_DEBOUNCE_MS = 800;
const DRAWING_SAVE_DEBOUNCE_MS = 600;

/** 成交量紧凑格式：1.23K / 4.56M / 7.89B。 */
function formatCompact(value: number): string {
  const abs = Math.abs(value);
  if (abs >= 1e9) return `${(value / 1e9).toFixed(2)}B`;
  if (abs >= 1e6) return `${(value / 1e6).toFixed(2)}M`;
  if (abs >= 1e3) return `${(value / 1e3).toFixed(2)}K`;
  return value.toFixed(2);
}

interface IndicatorState {
  id: string;
  params: Record<string, number>;
}

/** 回放只维护游标；完整 K 线仍在 allKlines，图表只拿 [0..index] 前缀。 */
interface ReplayState {
  startIndex: number;
  index: number;
  playing: boolean;
  speed: number;
  timer: number | null;
}

/** 可持久化的会话快照（last_session / 命名布局共用）。 */
interface SessionState {
  market_type: string;
  pair: string;
  timeframe: string;
  indicators: IndicatorState[];
  viewport: Viewport | null;
}

function isSessionState(value: unknown): value is SessionState {
  if (!value || typeof value !== 'object') return false;
  const v = value as Record<string, unknown>;
  return (
    typeof v.market_type === 'string' &&
    typeof v.pair === 'string' &&
    typeof v.timeframe === 'string' &&
    Array.isArray(v.indicators)
  );
}

export class App {
  private chart: ChartManager | null = null;
  private loading = false;
  private allKlines: Kline[] = [];
  private replay: ReplayState | null = null;
  private preReplayViewport: Viewport | null = null;
  /** 切周期/恢复布局时待应用的可视区间；首屏加载消费一次。 */
  private pendingViewport: Viewport | null = null;
  /** 边缘已确认没有更多数据，避免重复空请求。 */
  private exhausted: Record<LoadDirection, boolean> = { left: false, right: false };
  /** 全量加载序号：切周期时旧请求返回后直接丢弃。 */
  private loadSeq = 0;
  private activeIndicators: ActiveIndicator[] = [];
  /** 已渲染的指标条 id 顺序；回放每帧都会刷新指标，但 DOM 只在增删时重建。 */
  private renderedIndicators = '';
  private allPairs: string[] = [];
  private backtestPanel: BacktestPanel | null = null;
  private marketSelect: HTMLSelectElement;
  private pairSearch: HTMLInputElement;
  private pairDropdown: HTMLElement;
  private timeframeSelect: HTMLSelectElement;
  private infoEl: HTMLElement;
  private rangeInfoEl: HTMLElement;
  private qualityBadgeEl: HTMLElement;
  private qualitySeq = 0;
  private chartContainer: HTMLElement;
  private indicatorsEl: HTMLElement;
  private layoutSelect: HTMLSelectElement;
  private drawingToolSelect: HTMLSelectElement;
  private replayEnterBtn: HTMLButtonElement;
  private replayControls: HTMLElement;
  private replayPlayBtn: HTMLButtonElement;
  private replayStepBtn: HTMLButtonElement;
  private replayResetBtn: HTMLButtonElement;
  private replayExitBtn: HTMLButtonElement;
  private replaySpeedSelect: HTMLSelectElement;
  private replayPositionEl: HTMLElement;
  private _pair = '';
  private _timeframe = '';
  /** 恢复/加载布局期间抑制自动保存。 */
  private suppressSave = false;
  /** 初始化完成前不保存（避免把半初始化状态写成 last_session）。 */
  private ready = false;
  private saveTimer: number | null = null;
  private drawingSaveTimer: number | null = null;
  /** 当前 chart 的绘图已从服务端加载完成；未完成前禁止回写，避免用空画布覆盖。 */
  private drawingsLoaded = false;
  private drawingLoadToken = 0;
  private unbindViewport: (() => void) | null = null;
  private unbindDrawings: (() => void) | null = null;
  private unbindRange: (() => void) | null = null;
  /** 当前区间选择；统计计算按动画帧合并。 */
  private rangeSelection: RangeSelection | null = null;
  private rangeFrame: number | null = null;

  constructor(container: HTMLElement) {
    container.innerHTML = `
      <div class="toolbar">
        <label>市场</label>
        <select id="market-select"></select>
        <label>交易对</label>
        <div class="pair-search-wrap">
          <input id="pair-search" type="text" placeholder="搜索交易对..." autocomplete="off" disabled />
          <div id="pair-dropdown" class="pair-dropdown"></div>
        </div>
        <label>周期</label>
        <select id="timeframe-select" disabled><option>—</option></select>
        <label>布局</label>
        <select id="layout-select"><option value="">— 布局 —</option></select>
        <button id="layout-save" class="time-jump-btn">保存</button>
        <button id="layout-delete" class="time-jump-btn">删除</button>
        <input type="date" id="time-jump" class="time-jump" />
        <button id="time-jump-btn" class="time-jump-btn">跳转</button>
        <div class="replay-controls" id="replay-controls">
          <label>回放</label>
          <button id="replay-play" class="time-jump-btn">▶ 播放</button>
          <button id="replay-step" class="time-jump-btn">步进</button>
          <select id="replay-speed" title="回放速度">
            <option value="0.5">0.5×</option>
            <option value="1" selected>1×</option>
            <option value="2">2×</option>
            <option value="4">4×</option>
            <option value="8">8×</option>
          </select>
          <button id="replay-reset" class="time-jump-btn">重置</button>
          <button id="replay-exit" class="time-jump-btn">退出</button>
          <span id="replay-position" class="replay-position"></span>
        </div>
        <button id="replay-enter" class="time-jump-btn">回放</button>
        <button id="backtest-toggle" class="time-jump-btn">回测</button>
        <div class="indicator-search-wrap">
          <input id="indicator-search" type="text" placeholder="搜索指标..." autocomplete="off" />
          <div id="indicator-dropdown" class="indicator-dropdown"></div>
        </div>
        <div id="active-indicators" class="active-indicators"></div>
        <label>绘图</label>
        <select id="drawing-tool">
          <option value="">光标</option>
          <option value="trend-line">趋势线</option>
          <option value="horizontal-line">水平线</option>
          <option value="vertical-line">垂直线</option>
          <option value="rectangle">矩形</option>
          <option value="fib-retracement">斐波那契</option>
          <option value="date-range">区间选择</option>
          <option value="text">文字</option>
          <option value="arrow">箭头</option>
        </select>
        <button id="drawing-undo" class="time-jump-btn">撤销</button>
        <button id="drawing-redo" class="time-jump-btn">重做</button>
        <button id="drawing-clear" class="time-jump-btn">清空</button>
        <span class="info" id="info"></span>
        <span class="range-info" id="range-info" hidden></span>
        <span class="quality-badge" id="quality-badge" hidden></span>
        <span class="app-version" id="app-version"></span>
      </div>
      <div class="chart-container" id="chart-container">
        <div class="loading">选择交易对和周期</div>
      </div>
      <div id="backtest-panel" class="backtest-panel"></div>
    `;

    this.marketSelect = container.querySelector('#market-select')!;
    this.pairSearch = container.querySelector('#pair-search')!;
    this.pairDropdown = container.querySelector('#pair-dropdown')!;
    this.timeframeSelect = container.querySelector('#timeframe-select')!;
    this.infoEl = container.querySelector('#info')!;
    this.rangeInfoEl = container.querySelector('#range-info') as HTMLElement;
    this.qualityBadgeEl = container.querySelector('#quality-badge')!;
    this.chartContainer = container.querySelector('#chart-container')!;
    this.indicatorsEl = container.querySelector('#active-indicators') as HTMLElement;
    this.layoutSelect = container.querySelector('#layout-select') as HTMLSelectElement;
    this.drawingToolSelect = container.querySelector('#drawing-tool') as HTMLSelectElement;
    this.replayEnterBtn = container.querySelector('#replay-enter') as HTMLButtonElement;
    this.replayControls = container.querySelector('#replay-controls') as HTMLElement;
    this.replayPlayBtn = container.querySelector('#replay-play') as HTMLButtonElement;
    this.replayStepBtn = container.querySelector('#replay-step') as HTMLButtonElement;
    this.replayResetBtn = container.querySelector('#replay-reset') as HTMLButtonElement;
    this.replayExitBtn = container.querySelector('#replay-exit') as HTMLButtonElement;
    this.replaySpeedSelect = container.querySelector('#replay-speed') as HTMLSelectElement;
    this.replayPositionEl = container.querySelector('#replay-position') as HTMLElement;

    const searchInput = container.querySelector('#indicator-search') as HTMLInputElement;
    const dropdown = container.querySelector('#indicator-dropdown') as HTMLElement;
    const timeJump = container.querySelector('#time-jump') as HTMLInputElement;
    const timeJumpBtn = container.querySelector('#time-jump-btn') as HTMLButtonElement;
    const layoutSaveBtn = container.querySelector('#layout-save') as HTMLButtonElement;
    const layoutDeleteBtn = container.querySelector('#layout-delete') as HTMLButtonElement;
    const drawingUndoBtn = container.querySelector('#drawing-undo') as HTMLButtonElement;
    const drawingRedoBtn = container.querySelector('#drawing-redo') as HTMLButtonElement;
    const drawingClearBtn = container.querySelector('#drawing-clear') as HTMLButtonElement;

    (container.querySelector('#app-version') as HTMLElement).textContent = APP_VERSION;

    this.marketSelect.addEventListener('change', () => this.onMarketChange());
    this.timeframeSelect.addEventListener('change', () => this.onTimeframeChange());

    const backtestToggle = container.querySelector('#backtest-toggle') as HTMLButtonElement;
    this.backtestPanel = new BacktestPanel(
      container.querySelector('#backtest-panel') as HTMLElement,
      {
        currentContext: () => {
          if (!this._pair || !this._timeframe) return null;
          return {
            marketType: this.marketSelect.value,
            pair: this._pair,
            timeframe: this._timeframe,
          };
        },
        showMarkers: (detail) => {
          this.chart?.setBacktestMarkers(detail?.markers ?? []);
        },
        notify: (message) => {
          this.infoEl.textContent = message;
        },
      },
    );
    backtestToggle.addEventListener('click', () => this.backtestPanel?.toggle());

    this.layoutSelect.addEventListener('change', () => {
      const id = this.layoutSelect.value;
      if (id) void this.loadLayout(id);
    });
    layoutSaveBtn.addEventListener('click', () => void this.saveLayout());
    layoutDeleteBtn.addEventListener('click', () => void this.deleteSelectedLayout());

    this.drawingToolSelect.addEventListener('change', () => {
      const tool = this.drawingToolSelect.value;
      this.chart?.setDrawingTool(tool ? (tool as DrawingTool) : null);
    });
    drawingUndoBtn.addEventListener('click', () => this.chart?.undoDrawings());
    drawingRedoBtn.addEventListener('click', () => this.chart?.redoDrawings());
    drawingClearBtn.addEventListener('click', () => {
      if (!this.chart) return;
      if (!confirm('清空当前交易对的所有绘图？')) return;
      this.chart.clearDrawings();
    });

    const onJump = () => this.onTimeJump(timeJump);
    timeJump.addEventListener('change', onJump);
    timeJumpBtn.addEventListener('click', onJump);

    this.replayEnterBtn.addEventListener('click', () => void this.enterReplay());
    this.replayPlayBtn.addEventListener('click', () => this.toggleReplay());
    this.replayStepBtn.addEventListener('click', () => this.stepReplay());
    this.replayResetBtn.addEventListener('click', () => this.resetReplay());
    this.replayExitBtn.addEventListener('click', () => this.exitReplay());
    this.replaySpeedSelect.addEventListener('change', () => {
      this.setReplaySpeed(parseFloat(this.replaySpeedSelect.value));
    });

    this.pairSearch.addEventListener('input', () => this.onPairSearchInput());
    this.pairSearch.addEventListener('focus', () => this.onPairSearchInput());
    this.pairSearch.addEventListener('keydown', (e) => {
      if (e.key === 'Enter') {
        const first = this.matchPairs(this.pairSearch.value)[0];
        if (first) this.selectPair(first);
      } else if (e.key === 'Escape') {
        this.pairDropdown.classList.remove('visible');
      }
    });

    searchInput.addEventListener('input', () => this.onSearchInput(searchInput, dropdown));
    searchInput.addEventListener('focus', () => this.onSearchInput(searchInput, dropdown));
    document.addEventListener('click', (e) => {
      const t = e.target as HTMLElement;
      if (!t.closest('.indicator-search-wrap')) {
        dropdown.classList.remove('visible');
      }
      if (!t.closest('.pair-search-wrap')) {
        this.pairDropdown.classList.remove('visible');
      }
    });

    this.init();
  }

  private async init(): Promise<void> {
    try {
      const types = await fetchMarketTypes();
      for (const t of types) {
        const opt = document.createElement('option');
        opt.value = t;
        opt.textContent = t;
        this.marketSelect.appendChild(opt);
      }
      await this.refreshLayoutList();

      const preferences: Record<string, unknown> = await fetchPreferences().catch(() => ({}));
      const lastSession = preferences['last_session'];
      if (isSessionState(lastSession) && types.includes(lastSession.market_type)) {
        await this.applySession(lastSession);
      } else if (types.length > 0) {
        this.marketSelect.value = types[0];
        await this.loadPairs();
      }
    } catch (e) {
      this.infoEl.textContent = `加载失败: ${e}`;
    } finally {
      this.ready = true;
      this.scheduleSave();
    }
  }

  // ---- 会话 / 命名布局 ----

  private collectSessionState(): SessionState {
    return {
      market_type: this.marketSelect.value,
      pair: this._pair,
      timeframe: this._timeframe,
      indicators: this.activeIndicators.map((a) => ({
        id: a.def.id,
        params: { ...a.params },
      })),
      viewport: this.chart?.getViewport() ?? null,
    };
  }

  private scheduleSave(): void {
    if (this.suppressSave || !this.ready) return;
    if (this.saveTimer !== null) window.clearTimeout(this.saveTimer);
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = null;
      void this.saveSessionNow();
    }, SESSION_SAVE_DEBOUNCE_MS);
  }

  private async saveSessionNow(): Promise<void> {
    if (this.suppressSave || !this.ready || !this._pair || !this._timeframe) return;
    try {
      await putPreferences({ last_session: this.collectSessionState() });
    } catch (e) {
      console.warn('保存会话失败', e);
    }
  }

  private async flushPendingSaves(): Promise<void> {
    if (this.saveTimer !== null) {
      window.clearTimeout(this.saveTimer);
      this.saveTimer = null;
      await this.saveSessionNow();
    }
    await this.flushDrawings();
  }

  /**
   * 等两帧后再记录性能标记，确保图表绘制进入浏览器合成阶段。
   *
   * 只传 name 时写 `performance.mark`（首屏耗时）；同时传 start 时写
   * `performance.measure`（周期切换耗时）。
   */
  private markAfterPaint(name: string, start?: number): void {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        if (start === undefined) performance.mark(name);
        else performance.measure(name, { start, end: performance.now() });
      });
    });
  }

  // ---- 回放 ----

  private activeKlines(): Kline[] {
    if (!this.replay) return this.allKlines;
    return this.allKlines.slice(0, this.replay.index + 1);
  }

  private dataInfoText(): string {
    if (!this.replay) return `${this.allKlines.length} 条K线`;
    const r = this.replay;
    const bar = this.allKlines[r.index];
    const dt = bar
      ? new Date(bar.time).toLocaleString('zh-CN', { hour12: false })
      : '';
    const suffix = r.index >= this.allKlines.length - 1 ? ' · 已到末尾' : '';
    return `回放 ${r.index + 1}/${this.allKlines.length}${dt ? ` · ${dt}` : ''}${suffix}`;
  }

  /** 让当前徽标失效：递增序号丢弃飞行中的响应，并立即隐藏旧报告。 */
  private invalidateQualityBadge(): void {
    this.qualitySeq += 1;
    this.qualityBadgeEl.hidden = true;
    this.qualityBadgeEl.textContent = '';
    this.qualityBadgeEl.title = '';
  }

  /**
   * 拉取当前交易对/周期的数据质量报告。
   *
   * 后端按整段缓存统计，与图表当前只加载 200 根的窗口无关；切换交易对或
   * 周期时用序号丢弃过期响应，避免旧报告覆盖新标的。
   */
  private async loadQuality(marketType: string, pair: string, timeframe: string): Promise<void> {
    this.invalidateQualityBadge();
    const seq = this.qualitySeq;
    try {
      const report = await fetchQuality({ marketType, pair, timeframe });
      if (
        seq !== this.qualitySeq ||
        this.marketSelect.value !== marketType ||
        this._pair !== pair ||
        this._timeframe !== timeframe
      ) {
        return;
      }
      const view = qualityBadge(report);
      this.qualityBadgeEl.textContent = view.text;
      this.qualityBadgeEl.title = view.title;
      this.qualityBadgeEl.className = `quality-badge ${view.level}`;
      this.qualityBadgeEl.hidden = false;
    } catch {
      if (seq === this.qualitySeq) this.qualityBadgeEl.hidden = true;
    }
  }

  private updateReplayUi(): void {
    const active = this.replay !== null;
    this.replayControls.classList.toggle('visible', active);
    this.replayEnterBtn.disabled = active;
    if (!this.replay) {
      this.replayPositionEl.textContent = '';
      this.replayPlayBtn.textContent = '▶ 播放';
      return;
    }
    const r = this.replay;
    const atEnd = r.index >= this.allKlines.length - 1;
    this.replayPlayBtn.textContent = r.playing ? '⏸ 暂停' : '▶ 播放';
    this.replayPlayBtn.disabled = atEnd;
    this.replayStepBtn.disabled = atEnd;
    this.replaySpeedSelect.value = String(r.speed);
    this.replayPositionEl.textContent = `${r.index + 1} / ${this.allKlines.length}`;
  }

  private async enterReplay(): Promise<void> {
    if (this.replay || this.loading) return;
    if (!this.chart || !this._pair || !this._timeframe || this.allKlines.length === 0) {
      this.infoEl.textContent = '请先加载K线数据';
      return;
    }
    const chart = this.chart;
    const market = this.marketSelect.value;
    const pair = this._pair;
    const timeframe = this._timeframe;
    this.loading = true;
    this.infoEl.textContent = '加载回放数据...';
    try {
      const anchor = chart.getLastTime();
      const data = await fetchKlines({
        marketType: market,
        pair,
        timeframe,
        limit: REPLAY_LIMIT,
        ...(anchor !== null ? { before: anchor + 1 } : {}),
      });
      if (this.chart !== chart || this.marketSelect.value !== market || this._pair !== pair || this._timeframe !== timeframe) {
        return;
      }
      const full = data.klines;
      if (full.length < 2) {
        this.infoEl.textContent = '回放数据不足';
        return;
      }
      const viewport = chart.getViewport();
      let startIndex = 0;
      if (viewport) {
        const idx = full.findIndex((k) => k.time >= viewport.from);
        if (idx >= 0) startIndex = idx;
      }
      startIndex = Math.min(startIndex, full.length - 1);

      this.preReplayViewport = viewport;
      this.allKlines = full;
      this.replay = {
        startIndex,
        index: startIndex,
        playing: false,
        speed: 1,
        timer: null,
      };
      chart.setData(this.activeKlines());
      this.refreshIndicators();
      this.updateReplayUi();
      this.infoEl.textContent = this.dataInfoText();
      this.scheduleSave();
    } catch (e) {
      this.infoEl.textContent = `加载回放失败: ${e}`;
    } finally {
      this.loading = false;
    }
  }

  private toggleReplay(): void {
    if (!this.replay) return;
    if (this.replay.playing) this.pauseReplay();
    else this.playReplay();
  }

  private playReplay(): void {
    const r = this.replay;
    if (!r || r.playing) return;
    if (r.index >= this.allKlines.length - 1) {
      this.infoEl.textContent = '回放已到末尾';
      return;
    }
    r.playing = true;
    this.startReplayTimer();
    this.updateReplayUi();
    this.infoEl.textContent = this.dataInfoText();
  }

  private pauseReplay(): void {
    const r = this.replay;
    if (!r) return;
    r.playing = false;
    this.stopReplayTimer();
    this.updateReplayUi();
    this.infoEl.textContent = this.dataInfoText();
  }

  private stepReplay(): void {
    const r = this.replay;
    if (!r) return;
    if (r.playing) this.pauseReplay();
    if (r.index >= this.allKlines.length - 1) {
      this.infoEl.textContent = this.dataInfoText();
      return;
    }
    r.index++;
    this.renderReplayPrefix();
  }

  private resetReplay(): void {
    const r = this.replay;
    if (!r) return;
    r.playing = false;
    this.stopReplayTimer();
    r.index = r.startIndex;
    this.renderReplayPrefix();
  }

  private exitReplay(): void {
    if (!this.replay) return;
    const viewport = this.preReplayViewport;
    this.stopReplayTimer();
    this.replay = null;
    this.preReplayViewport = null;
    this.chart?.setData(this.allKlines);
    this.refreshIndicators();
    if (viewport) this.chart?.setViewport(viewport);
    this.updateReplayUi();
    this.infoEl.textContent = this.dataInfoText();
    this.scheduleSave();
  }

  private setReplaySpeed(speed: number): void {
    const r = this.replay;
    if (!r || !Number.isFinite(speed) || speed <= 0) return;
    r.speed = speed;
    if (r.playing) this.startReplayTimer();
  }

  private startReplayTimer(): void {
    this.stopReplayTimer();
    const r = this.replay;
    if (!r) return;
    const interval = Math.max(50, Math.round(1000 / r.speed));
    r.timer = window.setInterval(() => this.tickReplay(), interval);
  }

  private stopReplayTimer(): void {
    const r = this.replay;
    if (r && r.timer !== null) {
      window.clearInterval(r.timer);
      r.timer = null;
    }
  }

  private tickReplay(): void {
    const r = this.replay;
    if (!r || !r.playing) return;
    if (r.index >= this.allKlines.length - 1) {
      this.pauseReplay();
      return;
    }
    r.index++;
    this.renderReplayPrefix();
    if (r.index >= this.allKlines.length - 1) this.pauseReplay();
  }

  private renderReplayPrefix(): void {
    const r = this.replay;
    const chart = this.chart;
    if (!r || !chart) return;
    chart.updateData(this.activeKlines());
    this.refreshIndicators();
    this.updateReplayUi();
    this.infoEl.textContent = this.dataInfoText();
  }

  private disposeReplay(): void {
    this.stopReplayTimer();
    this.replay = null;
    this.preReplayViewport = null;
    this.updateReplayUi();
  }

  private async applySession(state: SessionState): Promise<void> {
    this.suppressSave = true;
    try {
      this.teardownChart('加载中...');
      this.pendingViewport = state.viewport;
      this.marketSelect.value = state.market_type;
      this.activeIndicators = state.indicators.flatMap((s) => {
        const def = findIndicator(s.id);
        return def ? [{ def, params: { ...def.defaultParams, ...s.params } }] : [];
      });

      await this.loadPairs(state.pair, state.timeframe);
    } finally {
      this.suppressSave = false;
    }
  }

  private async refreshLayoutList(selectedId?: string): Promise<void> {
    try {
      const layouts = await fetchLayouts();
      const current = selectedId ?? this.layoutSelect.value;
      this.layoutSelect.innerHTML = '<option value="">— 布局 —</option>';
      for (const layout of layouts) {
        const opt = document.createElement('option');
        opt.value = layout.id;
        opt.textContent = `${layout.name} · ${layout.pair} ${layout.timeframe}`;
        this.layoutSelect.appendChild(opt);
      }
      if (current && layouts.some((l) => l.id === current)) {
        this.layoutSelect.value = current;
      }
    } catch (e) {
      console.warn('加载布局列表失败', e);
    }
  }

  private async loadLayout(id: string): Promise<void> {
    try {
      await this.flushPendingSaves();
      const layout = await fetchLayout(id);
      if (!isSessionState(layout.payload)) {
        throw new Error('布局数据格式不兼容');
      }
      await this.applySession(layout.payload);
      this.layoutSelect.value = id;
      this.infoEl.textContent = `已加载布局: ${layout.name}`;
      this.scheduleSave();
    } catch (e) {
      this.infoEl.textContent = `加载布局失败: ${e}`;
    }
  }

  private async saveLayout(): Promise<void> {
    if (!this._pair || !this._timeframe) {
      this.infoEl.textContent = '请先加载K线数据';
      return;
    }
    const selected = this.layoutSelect.selectedOptions[0];
    const selectedName = selected?.value
      ? selected.textContent?.split(' · ')[0] ?? ''
      : '';
    const name = prompt('布局名称', selectedName || `${this._pair} ${this._timeframe}`);
    if (name === null) return;
    const trimmed = name.trim();
    if (!trimmed) return;
    await this.flushDrawings();

    const state = this.collectSessionState();
    const input: LayoutInput = {
      name: trimmed,
      market_type: state.market_type,
      pair: state.pair,
      timeframe: state.timeframe,
      payload: state,
    };
    try {
      const id = this.layoutSelect.value;
      const saved = id ? await updateLayout(id, input) : await createLayout(input);
      await this.refreshLayoutList(saved.id);
      this.infoEl.textContent = `布局已保存: ${saved.name}`;
    } catch (e) {
      this.infoEl.textContent = `保存布局失败: ${e}`;
    }
  }

  private async deleteSelectedLayout(): Promise<void> {
    const id = this.layoutSelect.value;
    if (!id) {
      this.infoEl.textContent = '请先选择一个布局';
      return;
    }
    const name = this.layoutSelect.selectedOptions[0]?.textContent ?? id;
    if (!confirm(`删除布局「${name}」？`)) return;
    try {
      await deleteLayout(id);
      await this.refreshLayoutList();
      this.infoEl.textContent = '布局已删除';
    } catch (e) {
      this.infoEl.textContent = `删除布局失败: ${e}`;
    }
  }

  // ---- 绘图持久化 ----

  private bindChart(): void {
    const chart = this.chart;
    if (!chart) return;
    this.drawingsLoaded = false;
    this.unbindViewport = chart.onViewportChange(() => {
      this.syncViewportAttr();
      this.scheduleSave();
    });
    this.unbindDrawings = chart.onDrawingsChange(() => this.scheduleDrawingSave());
    this.unbindRange = chart.onRangeSelect((range) => this.onRangeSelect(range));
    const tool = this.drawingToolSelect.value;
    chart.setDrawingTool(tool ? (tool as DrawingTool) : null);
    void this.loadDrawingsForPair();
  }

  private async loadDrawingsForPair(): Promise<void> {
    const chart = this.chart;
    if (!chart || !this._pair) return;
    const market = this.marketSelect.value;
    const pair = this._pair;
    const token = ++this.drawingLoadToken;
    try {
      const drawings = await fetchDrawings(market, pair);
      if (token !== this.drawingLoadToken || this.chart !== chart) return;
      if (this.marketSelect.value !== market || this._pair !== pair) return;
      chart.setDrawingsJSON(JSON.stringify(drawings));
      this.drawingsLoaded = true;
    } catch (e) {
      console.warn('加载绘图失败', e);
    }
  }

  private scheduleDrawingSave(): void {
    if (this.suppressSave || !this.ready || !this.chart || !this._pair || !this.drawingsLoaded) return;
    if (this.drawingSaveTimer !== null) window.clearTimeout(this.drawingSaveTimer);
    this.drawingSaveTimer = window.setTimeout(() => {
      this.drawingSaveTimer = null;
      void this.saveDrawingsNow();
    }, DRAWING_SAVE_DEBOUNCE_MS);
  }

  private async saveDrawingsNow(): Promise<void> {
    if (!this.chart || !this._pair || !this.drawingsLoaded) return;
    let drawings: DrawingDocument;
    try {
      const parsed = JSON.parse(this.chart.getDrawingsJSON()) as unknown;
      if (!Array.isArray(parsed)) return;
      drawings = parsed as DrawingDocument;
    } catch {
      return;
    }
    try {
      await replaceDrawings(this.marketSelect.value, this._pair, drawings);
    } catch (e) {
      console.warn('保存绘图失败', e);
    }
  }

  private async flushDrawings(): Promise<void> {
    if (this.drawingSaveTimer !== null) {
      window.clearTimeout(this.drawingSaveTimer);
      this.drawingSaveTimer = null;
    }
    if (this.drawingsLoaded) await this.saveDrawingsNow();
  }

  private teardownChart(placeholder: string): void {
    this.disposeReplay();
    this.invalidateQualityBadge();
    this.drawingLoadToken += 1;
    this.drawingsLoaded = false;
    if (this.saveTimer !== null) {
      window.clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
    if (this.drawingSaveTimer !== null) {
      window.clearTimeout(this.drawingSaveTimer);
      this.drawingSaveTimer = null;
    }
    this.unbindViewport?.();
    this.unbindViewport = null;
    this.unbindDrawings?.();
    this.unbindDrawings = null;
    this.unbindRange?.();
    this.unbindRange = null;
    if (this.rangeFrame !== null) {
      cancelAnimationFrame(this.rangeFrame);
      this.rangeFrame = null;
    }
    this.rangeSelection = null;
    this.rangeInfoEl.hidden = true;
    this.rangeInfoEl.textContent = '';
    this.pendingViewport = null;
    this.exhausted = { left: false, right: false };
    this.loadSeq += 1;
    this.loading = false;
    delete this.chartContainer.dataset.viewport;
    delete this.chartContainer.dataset.bars;
    this.chart?.destroy();
    this.chart = null;
    this.allKlines = [];
    this.chartContainer.innerHTML = `<div class="loading">${placeholder}</div>`;
    this.indicatorsEl.innerHTML = '';
    document.querySelectorAll('.param-editor').forEach((e) => e.remove());
  }

  private async loadPairs(preferredPair?: string, preferredTimeframe?: string): Promise<void> {
    const pairs = await fetchPairs(this.marketSelect.value);
    this.allPairs = pairs;
    this.pairSearch.value = '';
    this.pairSearch.placeholder = pairs.length > 0 ? `从 ${pairs.length} 个交易对搜索...` : '无交易对';
    this.pairSearch.disabled = pairs.length === 0;
    if (pairs.length > 0) {
      const pair = preferredPair && pairs.includes(preferredPair) ? preferredPair : pairs[0];
      this._pair = pair;
      this.pairSearch.value = pair;
      await this.loadTimeframes(preferredTimeframe);
    } else {
      this._pair = '';
      this._timeframe = '';
    }
  }

  private matchPairs(q: string): string[] {
    const query = q.trim().toUpperCase();
    const matched = query
      ? this.allPairs.filter((p) => p.includes(query))
      : this.allPairs.slice();
    matched.sort((a, b) => {
      const as = a.startsWith(query) ? 0 : 1;
      const bs = b.startsWith(query) ? 0 : 1;
      return as - bs || (a < b ? -1 : 1);
    });
    return matched.slice(0, PAIR_DROPDOWN_LIMIT);
  }

  private onPairSearchInput() {
    const matched = this.matchPairs(this.pairSearch.value);
    this.pairDropdown.innerHTML = '';
    if (matched.length === 0) {
      this.pairDropdown.classList.remove('visible');
      return;
    }
    for (const p of matched) {
      const item = document.createElement('div');
      item.className = 'pair-dropdown-item';
      if (p === this._pair) item.classList.add('selected');
      item.textContent = p;
      item.addEventListener('click', () => this.selectPair(p));
      this.pairDropdown.appendChild(item);
    }
    this.pairDropdown.classList.add('visible');
  }

  private async selectPair(pair: string): Promise<void> {
    this.pairDropdown.classList.remove('visible');
    if (pair === this._pair && this._timeframe) return;
    this.invalidateQualityBadge();
    await this.flushPendingSaves();
    this._pair = pair;
    this.pairSearch.value = pair;
    this._timeframe = '';
    this.timeframeSelect.disabled = true;
    this.activeIndicators = [];
    this.teardownChart('选择周期');
    await this.loadTimeframes();
    this.scheduleSave();
  }

  private async loadTimeframes(preferredTimeframe?: string): Promise<void> {
    const timeframes = await fetchTimeframes(this.marketSelect.value, this._pair);
    this.timeframeSelect.innerHTML = '';
    const placeholder = document.createElement('option');
    placeholder.value = '';
    placeholder.textContent = '选择周期';
    placeholder.disabled = true;
    placeholder.selected = true;
    this.timeframeSelect.appendChild(placeholder);
    for (const tf of timeframes) {
      const opt = document.createElement('option');
      opt.value = tf;
      opt.textContent = tf;
      this.timeframeSelect.appendChild(opt);
    }
    this.timeframeSelect.disabled = timeframes.length === 0;
    if (timeframes.length > 0) {
      const tf = preferredTimeframe && timeframes.includes(preferredTimeframe)
        ? preferredTimeframe
        : timeframes[0];
      this.timeframeSelect.value = tf;
      this._timeframe = tf;
      await this.loadKlines();
    } else {
      this._timeframe = '';
    }
  }

  private async loadKlines(dir?: LoadDirection): Promise<void> {
    // 边缘加载避让全量加载；全量加载总是抢占旧请求（切周期时可能连续触发）。
    if (this.replay) return;
    if (dir && this.loading) return;
    if (dir && this.exhausted[dir]) return;
    if (dir && !this.chart) return;

    const seq = ++this.loadSeq;
    const stale = () => seq !== this.loadSeq;
    this.loading = true;
    if (!dir) this.infoEl.textContent = '加载中...';
    try {
      if (dir === 'left') {
        const before = this.chart!.getFirstTime();
        if (before === null) return;
        const data = await fetchKlines({
          marketType: this.marketSelect.value,
          pair: this._pair,
          timeframe: this._timeframe,
          limit: PAGE_SIZE,
          before,
        });
        if (stale()) return;
        const bars = data.klines;
        if (bars.length > 0) {
          this.allKlines = bars.concat(this.allKlines);
          this.chart!.prependData(bars);
          this.refreshIndicators();
        }
        if (bars.length < PAGE_SIZE) this.exhausted.left = true;
      } else if (dir === 'right') {
        const after = this.chart!.getLastTime();
        if (after === null) return;
        const data = await fetchKlines({
          marketType: this.marketSelect.value,
          pair: this._pair,
          timeframe: this._timeframe,
          limit: PAGE_SIZE,
          after,
        });
        if (stale()) return;
        const bars = data.klines;
        if (bars.length > 0) {
          this.allKlines = this.allKlines.concat(bars);
          this.chart!.appendData(bars);
          this.refreshIndicators();
        }
        if (bars.length < PAGE_SIZE) this.exhausted.right = true;
      } else {
        this.invalidateQualityBadge();
        this.chartContainer.innerHTML = '<div class="loading">加载中...</div>';
        const viewport = this.pendingViewport;
        this.pendingViewport = null;
        let klines: Kline[] = [];
        if (viewport) {
          klines = await this.fetchWindowKlines(viewport);
          if (stale()) return;
        }
        // 视窗附近没有数据（超出该周期归档范围）时退回最新一段。
        if (klines.length === 0) {
          const data = await fetchKlines({
            marketType: this.marketSelect.value,
            pair: this._pair,
            timeframe: this._timeframe,
            limit: INITIAL_LOAD_BARS,
          });
          if (stale()) return;
          klines = data.klines;
        }
        if (klines.length === 0) {
          this.chartContainer.innerHTML = '<div class="loading">该周期无数据</div>';
          this.infoEl.textContent = '无数据';
          return;
        }
        this.allKlines = klines;
        this.chartContainer.innerHTML = '<div id="chart" style="height:100%;width:100%"></div>';
        const chartDiv = this.chartContainer.querySelector('#chart')!;
        this.chart = createChartManager(chartDiv as HTMLElement, (d) => void this.loadKlines(d));
        this.bindChart();
        this.chart.setData(klines);
        this.refreshIndicators();
        await this.applyInitialViewport(viewport);
        this.markAfterPaint('tv:first-screen');
        void this.loadQuality(this.marketSelect.value, this._pair, this._timeframe);
      }
      this.infoEl.textContent = this.dataInfoText();
      this.syncViewportAttr();
      if (!dir) this.scheduleSave();
    } catch (e) {
      if (!stale() && !dir) this.chartContainer.innerHTML = `<div class="loading">加载失败: ${e}</div>`;
      if (!stale()) this.infoEl.textContent = '加载失败';
    } finally {
      if (seq === this.loadSeq) {
        this.loading = false;
        requestAnimationFrame(() => this.loadEdgeIfNeeded());
      }
    }
  }

  /** 首屏/切周期后补足视窗两侧的缓冲；耗尽方向不再请求。 */
  private loadEdgeIfNeeded(): void {
    if (!this.chart || this.loading || this.replay) return;
    const dir = this.chart.needsEdgeLoad().find((d) => !this.exhausted[d]);
    if (dir) void this.loadKlines(dir);
  }

  /**
   * 按时间窗分块拉取同区间数据。
   *
   * 后端单页上限 10k，按 `before` 从新到旧逐页取，最后按时间去重合并。
   * 总根数受 [`MAX_WINDOW_BARS`] 限制，防止 1d 视窗切 1m 时把内存打爆。
   */
  private async fetchWindowKlines(viewport: Viewport): Promise<Kline[]> {
    const marketType = this.marketSelect.value;
    const pair = this._pair;
    const timeframe = this._timeframe;
    const barMs = timeframeMs(timeframe);
    const window = barMs
      ? windowForViewport(viewport, barMs, PREFETCH_BARS, MAX_WINDOW_BARS)
      : { from: viewport.from, to: viewport.to };
    const after = Math.floor(window.from) - 1;
    let before = Math.ceil(window.to) + 1;
    const chunks: Kline[][] = [];
    let total = 0;
    while (total < MAX_WINDOW_BARS) {
      const limit = Math.min(MAX_API_PAGE, MAX_WINDOW_BARS - total);
      const data = await fetchKlines({
        marketType,
        pair,
        timeframe,
        limit,
        after,
        before,
      });
      const bars = data.klines;
      if (bars.length === 0) break;
      chunks.push(bars);
      total += bars.length;
      if (bars.length < limit) break;
      before = bars[0].time;
      if (before <= after) break;
    }
    return mergeKlines(chunks);
  }

  /** 把请求的视窗裁剪到已加载数据范围内；越界部分不回退到最新端。 */
  private applyViewport(viewport: Viewport): boolean {
    const chart = this.chart;
    if (!chart) return false;
    const first = chart.getFirstTime();
    const last = chart.getLastTime();
    if (first === null || last === null) return false;
    const from = Math.max(viewport.from, first);
    const to = Math.min(viewport.to, last);
    if (to <= from) return false;
    chart.setViewport({ from, to });
    return true;
  }

  /**
   * 布局稳定后应用首屏视窗：保存的视窗可用则保持区间，否则显示最新 500 根。
   *
   * 图表刚插入 DOM 时尺寸可能还是 0，立即 setVisibleLogicalRange 会被随后的
   * ResizeObserver 覆盖成默认 barSpacing；放到下一帧再设即可落在真实宽度上。
   * 返回的 Promise 在两帧后、视窗写入 DOM 属性后完成，调用方可以据此再记录
   * 性能标记，避免测到默认视窗的中间态。
   */
  private applyInitialViewport(viewport: Viewport | null): Promise<void> {
    return new Promise((resolve) => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          if (!this.chart) {
            resolve();
            return;
          }
          if (!viewport || !this.applyViewport(viewport)) {
            this.chart.showLastBars();
          }
          this.syncViewportAttr();
          resolve();
        });
      });
    });
  }

  /** 暴露当前视窗给验收脚本：`#chart-container[data-viewport]`。 */
  private syncViewportAttr(): void {
    const viewport = this.chart?.getViewport() ?? null;
    if (viewport) this.chartContainer.dataset.viewport = JSON.stringify(viewport);
    else delete this.chartContainer.dataset.viewport;
    this.chartContainer.dataset.bars = String(this.allKlines.length);
    const visible = this.chart?.getVisibleBars() ?? null;
    if (visible !== null) this.chartContainer.dataset.visibleBars = String(visible);
    else delete this.chartContainer.dataset.visibleBars;
  }

  /** 区间选择（绘图库 date-range）变化：按动画帧合并统计。 */
  private onRangeSelect(range: RangeSelection | null): void {
    this.rangeSelection = range;
    if (this.rangeFrame !== null) return;
    this.rangeFrame = requestAnimationFrame(() => {
      this.rangeFrame = null;
      this.renderRangeInfo();
    });
  }

  private renderRangeInfo(): void {
    const selection = this.rangeSelection;
    if (!selection || this.allKlines.length === 0) {
      this.rangeInfoEl.hidden = true;
      this.rangeInfoEl.textContent = '';
      delete this.rangeInfoEl.dataset.range;
      delete this.rangeInfoEl.dataset.stats;
      return;
    }
    const stats = computeRangeStats(this.allKlines, selection.from, selection.to);
    if (!stats) {
      this.rangeInfoEl.hidden = true;
      this.rangeInfoEl.textContent = '';
      delete this.rangeInfoEl.dataset.range;
      delete this.rangeInfoEl.dataset.stats;
      return;
    }
    const pct = stats.changePct >= 0 ? `+${stats.changePct.toFixed(2)}` : stats.changePct.toFixed(2);
    const price = (v: number) => v.toLocaleString('zh-CN', { maximumFractionDigits: 8 });
    const volume = formatCompact(stats.volume);
    this.rangeInfoEl.textContent =
      `${stats.bars} 根 · ${pct}% · 高 ${price(stats.high)} · 低 ${price(stats.low)} · 量 ${volume}`;
    this.rangeInfoEl.dataset.range = JSON.stringify({ from: stats.from, to: stats.to });
    this.rangeInfoEl.dataset.stats = JSON.stringify({
      bars: stats.bars,
      changePct: stats.changePct,
      high: stats.high,
      low: stats.low,
      volume: stats.volume,
    });
    this.rangeInfoEl.hidden = false;
  }

  private refreshIndicators() {
    this.renderActiveIndicators();
    if (this.chart) {
      const klines = this.activeKlines();
      if (klines.length > 0) this.chart.applyIndicators(this.activeIndicators, klines);
    }
    this.scheduleSave();
  }

  private onSearchInput(input: HTMLInputElement, dropdown: HTMLElement) {
    const q = input.value.trim().toLowerCase();
    const all = getAllIndicators();
    const matched = q ? all.filter((i) => i.name.toLowerCase().includes(q) || i.description.toLowerCase().includes(q)) : all;
    dropdown.innerHTML = '';
    if (matched.length === 0) { dropdown.classList.remove('visible'); return; }
    for (const ind of matched) {
      const item = document.createElement('div');
      item.className = 'indicator-dropdown-item';
      item.innerHTML = `
        <span class="indicator-name">${ind.name}</span>
        <span class="indicator-cat ${ind.category}">${ind.category === 'main' ? '主图' : '副图'}</span>
        <span class="indicator-desc">${ind.description}</span>
      `;
      item.addEventListener('click', () => this.addIndicator(ind));
      dropdown.appendChild(item);
    }
    dropdown.classList.add('visible');
  }

  private addIndicator(def: IndicatorDef) {
    if (this.activeIndicators.some((a) => a.def.id === def.id)) return;
    this.activeIndicators.push({ def, params: { ...def.defaultParams } });
    (document.querySelector('#indicator-search') as HTMLInputElement).value = '';
    document.querySelector('#indicator-dropdown')?.classList.remove('visible');
    this.refreshIndicators();
  }

  private removeIndicator(id: string) {
    this.activeIndicators = this.activeIndicators.filter((a) => a.def.id !== id);
    this.refreshIndicators();
  }

  private editIndicator(id: string) {
    const active = this.activeIndicators.find((a) => a.def.id === id);
    if (!active || active.def.paramDefs.length === 0) return;

    const existing = document.querySelector(`.param-editor[data-id="${id}"]`);
    if (existing) { existing.remove(); return; }

    const editor = document.createElement('div');
    editor.className = 'param-editor';
    editor.dataset.id = id;
    editor.innerHTML = `<div class="param-editor-title">${active.def.name} 参数</div>`;

    for (const pd of active.def.paramDefs) {
      const row = document.createElement('div');
      row.className = 'param-row';
      const inp = document.createElement('input');
      inp.type = 'number';
      inp.value = String(active.params[pd.key]);
      if (pd.min !== undefined) inp.min = String(pd.min);
      if (pd.max !== undefined) inp.max = String(pd.max);
      inp.step = String(pd.step ?? 1);
      inp.dataset.key = pd.key;
      row.innerHTML = `<label>${pd.name}</label>`;
      row.appendChild(inp);
      inp.addEventListener('change', () => {
        active.params[pd.key] = parseFloat(inp.value) || pd.default;
        this.refreshIndicators();
      });
      editor.appendChild(row);
    }

    const closeBtn = document.createElement('button');
    closeBtn.className = 'param-close';
    closeBtn.textContent = '✕';
    closeBtn.addEventListener('click', () => editor.remove());
    editor.querySelector('.param-editor-title')!.after(closeBtn);

    document.body.appendChild(editor);
    const chip = document.querySelector(`[data-indicator-id="${id}"]`) as HTMLElement;
    if (chip) {
      const rect = chip.getBoundingClientRect();
      editor.style.top = `${rect.bottom + 4}px`;
      editor.style.left = `${rect.left}px`;
    }
  }

  private renderActiveIndicators() {
    const key = this.activeIndicators.map((a) => a.def.id).join('|');
    if (key === this.renderedIndicators) return;
    this.renderedIndicators = key;
    this.indicatorsEl.innerHTML = '';
    for (const active of this.activeIndicators) {
      const chip = document.createElement('span');
      chip.className = `indicator-chip ${active.def.category}`;
      chip.dataset.indicatorId = active.def.id;
      chip.innerHTML = `
        <span class="chip-label">${active.def.name}</span>
        ${active.def.paramDefs.length > 0 ? '<span class="chip-edit">⚙</span>' : ''}
        <span class="chip-remove">✕</span>
      `;
      const editBtn = chip.querySelector('.chip-edit');
      if (editBtn) editBtn.addEventListener('click', (e) => { e.stopPropagation(); this.editIndicator(active.def.id); });
      chip.querySelector('.chip-remove')!.addEventListener('click', (e) => { e.stopPropagation(); this.removeIndicator(active.def.id); });
      this.indicatorsEl.appendChild(chip);
    }
  }

  private async onMarketChange(): Promise<void> {
    this.invalidateQualityBadge();
    await this.flushPendingSaves();
    this._pair = '';
    this._timeframe = '';
    this.allPairs = [];
    this.pairSearch.value = '';
    this.pairSearch.placeholder = '搜索交易对...';
    this.pairSearch.disabled = true;
    this.pairDropdown.classList.remove('visible');
    this.timeframeSelect.disabled = true;
    this.activeIndicators = [];
    this.teardownChart('选择交易对和周期');
    await this.loadPairs();
    this.scheduleSave();
  }

  private async onTimeframeChange(): Promise<void> {
    const started = performance.now();
    const viewport = this.chart?.getViewport() ?? null;
    this.invalidateQualityBadge();
    await this.flushPendingSaves();
    this._timeframe = this.timeframeSelect.value;
    this.activeIndicators = [];
    this.teardownChart('加载中...');
    this.pendingViewport = viewport;
    if (this._timeframe) await this.loadKlines();
    this.markAfterPaint(`tv:tf-switch:${this._timeframe}`, started);
    this.scheduleSave();
  }

  private onTimeJump(input: HTMLInputElement) {
    if (!this.chart) {
      this.infoEl.textContent = '请先加载数据';
      return;
    }
    if (!input.value) return;
    const ms = new Date(input.value).getTime();
    if (isNaN(ms)) {
      this.infoEl.textContent = '无效日期格式';
      return;
    }
    this.chart.jumpToTime(ms);
  }
}
