//! 可选 HTTP 认证：Bearer 令牌或浏览器原生 Basic 登录。
//!
//! 设计取舍：
//! - 只认配置里的一个令牌，不做多用户/会话；够用且没有状态。
//! - Basic 用户名任意、密码为令牌，浏览器会弹原生登录框；脚本用 Bearer。
//! - Base64 解码手写，不引入额外依赖；只接受标准字母表与合法填充。
//! - 比较使用常量时间实现，避免按字节提前返回。
//! - `/api/health` 与 `OPTIONS` 豁免，方便探活与 CORS 预检。

use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// 认证令牌；`None` 表示关闭认证。
pub type AuthState = Option<Arc<str>>;

/// 常量时间字节比较：长度不同直接失败，长度相同时逐字节异或累加。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// 标准 Base64 解码；非法字符、错误填充或长度不是 4 的倍数返回 `None`。
fn decode_base64(input: &str) -> Option<Vec<u8>> {
    const INVALID: u8 = 0xff;

    fn sextet(b: u8) -> u8 {
        match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => INVALID,
        }
    }

    let bytes = input.as_bytes();
    let (chunks, remainder) = bytes.as_chunks::<4>();
    if !remainder.is_empty() {
        return None;
    }
    let total = chunks.len();
    let mut out = Vec::with_capacity(total * 3);
    for (index, chunk) in chunks.iter().enumerate() {
        let pad = chunk.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 {
            return None;
        }
        // 填充只能出现在最后一组。
        if pad > 0 && index + 1 != total {
            return None;
        }
        let mut buf = [0u8; 4];
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                if i < 4 - pad {
                    return None;
                }
                buf[i] = 0;
            } else {
                let v = sextet(b);
                if v == INVALID {
                    return None;
                }
                buf[i] = v;
            }
        }
        let n = (u32::from(buf[0]) << 18)
            | (u32::from(buf[1]) << 12)
            | (u32::from(buf[2]) << 6)
            | u32::from(buf[3]);
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// 校验 `Authorization` 头：支持 `Bearer <token>` 与 `Basic <base64>`。
fn check_authorization(value: &str, token: &str) -> bool {
    let mut parts = value.splitn(2, ' ');
    let scheme = parts.next().unwrap_or_default();
    let credentials = parts.next().unwrap_or_default().trim();
    if credentials.is_empty() {
        return false;
    }
    if scheme.eq_ignore_ascii_case("bearer") {
        return constant_time_eq(credentials.as_bytes(), token.as_bytes());
    }
    if scheme.eq_ignore_ascii_case("basic") {
        let Some(decoded) = decode_base64(credentials) else {
            return false;
        };
        let Some(colon) = decoded.iter().position(|&b| b == b':') else {
            return false;
        };
        // 用户名任意，密码即令牌。
        return constant_time_eq(&decoded[colon + 1..], token.as_bytes());
    }
    false
}

/// 认证中间件：未配置令牌时直接放行。
///
/// # Errors
/// 不返回错误；未认证时直接构造 401 响应。
pub async fn require_auth(State(token): State<AuthState>, req: Request, next: Next) -> Response {
    let Some(token) = token else {
        return next.run(req).await;
    };
    let path = req.uri().path();
    // 行情与偏好接口（`/api/*`、`/udf/*`）需要认证；静态页面与健康检查放行。
    let protected =
        path == "/api" || path.starts_with("/api/") || path == "/udf" || path.starts_with("/udf/");
    if req.method() == Method::OPTIONS || path == "/api/health" || !protected {
        return next.run(req).await;
    }
    let authorized = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| check_authorization(v, &token));
    if authorized {
        return next.run(req).await;
    }

    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({
            "success": false,
            "data": null,
            "error": "unauthorized",
        })),
    )
        .into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"tradingview\""),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_decodes_known_values() {
        assert_eq!(decode_base64("dXNlcjpwYXNz").unwrap(), b"user:pass");
        assert_eq!(decode_base64("dG9r").unwrap(), b"tok");
        assert_eq!(decode_base64("dG8=").unwrap(), b"to");
        assert_eq!(decode_base64("dA==").unwrap(), b"t");
        assert_eq!(decode_base64("").unwrap(), b"");
    }

    #[test]
    fn base64_rejects_invalid_input() {
        assert!(decode_base64("dG9r=").is_none());
        assert!(decode_base64("dG9r===").is_none());
        assert!(decode_base64("d=9r").is_none());
        assert!(decode_base64("dG9r*").is_none());
        assert!(decode_base64("dG9r").is_some());
    }

    #[test]
    fn bearer_and_basic_are_accepted() {
        assert!(check_authorization("Bearer tok", "tok"));
        assert!(check_authorization("bearer tok", "tok"));
        assert!(check_authorization("Basic dXNlcjpwYXNz", "pass"));
        assert!(check_authorization("basic dXNlcjpwYXNz", "pass"));
        // 用户名任意。
        assert!(check_authorization("Basic YWRtaW46cGFzcw==", "pass"));
    }

    #[test]
    fn wrong_credentials_are_rejected() {
        assert!(!check_authorization("Bearer nope", "tok"));
        assert!(!check_authorization("Bearer", "tok"));
        assert!(!check_authorization("Basic dXNlcjpwYXNz", "tok"));
        assert!(!check_authorization("Basic !!!", "tok"));
        assert!(!check_authorization("Basic dXNlcg==", "user"));
        assert!(!check_authorization("Digest abc", "tok"));
        assert!(!check_authorization("", "tok"));
    }

    #[test]
    fn constant_time_compare_matches_bytes() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }
}
