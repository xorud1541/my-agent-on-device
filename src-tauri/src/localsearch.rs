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
    pub score: f32,
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
}
