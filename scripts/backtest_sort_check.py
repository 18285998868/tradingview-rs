#!/usr/bin/env python3
"""AC-B05 运行列表排序检查：真实浏览器里切换排序并核对顺序。

复用 `browser_layout_check.py` 的 WebDriver 封装：脚本自己启动 chromedriver，
打开回测抽屉，依次切换默认/收益/夏普/最大回撤四种排序，把渲染出的运行顺序
与接口指标算出的期望顺序逐项比对。被测服务启用 Bearer 认证时，浏览器通过
本机注入头的反向代理访问，脚本直连接口时带同一个令牌。

用法：

    python3 scripts/backtest_sort_check.py \
      --binary /usr/bin/chromium --driver /usr/bin/chromedriver \
      --url http://127.0.0.1:3001 \
      --width 1366 --height 768 --label chromium-sort

退出码：0 全部断言通过；1 有断言失败；2 环境或数据不满足。
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
import tempfile
import time
import urllib.request
from typing import Any

from browser_layout_check import AuthProxy, Session, create_session

SORT_KEYS = ("created", "return", "sharpe", "drawdown")
METRIC_FIELD = {
    "return": "total_return_pct",
    "sharpe": "sharpe_ratio",
    "drawdown": "max_drawdown_pct",
}

OPEN_PANEL = "document.querySelector('#backtest-toggle').click(); return true;"

WAIT_ROWS = r"""
const done = arguments[arguments.length - 1];
const want = arguments[0];
const started = performance.now();
function poll() {
  const rows = document.querySelectorAll('#bt-runs .bt-run');
  if (rows.length >= want) {
    done({ok: true, count: rows.length});
  } else if (performance.now() - started > 20000) {
    done({ok: false, count: rows.length});
  } else {
    setTimeout(poll, 50);
  }
}
poll();
"""

READ_ROWS = r"""
const rows = [...document.querySelectorAll('#bt-runs .bt-run')];
return rows.map((row) => ({
  title: row.querySelector('.bt-run-title')?.textContent ?? '',
  meta: row.querySelector('.bt-run-meta')?.textContent ?? '',
}));
"""

SET_SORT = r"""
const select = document.querySelector('#bt-sort');
if (!select) return {ok: false, error: '缺少 #bt-sort'};
select.value = arguments[0];
select.dispatchEvent(new Event('change', { bubbles: true }));
return {ok: true, value: select.value, options: [...select.options].map((item) => item.value)};
"""


def fetch_runs(url: str, token: str, limit: int = 100) -> list[dict[str, Any]]:
    request = urllib.request.Request(
        f"{url.rstrip('/')}/api/backtests?limit={limit}",
        headers={"Authorization": f"Bearer {token}"} if token else {},
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        document = json.load(response)
    data = document.get("data")
    if not isinstance(data, list):
        raise RuntimeError(f"接口返回异常: {json.dumps(document, ensure_ascii=False)[:200]}")
    return data


def run_label(run: dict[str, Any]) -> str:
    return f"{run.get('params_text', '')} · {run.get('pair', '')} {run.get('timeframe', '')}"


def order_for(runs: list[dict[str, Any]], key: str) -> list[str]:
    """按前端规则算期望顺序：收益/夏普降序、回撤升序、缺失指标最后，稳定排序。"""
    if key == "created":
        return [run["id"] for run in runs]
    field = METRIC_FIELD[key]
    direction = 1 if key == "drawdown" else -1

    def sort_key(run: dict[str, Any]) -> tuple[int, float]:
        metrics = run.get("metrics") or {}
        value = metrics.get(field)
        if value is None:
            return (1, 0.0)
        return (0, float(value) * direction)

    return [run["id"] for run in sorted(runs, key=sort_key)]


def map_rows(runs: list[dict[str, Any]], rows: list[dict[str, Any]]) -> tuple[list[str], list[str]]:
    """把 DOM 行按标题映射回运行 ID；同标题按接口顺序配对。"""
    by_label: dict[str, list[str]] = {}
    for run in runs:
        by_label.setdefault(run_label(run), []).append(run["id"])
    mapped: list[str] = []
    missing: list[str] = []
    for row in rows:
        candidates = by_label.get(row.get("title", ""))
        if not candidates:
            missing.append(row.get("title", ""))
            continue
        mapped.append(candidates.pop(0))
    return mapped, missing


def short(ids: list[str]) -> list[str]:
    return [item[:8] for item in ids]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, help="浏览器可执行文件")
    parser.add_argument("--driver", required=True, help="chromedriver 路径")
    parser.add_argument("--url", required=True, help="上游服务地址")
    parser.add_argument("--auth-token", default="", help="启用 Bearer 认证时提供")
    parser.add_argument("--width", type=int, default=1366)
    parser.add_argument("--height", type=int, default=768)
    parser.add_argument("--label", default="sort-check")
    parser.add_argument("--screenshot", type=pathlib.Path)
    args = parser.parse_args()

    result: dict[str, Any] = {"label": args.label, "url": args.url}
    proxy: AuthProxy | None = None
    process = None
    session: Session | None = None
    try:
        runs = fetch_runs(args.url, args.auth_token)
        if len(runs) < 2:
            print(json.dumps({**result, "ok": False, "error": f"运行数不足: {len(runs)}"}, ensure_ascii=False, indent=2))
            return 2
        for key, field in METRIC_FIELD.items():
            values = {run.get("metrics", {}).get(field) for run in runs}
            if len(values) < 2:
                print(
                    json.dumps(
                        {**result, "ok": False, "error": f"{key} 指标不足以区分顺序: {sorted(map(str, values))}"},
                        ensure_ascii=False,
                        indent=2,
                    )
                )
                return 2

        proxy = AuthProxy(args.url, args.auth_token) if args.auth_token else None
        if proxy:
            proxy.__enter__()
        target = proxy.url if proxy else args.url.rstrip("/") + "/"
        driver_log = pathlib.Path(tempfile.gettempdir()) / f"sort-{args.label}-driver.log"
        process, base, session_id = create_session(
            args.driver, "chrome", args.binary, args.width, args.height, driver_log
        )
        session = Session(base, session_id)
        session._call("POST", "/timeouts", {"script": 30000, "pageLoad": 60000, "implicit": 0})
        session.fit_viewport(args.width, args.height)
        session._call("POST", "/url", {"url": target})

        session.execute(OPEN_PANEL)
        wait = session.execute_async(WAIT_ROWS, [len(runs)])
        if not wait.get("ok"):
            raise RuntimeError(f"运行列表未渲染完整: {wait.get('count')}/{len(runs)}")

        checks: dict[str, Any] = {}
        failures: list[str] = []
        for key in SORT_KEYS:
            applied = session.execute(SET_SORT, [key])
            if not applied.get("ok"):
                failures.append(f"{key}: {applied.get('error')}")
                continue
            if applied.get("value") != key:
                failures.append(f"{key}: 下拉框未切换到 {key}")
            if set(applied.get("options") or []) != set(SORT_KEYS):
                failures.append(f"{key}: 排序选项不完整 {applied.get('options')}")
            time.sleep(0.15)
            rows = session.execute(READ_ROWS) or []
            if len(rows) != len(runs):
                failures.append(f"{key}: 行数 {len(rows)} != 运行数 {len(runs)}")
            mapped, missing = map_rows(runs, rows)
            if missing:
                failures.append(f"{key}: 无法映射的行 {short(missing)}")
            expected = order_for(runs, key)
            checks[key] = {
                "expected": short(expected),
                "actual": short(mapped),
                "match": mapped == expected,
            }
            if mapped != expected:
                failures.append(f"{key}: 顺序与期望不一致")

        if args.screenshot:
            session.screenshot(args.screenshot)
        result.update(
            {
                "ok": not failures,
                "runs": len(runs),
                "checks": checks,
                "failures": failures,
                "screenshot": str(args.screenshot) if args.screenshot else None,
            }
        )
        print(json.dumps(result, ensure_ascii=False, indent=2))
        return 0 if not failures else 1
    except Exception as exc:  # noqa: BLE001 - 命令行工具把错误落成 JSON
        print(json.dumps({**result, "ok": False, "error": f"{type(exc).__name__}: {exc}"}, ensure_ascii=False, indent=2))
        return 2
    finally:
        if session is not None:
            session.close()
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except Exception:  # noqa: BLE001
                process.kill()
        if proxy is not None:
            proxy.__exit__(None, None, None)


if __name__ == "__main__":
    sys.exit(main())
