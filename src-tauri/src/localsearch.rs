//! LocalSearch 사이드카(HTTP) 클라이언트.
//!
//! `team-util/LocalSearch` 의 `localsearch-cli serve --port N` 가 노출하는
//! HTTP API 를 호출한다. 계약(2026-06-14 feature/cli 기준):
//!   - GET  /api/status  → { loaded, harrier_dim, files, indexed_count }
//!   - POST /api/search  {query, top_k} → { results: [Hit...] }
//!
//! 설계: docs/superpowers/specs/2026-06-14-local-search-rag-design.md

use anyhow::Result;
use serde::Deserialize;

/// /api/search 결과 한 건. 사이드카는 더 많은 필드를 보내지만 RAG 에 쓰는 것만 받는다
/// (serde 는 미선언 필드를 무시한다).
#[derive(Debug, Clone, Deserialize)]
pub struct Hit {
    pub filename: String,
    #[serde(default)]
    pub heading: String,
    pub text: String,
    /// RRF 하이브리드 점수(스케일이 작다). RAG 게이트엔 부적합 — dense_cosine 사용.
    pub score: f32,
    /// 질의-청크 의미 유사도 [-1,1]. RAG 관련성 게이트 기준.
    #[serde(default)]
    pub dense_cosine: f32,
    #[serde(default)]
    pub file_path: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<Hit>,
}

/// /api/search 응답 본문(JSON)을 Hit 목록으로 파싱한다.
pub fn parse_search_response(body: &str) -> Result<Vec<Hit>> {
    let resp: SearchResponse = serde_json::from_str(body)?;
    Ok(resp.results)
}

/// /api/status 응답.
#[derive(Debug, Clone, Deserialize)]
pub struct StatusResponse {
    #[serde(default)]
    pub indexed_count: u64,
}

/// /api/status 응답 본문(JSON)을 파싱한다.
pub fn parse_status_response(body: &str) -> Result<StatusResponse> {
    Ok(serde_json::from_str(body)?)
}

/// LocalSearch 사이드카 HTTP 클라이언트.
#[derive(Clone)]
pub struct SearchClient {
    base_url: String,
    http: reqwest::Client,
}

impl SearchClient {
    /// `base_url` 예: "http://127.0.0.1:11234"
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    /// 의미 검색. 상위 `top_k` 청크를 점수 내림차순으로 반환한다.
    pub async fn search(&self, query: &str, top_k: u32) -> Result<Vec<Hit>> {
        let body = serde_json::json!({ "query": query, "top_k": top_k });
        let text = self
            .http
            .post(format!("{}/api/search", self.base_url))
            .json(&body)
            .send()
            .await?
            .text()
            .await?;
        parse_search_response(&text)
    }

    /// 사이드카 상태(인덱싱된 파일 수 등) 조회.
    pub async fn status(&self) -> Result<StatusResponse> {
        let text = self
            .http
            .get(format!("{}/api/status", self.base_url))
            .send()
            .await?
            .text()
            .await?;
        parse_status_response(&text)
    }

    /// 인덱싱된 파일 수. RAG 프리훅이 "색인 없음 → skip" 판단에 쓴다.
    pub async fn indexed_count(&self) -> Result<u64> {
        Ok(self.status().await?.indexed_count)
    }

    /// RAG 프리훅: 색인이 있으면 질의를 검색해 근거 블록을 만든다.
    /// None = 색인 없음 / 검색 실패 / 관련 문서 없음(임계값 미달) → 일반대화로 흘러감.
    /// 어떤 실패도 패닉/에러 전파 없이 None 으로 떨어진다(대화 경로는 항상 진행).
    pub async fn rag_context(&self, query: &str, top_k: u32, min_cosine: f32) -> Option<String> {
        if self.indexed_count().await.ok()? == 0 {
            return None;
        }
        let hits = self.search(query, top_k).await.ok()?;
        build_rag_context(&hits, min_cosine)
    }
}

/// 시스템 프롬프트에 합쳐지는 RAG 근거 블록의 시작 표지.
pub const RAG_MARKER: &str = "[참고 문서]";

/// 검색 히트에서 RAG 근거 블록을 만든다.
/// 최상위 히트의 dense_cosine 이 `min_cosine` 미만이거나 히트가 없으면 None
/// (= 관련 문서 없음 → 일반대화로 흘러감). 임계값을 넘는 청크만 포함한다.
pub fn build_rag_context(hits: &[Hit], min_cosine: f32) -> Option<String> {
    let top = hits.first()?;
    if top.dense_cosine < min_cosine {
        return None;
    }
    let body = hits
        .iter()
        .filter(|h| h.dense_cosine >= min_cosine)
        .enumerate()
        .map(|(i, h)| {
            format!(
                "[#{n} 문서: {file} / 섹션: {head}] (관련도 {cos:.3})\n{text}",
                n = i + 1,
                file = h.filename,
                head = h.heading,
                cos = h.dense_cosine,
                text = h.text,
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Some(format!(
        "{RAG_MARKER}\n{body}\n\n[지시] 위 문서가 사용자 질문과 의미상 관련될 때만 근거로 쓰고, \
         무관하면 완전히 무시한다. 문서에 없는 내용은 지어내지 말고 모른다고 답한다."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hits_from_search_response_envelope() {
        // /api/search 가 내보내는 실제 형태 (cli.rs handle_search). JSON 키 "text" 가
        // 내부 full_text 에서 매핑된다는 점이 계약의 핵심 — 키 이름을 틀리면 deser 실패.
        let json = r#"{"results":[
            {"filename":"보고서.pdf","heading":"3장 결론","text":"본문 내용입니다",
             "score":0.82,"doc_type":"analytical","file_path":"/docs/보고서.pdf",
             "file_hash":"abc","dense_cosine":0.5,"dense_rank":1,"bm25_rank":2,
             "lexical_boost":0.1}
        ]}"#;

        let hits = parse_search_response(json).unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].filename, "보고서.pdf");
        assert_eq!(hits[0].heading, "3장 결론");
        assert_eq!(hits[0].text, "본문 내용입니다");
        assert_eq!(hits[0].score, 0.82);
        assert_eq!(hits[0].file_path, "/docs/보고서.pdf");
    }

    #[test]
    fn hit_exposes_dense_cosine_for_rag_gating() {
        // RAG 게이트는 작은 RRF score 가 아니라 의미 유사도(dense_cosine)로 거른다
        // (2026-06-14 mac 빌드 실측: top 히트도 score≈0.04, dense_cosine≈0.36).
        let json = r#"{"results":[{"filename":"a.md","heading":"","text":"t",
                       "score":0.04,"dense_cosine":0.362}]}"#;

        let hits = parse_search_response(json).unwrap();

        assert!((hits[0].dense_cosine - 0.362).abs() < 1e-6);
    }

    #[test]
    fn parses_indexed_count_from_status_response() {
        let json = r#"{"loaded":true,"harrier_dim":768,"files":42,"indexed_count":42}"#;

        let status = parse_status_response(json).unwrap();

        assert_eq!(status.indexed_count, 42);
    }

    /// 고정 JSON 본문 하나만 응답하고 닫는 일회성 HTTP 서버. 반환된 주소로 클라이언트를 향하게 한다.
    async fn spawn_one_shot_json(body: &'static str) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf).await; // 요청 소비
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.as_bytes().len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        });
        addr
    }

    #[tokio::test]
    async fn search_posts_query_and_returns_hits() {
        let body = r#"{"results":[{"filename":"a.txt","heading":"h","text":"t","score":0.9,"file_path":"/a.txt"}]}"#;
        let addr = spawn_one_shot_json(body).await;

        let client = SearchClient::new(format!("http://{addr}"));
        let hits = client.search("질의", 3).await.unwrap();

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].filename, "a.txt");
        assert_eq!(hits[0].score, 0.9);
    }

    #[tokio::test]
    async fn status_returns_indexed_count() {
        let body = r#"{"loaded":true,"harrier_dim":640,"files":7,"indexed_count":7}"#;
        let addr = spawn_one_shot_json(body).await;

        let client = SearchClient::new(format!("http://{addr}"));
        let status = client.status().await.unwrap();

        assert_eq!(status.indexed_count, 7);
    }

    #[tokio::test]
    async fn indexed_count_is_zero_when_index_empty() {
        let body = r#"{"loaded":true,"harrier_dim":640,"files":0,"indexed_count":0}"#;
        let addr = spawn_one_shot_json(body).await;

        let client = SearchClient::new(format!("http://{addr}"));

        assert_eq!(client.indexed_count().await.unwrap(), 0);
    }

    fn hit(filename: &str, heading: &str, text: &str, score: f32, dense_cosine: f32) -> Hit {
        Hit {
            filename: filename.into(),
            heading: heading.into(),
            text: text.into(),
            score,
            dense_cosine,
            file_path: format!("/docs/{filename}"),
        }
    }

    #[test]
    fn rag_context_is_none_when_top_hit_below_cosine_threshold() {
        let hits = vec![hit("a.md", "", "내용", 0.04, 0.30)];

        assert!(build_rag_context(&hits, 0.45).is_none());
    }

    #[test]
    fn rag_context_is_none_when_no_hits() {
        assert!(build_rag_context(&[], 0.45).is_none());
    }

    /// 요청 경로(/api/status | /api/search)에 따라 응답을 골라주는 다회성 HTTP 서버.
    async fn spawn_router_server(
        status_body: &'static str,
        search_body: &'static str,
    ) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => break,
                };
                let mut buf = [0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let body = if req.contains("/api/status") { status_body } else { search_body };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.as_bytes().len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn rag_context_is_none_when_index_empty() {
        let addr = spawn_router_server(
            r#"{"indexed_count":0}"#,
            r#"{"results":[]}"#,
        )
        .await;
        let client = SearchClient::new(format!("http://{addr}"));

        assert!(client.rag_context("연차", 3, 0.45).await.is_none());
    }

    #[tokio::test]
    async fn rag_context_returns_block_for_relevant_index_hit() {
        let addr = spawn_router_server(
            r#"{"indexed_count":2}"#,
            r#"{"results":[{"filename":"hr.md","heading":"휴가","text":"연차 15일","score":0.05,"dense_cosine":0.62}]}"#,
        )
        .await;
        let client = SearchClient::new(format!("http://{addr}"));

        let ctx = client.rag_context("연차 며칠", 3, 0.45).await.unwrap();

        assert!(ctx.contains(RAG_MARKER));
        assert!(ctx.contains("hr.md"));
    }

    #[test]
    fn rag_context_includes_marker_and_relevant_hits_only() {
        let hits = vec![
            hit("hr.md", "휴가 정책", "연차는 15일", 0.05, 0.62),
            hit("net.md", "네트워크", "VPN 포트 443", 0.01, 0.20),
        ];

        let ctx = build_rag_context(&hits, 0.45).unwrap();

        assert!(ctx.contains(RAG_MARKER));
        assert!(ctx.contains("hr.md"));
        assert!(ctx.contains("연차는 15일"));
        assert!(!ctx.contains("net.md")); // 임계값 미달 청크는 제외
    }
}
