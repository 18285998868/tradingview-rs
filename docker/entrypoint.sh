#!/bin/sh
# 从环境变量生成 TOML 配置，再启动服务。
#
# 必填：TV_AUTH_TOKEN（容器监听 0.0.0.0，未配置认证时服务会拒绝启动）。
# 可选：TV_DATA_DIR（默认 /data）、TV_STATE_DIR（默认 /state）、
#       TV_INTERNAL_PORT（默认 3001）、RUST_LOG（默认 info）。
set -eu
umask 077

if [ -z "${TV_AUTH_TOKEN:-}" ]; then
    echo "TV_AUTH_TOKEN is required (container listens on 0.0.0.0)" >&2
    exit 64
fi

DATA_DIR=${TV_DATA_DIR:-/data}
STATE_DIR=${TV_STATE_DIR:-/state}
INTERNAL_PORT=${TV_INTERNAL_PORT:-3001}
CONFIG_PATH=${TRADINGVIEW_CONFIG:-/tmp/tradingview.toml}

mkdir -p "$STATE_DIR/backtests" "$STATE_DIR/cache"

CONFIG_PATH="$CONFIG_PATH" DATA_DIR="$DATA_DIR" STATE_DIR="$STATE_DIR" INTERNAL_PORT="$INTERNAL_PORT" \
/usr/local/bin/python - <<'PY'
import json
import os
import pathlib

def quote(value: str) -> str:
    # TOML basic string 与 JSON 字符串的转义规则兼容，避免令牌里的引号破坏配置。
    return json.dumps(value, ensure_ascii=False)

data_dir = os.environ["DATA_DIR"]
state_dir = os.environ["STATE_DIR"]
port = int(os.environ["INTERNAL_PORT"])
config = f"""\
host = "0.0.0.0"
port = {port}
data_dir = {quote(data_dir)}
db_path = {quote(state_dir + "/tradingview.db")}
frontend_dir = "/app/frontend"
auth_token = {quote(os.environ["TV_AUTH_TOKEN"])}

[backtest]
enabled = true
python_bin = "/usr/local/bin/python"
worker_dir = "/app/python"
output_dir = {quote(state_dir + "/backtests")}
timeout_secs = 1800
max_concurrent = 2
max_compare_runs = 10
max_grid_runs = 100
max_curve_points = 2000
max_trades = 200000
keep_equity = true
"""
pathlib.Path(os.environ["CONFIG_PATH"]).write_text(config, encoding="utf-8")
PY

export RUST_LOG=${RUST_LOG:-info}
exec /usr/local/bin/tradingview
