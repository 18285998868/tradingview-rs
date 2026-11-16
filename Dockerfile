# 前端构建：产物复制到运行时镜像的 /app/frontend。
FROM node:22-bookworm-slim AS frontend
WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY frontend/ ./
RUN npm run build

# 后端构建。data-reader 是本仓库 Cargo 路径依赖的兄弟 workspace，
# 由 compose 的 additional_contexts 以只读命名上下文传入。
FROM rust:1.100-slim-bookworm AS backend
WORKDIR /build/tradingview-rs
COPY Cargo.toml Cargo.lock README.md LICENSE ./
COPY src ./src
COPY migrations ./migrations
COPY python ./python
COPY --from=data-reader Cargo.toml Cargo.lock /build/data-reader/
COPY --from=data-reader crates /build/data-reader/crates
RUN cargo build --release --locked --bin tradingview

# 运行时：Python + vectorbt 负责回测 worker，Rust 二进制负责 HTTP 服务。
FROM python:3.12-slim-bookworm AS runtime

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    HOME=/state \
    NUMBA_CACHE_DIR=/state/cache/numba \
    MPLCONFIGDIR=/state/cache/matplotlib \
    TRADINGVIEW_CONFIG=/tmp/tradingview.toml

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgomp1 tini \
    && rm -rf /var/lib/apt/lists/*

# 先装依赖，再复制入口脚本和构建产物，保证源码改动不重复安装 vectorbt。
COPY python /app/python
RUN pip install --no-cache-dir /app/python

RUN groupadd --gid 10001 tradingview \
    && useradd --uid 10001 --gid tradingview --home-dir /state --shell /usr/sbin/nologin tradingview \
    && mkdir -p /state/cache /app/frontend \
    && chown -R tradingview:tradingview /state /app

COPY --from=backend /build/tradingview-rs/target/release/tradingview /usr/local/bin/tradingview
COPY --from=frontend /build/frontend/dist /app/frontend
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod 0755 /usr/local/bin/entrypoint.sh

USER tradingview
EXPOSE 3001

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/entrypoint.sh"]
