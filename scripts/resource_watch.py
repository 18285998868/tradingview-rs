#!/usr/bin/env python3
"""AC-P04 资源峰值监控与超限告警。

采样目标进程及其子进程树（含 Python 回测 worker）的 RSS，以及项目数据目录的
已分配磁盘空间；超过阈值时输出 WARN/ERROR 日志，并在退出码上报告。

用法：
    scripts/resource_watch.py --pid 1234 --path /data/data \
        --path /data/data/tradingview.db --max-rss-gib 16 --max-disk-gib 500

退出码：0 未超限；2 超限；1 监控本身失败。
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import sys
import time
from datetime import UTC, datetime

GIB = 1024**3


def log(level: str, message: str) -> None:
    stamp = datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    print(f"{stamp} {level} {message}", file=sys.stderr, flush=True)


def read_rss(pid: int) -> int | None:
    try:
        with open(f"/proc/{pid}/status", encoding="ascii") as handle:
            for line in handle:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
    except OSError:
        return None
    return None


def children_of(pid: int) -> list[int]:
    result: list[int] = []
    task_dir = f"/proc/{pid}/task"
    try:
        tids = os.listdir(task_dir)
    except OSError:
        return result
    for tid in tids:
        try:
            with open(f"{task_dir}/{tid}/children", encoding="ascii") as handle:
                result.extend(int(value) for value in handle.read().split())
        except (OSError, ValueError):
            continue
    return result


def process_tree(root: int) -> list[int]:
    seen: list[int] = []
    stack = [root]
    while stack:
        pid = stack.pop()
        if pid in seen:
            continue
        seen.append(pid)
        stack.extend(children_of(pid))
    return seen


def tree_rss(root: int) -> tuple[int, int]:
    """返回 (RSS 合计, 存活进程数)；目标进程退出后 RSS 记为 0。"""
    total = 0
    alive = 0
    for pid in process_tree(root):
        rss = read_rss(pid)
        if rss is not None:
            total += rss
            alive += 1
    return total, alive


def allocated_bytes(paths: list[str]) -> int:
    total = 0
    for path in paths:
        try:
            if os.path.isfile(path):
                total += os.stat(path, follow_symlinks=False).st_blocks * 512
                continue
        except OSError:
            continue
        for root, _dirs, files in os.walk(path):
            for name in files:
                try:
                    total += os.stat(os.path.join(root, name), follow_symlinks=False).st_blocks * 512
                except OSError:
                    continue
    return total


def gib(value: int) -> float:
    return round(value / GIB, 3)


def main() -> int:
    parser = argparse.ArgumentParser(description="监听进程树 RSS 与项目磁盘占用")
    parser.add_argument("--pid", type=int, required=True, help="服务主进程 PID")
    parser.add_argument("--path", action="append", default=[], help="计入磁盘占用的文件或目录，可重复")
    parser.add_argument("--max-rss-gib", type=float, default=16.0, help="RSS 上限（GiB）")
    parser.add_argument("--max-disk-gib", type=float, default=500.0, help="磁盘上限（GiB）")
    parser.add_argument("--interval", type=float, default=1.0, help="RSS 采样间隔（秒）")
    parser.add_argument("--disk-interval", type=float, default=15.0, help="磁盘采样间隔（秒）")
    parser.add_argument("--duration", type=float, default=0.0, help="0 表示持续运行，直到收到 SIGTERM/SIGINT")
    parser.add_argument("--json-out", help="结束时写入的 JSON 摘要路径")
    parser.add_argument("--once", action="store_true", help="只采样一次后退出")
    args = parser.parse_args()

    rss_limit = int(args.max_rss_gib * GIB)
    disk_limit = int(args.max_disk_gib * GIB)
    if not os.path.isdir(f"/proc/{args.pid}"):
        log("ERROR", f"pid {args.pid} does not exist")
        return 1

    stop = False

    def request_stop(_signum: int, _frame: object) -> None:
        nonlocal stop
        stop = True

    signal.signal(signal.SIGTERM, request_stop)
    signal.signal(signal.SIGINT, request_stop)

    started = time.monotonic()
    rss_peak = 0
    rss_peak_at = ""
    disk_peak = allocated_bytes(args.path) if args.path else 0
    disk_peak_at = ""
    rss_samples = 0
    disk_samples = 0
    rss_breached = False
    disk_breached = False
    last_disk = started
    last_report = started

    while True:
        now = time.monotonic()
        rss, alive = tree_rss(args.pid)
        rss_samples += 1
        if rss > rss_peak:
            rss_peak = rss
            rss_peak_at = datetime.now(UTC).isoformat()
        if rss > rss_limit and not rss_breached:
            rss_breached = True
            log(
                "WARN",
                f"memory limit exceeded: rss={gib(rss)}GiB limit={args.max_rss_gib}GiB",
            )

        if args.path and (now - last_disk >= args.disk_interval or args.once):
            disk = allocated_bytes(args.path)
            disk_samples += 1
            if disk > disk_peak:
                disk_peak = disk
                disk_peak_at = datetime.now(UTC).isoformat()
            if disk > disk_limit and not disk_breached:
                disk_breached = True
                log(
                    "WARN",
                    f"disk limit exceeded: used={gib(disk)}GiB limit={args.max_disk_gib}GiB",
                )
            last_disk = now

        if now - last_report >= 30:
            log(
                "INFO",
                f"rss={gib(rss)}GiB peak={gib(rss_peak)}GiB "
                f"disk={gib(disk_peak)}GiB procs={alive}",
            )
            last_report = now

        if args.once:
            break
        if stop:
            break
        if args.duration > 0 and now - started >= args.duration:
            break
        if alive == 0:
            log("WARN", f"pid {args.pid} exited; stopping monitor")
            break
        time.sleep(max(args.interval, 0.05))

    summary = {
        "pid": args.pid,
        "duration_secs": round(time.monotonic() - started, 1),
        "rss_samples": rss_samples,
        "rss_peak_bytes": rss_peak,
        "rss_peak_gib": gib(rss_peak),
        "rss_limit_bytes": rss_limit,
        "rss_peak_at": rss_peak_at,
        "rss_breached": rss_breached,
        "disk_samples": disk_samples,
        "disk_peak_bytes": disk_peak,
        "disk_peak_gib": gib(disk_peak),
        "disk_limit_bytes": disk_limit,
        "disk_peak_at": disk_peak_at,
        "disk_breached": disk_breached,
        "paths": args.path,
    }
    if args.json_out:
        with open(args.json_out, "w", encoding="utf-8") as handle:
            json.dump(summary, handle, ensure_ascii=False, indent=2)
    log(
        "ERROR" if (rss_breached or disk_breached) else "INFO",
        "SUMMARY " + json.dumps(summary, ensure_ascii=False),
    )
    return 2 if (rss_breached or disk_breached) else 0


if __name__ == "__main__":
    sys.exit(main())
