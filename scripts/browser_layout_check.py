#!/usr/bin/env python3
"""浏览器布局兼容检查：Firefox / Chromium + W3C WebDriver。

只依赖标准库。被测服务启用 Bearer 认证时，脚本会在本机起一个注入
Authorization 头的反向代理，浏览器访问代理端口，避免把令牌写进 URL。

用法示例：

    python3 scripts/browser_layout_check.py \
      --browser firefox --binary /usr/bin/firefox \
      --driver /tmp/gecko/geckodriver \
      --url http://127.0.0.1:3001 --auth-token "$TV_AUTH_TOKEN" \
      --width 1366 --height 768 --label firefox-160-1366x768 \
      --screenshot /tmp/v07/firefox-160-1366x768.png

退出码 0 表示所有断言通过，1 表示有断言失败，2 表示环境/驱动错误。
"""

from __future__ import annotations

import argparse
import base64
import http.client
import json
import os
import pathlib
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


class CheckError(RuntimeError):
    pass


class AuthProxy:
    """把浏览器请求转发到上游，并补上 Bearer 令牌。"""

    def __init__(self, upstream: str, token: str) -> None:
        parsed = urllib.parse.urlsplit(upstream)
        if parsed.scheme != "http" or not parsed.hostname:
            raise CheckError("--url 必须是 http://host:port 形式")
        self.upstream_host = parsed.hostname
        self.upstream_port = parsed.port or 80
        self.token = token
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def _forward(self) -> None:
                length = int(self.headers.get("content-length") or 0)
                body = self.rfile.read(length) if length else None
                headers = {
                    key: value
                    for key, value in self.headers.items()
                    if key.lower() not in {"host", "authorization", "connection"}
                }
                headers["Authorization"] = f"Bearer {outer.token}"
                connection = http.client.HTTPConnection(
                    outer.upstream_host, outer.upstream_port, timeout=120
                )
                try:
                    connection.request(self.command, self.path, body=body, headers=headers)
                    response = connection.getresponse()
                    payload = response.read()
                finally:
                    connection.close()

                self.send_response(response.status, response.reason)
                for key, value in response.getheaders():
                    if key.lower() in {"transfer-encoding", "connection", "content-length"}:
                        continue
                    if key.lower() == "location":
                        value = value.replace(
                            f"{outer.upstream_host}:{outer.upstream_port}",
                            f"127.0.0.1:{self.server.server_port}",
                        )
                    self.send_header(key, value)
                self.send_header("Content-Length", str(len(payload)))
                self.send_header("Connection", "close")
                self.end_headers()
                if self.command != "HEAD":
                    self.wfile.write(payload)

            do_GET = _forward
            do_POST = _forward
            do_PUT = _forward
            do_PATCH = _forward
            do_DELETE = _forward
            do_OPTIONS = _forward

            def log_message(self, format: str, *args: Any) -> None:
                del format, args

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    @property
    def url(self) -> str:
        return f"http://127.0.0.1:{self.server.server_port}/"

    def __enter__(self) -> AuthProxy:
        self.thread.start()
        return self

    def __exit__(self, *exc: object) -> None:
        self.server.shutdown()
        self.server.server_close()


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def wait_port(port: int, process: subprocess.Popen[bytes], timeout: float = 30.0) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise CheckError(f"WebDriver 提前退出，退出码 {process.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return
        except OSError:
            time.sleep(0.1)
    raise CheckError(f"WebDriver 端口 {port} 在 {timeout}s 内未就绪")


def http_json(
    base: str, method: str, path: str, payload: dict[str, Any] | None = None
) -> dict[str, Any]:
    body = None if payload is None else json.dumps(payload).encode()
    request = urllib.request.Request(
        base + path,
        data=body,
        method=method,
        headers={"content-type": "application/json"} if body is not None else {},
    )
    try:
        with urllib.request.urlopen(request, timeout=180) as response:
            return json.loads(response.read().decode())
    except urllib.error.HTTPError as exc:
        raise CheckError(f"{method} {path} -> {exc.code}: {exc.read().decode(errors='replace')}")


class Session:
    def __init__(self, base: str, session_id: str) -> None:
        self.base = base
        self.session_id = session_id

    def _path(self, suffix: str) -> str:
        return f"/session/{self.session_id}{suffix}"

    def _call(self, method: str, suffix: str, payload: dict[str, Any] | None = None) -> Any:
        document = http_json(self.base, method, self._path(suffix), payload)
        value = document.get("value")
        if isinstance(value, dict) and value.get("error"):
            raise CheckError(f"{method} {suffix}: {value}")
        return value

    def execute(self, script: str, args: list[Any] | None = None) -> Any:
        return self._call("POST", "/execute/sync", {"script": script, "args": args or []})

    def execute_async(self, script: str, args: list[Any] | None = None) -> Any:
        return self._call("POST", "/execute/async", {"script": script, "args": args or []})

    def screenshot(self, path: pathlib.Path) -> None:
        data = self._call("GET", "/screenshot")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(base64.b64decode(data))

    def window_rect(self) -> dict[str, Any]:
        return self._call("GET", "/window/rect")

    def set_window(self, width: int, height: int) -> None:
        self._call("POST", "/window/rect", {"width": width, "height": height})

    def viewport(self) -> tuple[int, int]:
        size = self.execute("return {w: window.innerWidth, h: window.innerHeight};")
        return int(size["w"]), int(size["h"])

    def fit_viewport(self, width: int, height: int) -> tuple[int, int]:
        self.set_window(width, height)
        for _ in range(4):
            inner_w, inner_h = self.viewport()
            dx, dy = width - inner_w, height - inner_h
            if abs(dx) <= 1 and abs(dy) <= 1:
                return inner_w, inner_h
            rect = self.window_rect()
            self.set_window(int(rect["width"]) + dx, int(rect["height"]) + dy)
        return self.viewport()

    def close(self) -> None:
        try:
            http_json(self.base, "DELETE", self._path(""))
        except CheckError:
            pass


def create_session(
    driver: str, browser: str, binary: str, width: int, height: int, log_path: pathlib.Path
) -> tuple[subprocess.Popen[bytes], str, str]:
    port = free_port()
    log = log_path.open("wb")
    command = [driver, f"--port={port}"]
    if browser == "firefox":
        command = [driver, "--port", str(port)]
    process = subprocess.Popen(
        command,
        stdin=subprocess.DEVNULL,
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    try:
        wait_port(port, process)
        base = f"http://127.0.0.1:{port}"
        if browser == "firefox":
            options: dict[str, Any] = {
                "binary": binary,
                "args": ["-headless", f"--width={width}", f"--height={height}"],
                "prefs": {
                    "browser.shell.checkDefaultBrowser": False,
                    "browser.startup.homepage_override.mstone": "ignore",
                    "datareporting.policy.dataSubmissionEnabled": False,
                },
            }
            capabilities: dict[str, Any] = {
                "browserName": "firefox",
                "acceptInsecureCerts": True,
                "moz:firefoxOptions": options,
            }
        else:
            options = {
                "binary": binary,
                "args": [
                    "--headless=new",
                    "--no-sandbox",
                    "--disable-dev-shm-usage",
                    "--disable-gpu",
                    f"--window-size={width},{height}",
                ],
            }
            capabilities = {
                "browserName": "chrome",
                "acceptInsecureCerts": True,
                "goog:chromeOptions": options,
            }
        document = http_json(
            base,
            "POST",
            "/session",
            {"capabilities": {"alwaysMatch": capabilities}},
        )
        session_id = document["value"]["sessionId"]
        return process, base, session_id
    except Exception:
        process.terminate()
        process.wait(timeout=10)
        log.close()
        raise


METRICS_SCRIPT = r"""
const rect = (el) => {
  if (!el) return null;
  const r = el.getBoundingClientRect();
  return {left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height};
};
const visible = (el) => !!el && el.offsetParent !== null && rect(el).width > 0 && rect(el).height > 0;
const keys = [
  '#market-select', '#pair-search', '#timeframe-select', '#layout-select',
  '#layout-save', '#backtest-toggle', '#info'
];
const keyRects = {};
for (const key of keys) keyRects[key] = {visible: visible(document.querySelector(key)), rect: rect(document.querySelector(key))};
const doc = document.documentElement;
const chart = document.querySelector('#chart-container');
const canvas = document.querySelector('#chart canvas');
const panel = document.querySelector('#backtest-panel');
return {
  userAgent: navigator.userAgent,
  innerWidth: window.innerWidth,
  innerHeight: window.innerHeight,
  dpr: window.devicePixelRatio,
  scrollWidth: doc.scrollWidth,
  bodyScrollWidth: document.body.scrollWidth,
  horizontalOverflow: Math.max(0, doc.scrollWidth - window.innerWidth),
  stylesheets: document.styleSheets.length,
  bodyBackground: getComputedStyle(document.body).backgroundColor,
  toolbar: rect(document.querySelector('.toolbar')),
  chart: rect(chart),
  canvas: rect(canvas),
  panel: rect(panel),
  panelVisible: !!panel && panel.classList.contains('visible'),
  loading: !!document.querySelector('.loading'),
  infoText: document.querySelector('#info')?.textContent ?? '',
  appVersion: document.querySelector('#app-version')?.textContent ?? '',
  keyRects,
};
"""

WAIT_MARK_SCRIPT = r"""
const done = arguments[arguments.length - 1];
const started = performance.now();
function poll() {
  const mark = performance.getEntriesByName('tv:first-screen').at(-1);
  if (mark) {
    done({ok: true, mark: mark.startTime, bars: document.querySelector('#info')?.textContent ?? ''});
  } else if (performance.now() - started > 20000) {
    done({ok: false, error: 'tv:first-screen 超时', bars: document.querySelector('#info')?.textContent ?? ''});
  } else {
    setTimeout(poll, 20);
  }
}
poll();
"""

PANEL_SCRIPT = r"""
const panel = document.querySelector('#backtest-panel');
const body = document.querySelector('.bt-body');
const rect = (el) => { if (!el) return null; const r = el.getBoundingClientRect(); return {left: r.left, top: r.top, right: r.right, bottom: r.bottom, width: r.width, height: r.height}; };
return {
  panel: rect(panel),
  visible: !!panel && panel.classList.contains('visible'),
  bodyClientHeight: body?.clientHeight ?? 0,
  bodyScrollHeight: body?.scrollHeight ?? 0,
  submit: rect(document.querySelector('#bt-submit')),
  close: rect(document.querySelector('#bt-close')),
};
"""


def check_metrics(
    metrics: dict[str, Any], width: int, height: int, panel: dict[str, Any]
) -> list[str]:
    failures: list[str] = []
    if metrics["horizontalOverflow"] > 1:
        failures.append(f"页面横向溢出 {metrics['horizontalOverflow']}px")
    if metrics["bodyScrollWidth"] > width + 1:
        failures.append(f"body 横向溢出 {metrics['bodyScrollWidth'] - width}px")
    if metrics["stylesheets"] == 0:
        failures.append("样式表未加载")
    if not metrics["infoText"]:
        failures.append("#info 为空")
    if metrics["loading"]:
        failures.append("首屏标记后仍显示 loading")
    if metrics["appVersion"] == "":
        failures.append("#app-version 为空")

    chart = metrics["chart"]
    if not chart:
        failures.append("缺少 #chart-container")
    else:
        if chart["width"] < width * 0.95:
            failures.append(f"图表宽度 {chart['width']:.0f} < 视口 95%")
        if chart["height"] < height * 0.55:
            failures.append(f"图表高度 {chart['height']:.0f} < 视口 55%")
        if chart["right"] > width + 1 or chart["bottom"] > height + 1:
            failures.append("图表超出视口")
    canvas = metrics["canvas"]
    if not canvas:
        failures.append("缺少图表 canvas")
    elif canvas["width"] < 200 or canvas["height"] < 150:
        failures.append(f"canvas 尺寸异常 {canvas['width']:.0f}x{canvas['height']:.0f}")

    toolbar = metrics["toolbar"]
    if not toolbar:
        failures.append("缺少工具栏")
    elif toolbar["left"] < -1 or toolbar["right"] > width + 1:
        failures.append("工具栏横向超出视口")
    for key, item in metrics["keyRects"].items():
        if not item["visible"]:
            failures.append(f"关键控件不可见: {key}")
            continue
        rect = item["rect"]
        if rect["left"] < -1 or rect["right"] > width + 1:
            failures.append(f"关键控件超出视口: {key}")

    if not panel["visible"]:
        failures.append("回测面板未打开")
    else:
        rect = panel["panel"]
        if rect["left"] < -1 or rect["right"] > width + 1:
            failures.append("回测面板横向超出视口")
        if rect["top"] < -1 or rect["bottom"] > height + 1:
            failures.append("回测面板纵向超出视口")
        if rect["width"] < 300:
            failures.append(f"回测面板宽度异常 {rect['width']:.0f}")
        if not panel["close"] or panel["close"]["width"] <= 0:
            failures.append("回测面板关闭按钮不可见")
        if panel["bodyScrollHeight"] < panel["bodyClientHeight"]:
            failures.append("回测面板内容高度异常")
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser", choices=["firefox", "chrome"], required=True)
    parser.add_argument("--binary", required=True, help="浏览器可执行文件")
    parser.add_argument("--driver", required=True, help="geckodriver 或 chromedriver")
    parser.add_argument("--url", required=True, help="上游服务地址")
    parser.add_argument("--auth-token", default="", help="启用 Bearer 认证时提供")
    parser.add_argument("--width", type=int, required=True)
    parser.add_argument("--height", type=int, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--screenshot", type=pathlib.Path)
    parser.add_argument("--timeout", type=int, default=45)
    args = parser.parse_args()

    proxy = AuthProxy(args.url, args.auth_token) if args.auth_token else None
    target = proxy.url if proxy else args.url.rstrip("/") + "/"
    driver_log = pathlib.Path(tempfile.gettempdir()) / f"v07-{args.label}-driver.log"
    process: subprocess.Popen[bytes] | None = None
    session: Session | None = None
    result: dict[str, Any] = {"label": args.label, "browser": args.browser, "target": target}
    exit_code = 1
    try:
        if proxy:
            proxy.__enter__()
        process, base, session_id = create_session(
            args.driver, args.browser, args.binary, args.width, args.height, driver_log
        )
        session = Session(base, session_id)
        session._call(
            "POST",
            "/timeouts",
            {"script": 30000, "pageLoad": 60000, "implicit": 0},
        )
        inner = session.fit_viewport(args.width, args.height)
        http_json(base, "POST", f"/session/{session_id}/url", {"url": target})
        mark = session.execute_async(WAIT_MARK_SCRIPT)
        time.sleep(0.5)
        metrics = session.execute(METRICS_SCRIPT)
        if args.screenshot:
            # 主视图截图在打开回测抽屉之前拍，避免固定抽屉遮住工具栏。
            session.screenshot(args.screenshot)
        session.execute("document.querySelector('#backtest-toggle').click(); return true;")
        time.sleep(0.6)
        panel = session.execute(PANEL_SCRIPT)
        failures = check_metrics(metrics, args.width, args.height, panel)
        if not mark.get("ok"):
            failures.append(f"首屏标记失败: {mark.get('error')}")
        result.update(
            {
                "ok": not failures,
                "viewport": {"requested": [args.width, args.height], "inner": inner},
                "mark": mark,
                "metrics": metrics,
                "panel": panel,
                "failures": failures,
                "screenshot": str(args.screenshot) if args.screenshot else None,
                "driverLog": str(driver_log),
            }
        )
        exit_code = 0 if not failures else 1
    except Exception as exc:  # noqa: BLE001 - 命令行工具需要把错误落成 JSON
        result.update({"ok": False, "failures": [f"{type(exc).__name__}: {exc}"]})
        exit_code = 2
    finally:
        if session is not None:
            session.close()
        if process is not None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
        if proxy is not None:
            proxy.__exit__(None, None, None)
        print(json.dumps(result, ensure_ascii=False, indent=2))
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
