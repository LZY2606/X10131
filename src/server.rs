//! 基于 tiny_http 的本地 HTTP 服务，静态页面内嵌进二进制。


use tiny_http::{Header, Method, Request, Response, Server};

use crate::api::{self, QueryRequest, SharedState};
use crate::storage::ExportEnvelope;

const INDEX_HTML: &str = include_str!("static/index.html");
const APP_JS: &str = include_str!("static/app.js");
const STYLE_CSS: &str = include_str!("static/style.css");

pub fn serve(server: Server, state: SharedState) {
    for request in server.incoming_requests() {
        let method = request.method().clone();
        let raw_url = request.url().to_string();
        let (path, query) = raw_url.split_once('?').unwrap_or((&raw_url, ""));
        if let Err(message) = handle(request, &method, path, query, state.clone()) {
            eprintln!("请求出错 {path}: {message}");
        }
    }
}

fn content_type(ct: &str) -> Header {
    Header::from_bytes(&b"Content-Type"[..], ct.as_bytes()).unwrap()
}

fn respond_text(request: Request, status: u16, ct: &str, body: &'static str) -> std::io::Result<()> {
    let resp = Response::from_data(body.as_bytes())
        .with_status_code(status)
        .with_header(content_type(ct));
    request.respond(resp)
}

fn respond_json(request: Request, status: u16, body: String) -> std::io::Result<()> {
    let resp = Response::from_data(body.into_bytes())
        .with_status_code(status)
        .with_header(content_type("application/json; charset=utf-8"));
    request.respond(resp)
}

fn respond_err(request: Request, status: u16, message: &str) -> std::io::Result<()> {
    respond_json(request, status, serde_json::json!({ "error": message }).to_string())
}

fn handle(
    request: Request,
    method: &Method,
    path: &str,
    query: &str,
    state: SharedState,
) -> Result<(), String> {
    let req = Req(request);
    match (method, path) {
        (Method::Get, "/") | (Method::Get, "/index.html") => {
            req.text(200, "text/html; charset=utf-8", INDEX_HTML)
        }
        _ => handle_dyn_req(req, method, path, query, state),
    }
}

// tiny_http 的 Request 按值传递，用一个小包装避免所有权书写噪音。
struct Req(Request);

impl Req {
    fn text(self, status: u16, ct: &str, body: &'static str) -> Result<(), String> {
        respond_text(self.0, status, ct, body).map_err(|e| e.to_string())
    }
    fn json(self, status: u16, body: String) -> Result<(), String> {
        respond_json(self.0, status, body).map_err(|e| e.to_string())
    }
    fn err(self, status: u16, message: &str) -> Result<(), String> {
        respond_err(self.0, status, message).map_err(|e| e.to_string())
    }
    fn body(&mut self) -> Result<Vec<u8>, String> {
        let mut content = Vec::new();
        self.0
            .as_reader()
            .read_to_end(&mut content)
            .map_err(|e| e.to_string())?;
        Ok(content)
    }
}

fn handle_dyn_req(
    req: Req,
    method: &Method,
    path: &str,
    _query: &str,
    state: SharedState,
) -> Result<(), String> {
    match (method, path) {
        (Method::Get, "/static/app.js") => {
            req.text(200, "application/javascript; charset=utf-8", APP_JS)
        }
        (Method::Get, "/static/style.css") => req.text(200, "text/css; charset=utf-8", STYLE_CSS),
        (Method::Get, "/api/health") => {
            let body = serde_json::json!({
                "ok": true,
                "tzdb": crate::tzutil::tzdb_version(),
                "title": "周期规则裁决器"
            });
            req.json(200, body.to_string())
        }
        (Method::Post, "/api/resolve-local") => {
            let mut req = req;
            let bytes = req.body()?;
            let rreq = match serde_json::from_slice::<api::ResolveRequest>(&bytes) {
                Ok(r) => r,
                Err(e) => return req.err(400, &e.to_string()),
            };
            match api::resolve_local_endpoint(rreq) {
                Ok(items) => req.json(200, serde_json::json!({ "items": items }).to_string()),
                Err(err) => req.err(err.status, &err.message),
            }
        }
        (Method::Get, "/api/timezones") => {
            req.json(200, serde_json::to_string(&api::list_timezones()).unwrap())
        }
        (Method::Get, "/api/rules") => {
            let st = state.lock().unwrap();
            let rules = st.store.list_rules().map_err(|e| e.to_string())?;
            req.json(200, serde_json::to_string(&rules).unwrap())
        }
        (Method::Post, "/api/rules") => {
            let mut req = req;
            let bytes = req.body()?;
            let rule = match serde_json::from_slice::<crate::model::RuleVersion>(&bytes) {
                Ok(r) => r,
                Err(e) => return req.err(400, &e.to_string()),
            };
            let mut st = state.lock().unwrap();
            match api::save_rule(&mut st, rule) {
                Ok(fp) => req.json(200, serde_json::json!({ "ok": true, "rule_fingerprint": fp }).to_string()),
                Err(err) => req.err(err.status, &err.message),
            }
        }
        (Method::Get, "/api/sessions") => {
            let st = state.lock().unwrap();
            let ids = st.store.list_sessions().map_err(|e| e.to_string())?;
            req.json(200, serde_json::to_string(&ids).unwrap())
        }
        (Method::Post, "/api/evaluate") => {
            let mut req = req;
            let bytes = req.body()?;
            let rule = match serde_json::from_slice::<crate::model::RuleVersion>(&bytes) {
                Ok(r) => r,
                Err(e) => return req.err(400, &e.to_string()),
            };
            match api::evaluate_rule(rule) {
                Ok(resp) => req.json(200, serde_json::to_string(&resp).unwrap()),
                Err(err) => req.err(err.status, &err.message),
            }
        }
        (Method::Post, "/api/query") => {
            let mut req = req;
            let bytes = req.body()?;
            let qreq = match serde_json::from_slice::<QueryRequest>(&bytes) {
                Ok(q) => q,
                Err(e) => return req.err(400, &e.to_string()),
            };
            let mut st = state.lock().unwrap();
            match api::handle_query(&mut st, qreq) {
                Ok(resp) => req.json(200, serde_json::to_string(&resp).unwrap()),
                Err(err) => req.err(err.status, &err.message),
            }
        }
        (Method::Post, "/api/export") => {
            #[derive(serde::Deserialize)]
            struct Body {
                session_id: String,
            }
            let mut req = req;
            let bytes = req.body()?;
            let body = match serde_json::from_slice::<Body>(&bytes) {
                Ok(b) => b,
                Err(e) => return req.err(400, &e.to_string()),
            };
            let st = state.lock().unwrap();
            let session = match st.store.load_session(&body.session_id) {
                Ok(Some(s)) => s,
                Ok(None) => return req.err(404, "会话不存在"),
                Err(e) => return req.err(500, &e.to_string()),
            };
            let envelope =
                crate::storage::export_session(session, crate::tzutil::tzdb_version());
            req.json(200, serde_json::to_string(&envelope).unwrap())
        }
        (Method::Post, "/api/import") => {
            let mut req = req;
            let bytes = req.body()?;
            let envelope = match serde_json::from_slice::<ExportEnvelope>(&bytes) {
                Ok(e) => e,
                Err(e) => return req.err(400, &e.to_string()),
            };
            let (session, warning) = match crate::storage::import_envelope(envelope) {
                Ok(v) => v,
                Err(e) => return req.err(400, &e.to_string()),
            };
            let st = state.lock().unwrap();
            st.store
                .save_rule(&session.rule_snapshot)
                .map_err(|e| e.to_string())?;
            st.store
                .save_session(&session)
                .map_err(|e| e.to_string())?;
            let resp = serde_json::json!({
                "ok": true,
                "session_id": session.session_id,
                "rule_fingerprint": session.rule_fingerprint,
                "queries": session.queries.len(),
                "warning": warning,
            });
            req.json(200, resp.to_string())
        }
        _ => req.text(404, "text/plain; charset=utf-8", "not found"),
    }
}
