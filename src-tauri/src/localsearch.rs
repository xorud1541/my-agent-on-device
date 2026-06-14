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
use std::path::PathBuf;

/// LocalSearch 사이드카 기동 설정. config.rs 와 분리해 모듈을 자족적으로 유지한다
/// (호출부에서 AppConfig 로부터 채워 넣음).
#[derive(Debug, Clone)]
pub struct LocalSearchConfig {
    /// localsearch-cli 실행 파일 경로 (플랫폼별: win `.exe` / mac Mach-O)
    pub binary: PathBuf,
    /// harrier-v1-270m-onnx 의 부모 디렉토리
    pub models_dir: PathBuf,
    /// 색인 DB 디렉토리 (text.db 자동 생성)
    pub db_dir: PathBuf,
    /// libonnxruntime 동적 라이브러리 경로 → ORT_DYLIB_PATH 로 전달 (없으면 시스템 탐색)
    pub ort_dylib: Option<PathBuf>,
    pub port: u16,
}

/// `localsearch-cli` 의 serve 인자 배열. clap 글로벌 인자(--models-dir/--db-dir)를
/// 서브커맨드(serve) 앞에 둔다.
pub fn serve_args(cfg: &LocalSearchConfig) -> Vec<String> {
    vec![
        "--models-dir".into(),
        cfg.models_dir.to_string_lossy().into_owned(),
        "--db-dir".into(),
        cfg.db_dir.to_string_lossy().into_owned(),
        "serve".into(),
        "--port".into(),
        cfg.port.to_string(),
    ]
}

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

/// `localsearch-cli` 의 index 인자 배열. 인덱싱은 HTTP serve 가 아니라 별도 서브프로세스로
/// 수행한다(serve 에는 index 라우트가 없다). 같은 db_dir 를 공유한다.
pub fn index_args(cfg: &LocalSearchConfig, path: &str) -> Vec<String> {
    vec![
        "--models-dir".into(),
        cfg.models_dir.to_string_lossy().into_owned(),
        "--db-dir".into(),
        cfg.db_dir.to_string_lossy().into_owned(),
        "index".into(),
        path.into(),
    ]
}

/// index 서브프로세스 결과 요약.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSummary {
    pub indexed: u64,
    pub skipped: u64,
    pub errors: u64,
}

/// 마커 직후의 첫 정수를 파싱한다 (앞쪽 비숫자는 건너뛴다).
fn first_uint_after(s: &str, marker: &str) -> Option<u64> {
    let rest = s.split(marker).nth(1)?;
    let digits: String = rest
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// `index` 의 표준출력에서 "인덱싱 완료: N chunks, skipped=A, errors=B" 줄을 파싱한다.
pub fn parse_index_summary(stdout: &str) -> Option<IndexSummary> {
    let line = stdout.lines().rev().find(|l| l.contains("인덱싱 완료:"))?;
    Some(IndexSummary {
        indexed: first_uint_after(line, "완료:")?,
        skipped: first_uint_after(line, "skipped=")?,
        errors: first_uint_after(line, "errors=")?,
    })
}

/// `localsearch-cli index <path>` 를 동기로 실행하고 요약을 파싱한다 (블로킹).
pub fn run_index(cfg: &LocalSearchConfig, path: &str) -> Result<IndexSummary> {
    use anyhow::{bail, Context};
    if !cfg.binary.exists() {
        bail!("localsearch-cli 실행 파일이 없습니다: {}", cfg.binary.display());
    }
    let mut cmd = std::process::Command::new(&cfg.binary);
    cmd.args(index_args(cfg, path));
    if let Some(dylib) = &cfg.ort_dylib {
        cmd.env("ORT_DYLIB_PATH", dylib);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = cmd.output().context("localsearch-cli index 실행 실패")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_index_summary(&stdout).with_context(|| {
        format!(
            "인덱싱 결과를 해석하지 못했습니다. stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

impl LocalSearchConfig {
    /// 환경변수 + 기본 위치로 사이드카 설정을 해석한다.
    /// TODO(task3): config.rs(AppConfig) 정식 필드로 대체. 지금은 미구성 시 None.
    pub fn resolve_from_env() -> Option<Self> {
        let binary = PathBuf::from(std::env::var("LOCALSEARCH_CLI_BIN").ok()?);
        if !binary.exists() {
            return None;
        }
        let models_dir = PathBuf::from(std::env::var("LOCALSEARCH_MODELS_DIR").ok()?);
        let db_dir = default_index_db_dir();
        std::fs::create_dir_all(&db_dir).ok();
        Some(Self {
            binary,
            models_dir,
            db_dir,
            ort_dylib: std::env::var("ORT_DYLIB_PATH").ok().map(PathBuf::from),
            port: 11434,
        })
    }
}

/// 색인 DB 영속 위치 (워크스페이스와 무관하게 유지).
fn default_index_db_dir() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("local-agent")
        .join("localsearch")
}

/// LocalSearch 사이드카 프로세스 관리자 (llm::server::LlamaServer 패턴).
pub struct LocalSearchServer {
    child: Option<tokio::process::Child>,
    base_url: String,
}

impl Default for LocalSearchServer {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalSearchServer {
    pub fn new() -> Self {
        Self {
            child: None,
            base_url: String::new(),
        }
    }

    /// `localsearch-cli serve` 를 띄우고 /api/status 가 응답할 때까지 대기한다
    /// (Harrier 모델 로드 포함). 준비되면 사이드카를 가리키는 SearchClient 를 돌려준다.
    pub async fn start(&mut self, cfg: &LocalSearchConfig) -> anyhow::Result<SearchClient> {
        use anyhow::{bail, Context};
        use std::time::Duration;

        self.stop().await;
        if !cfg.binary.exists() {
            bail!("localsearch-cli 실행 파일이 없습니다: {}", cfg.binary.display());
        }

        let mut cmd = tokio::process::Command::new(&cfg.binary);
        cmd.args(serve_args(cfg));
        // ort load-dynamic 이 dlopen 할 onnxruntime 경로 (mac: Homebrew dylib 등)
        if let Some(dylib) = &cfg.ort_dylib {
            cmd.env("ORT_DYLIB_PATH", dylib);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        // 사이드카 출력은 로그 파일로 (문제 추적용)
        let log_path = crate::logging::log_dir().join("localsearch-server.log");
        if let Ok(log) = std::fs::File::create(&log_path) {
            if let Ok(log2) = log.try_clone() {
                cmd.stdout(std::process::Stdio::from(log));
                cmd.stderr(std::process::Stdio::from(log2));
            }
        }
        cmd.kill_on_drop(true);
        let child = cmd.spawn().context("localsearch-cli 실행 실패")?;
        self.child = Some(child);
        self.base_url = format!("http://127.0.0.1:{}", cfg.port);
        let client = SearchClient::new(self.base_url.clone());

        // 준비 대기 (최대 120초) — 모델 로드가 길 수 있다
        for _ in 0..120 {
            if let Some(child) = &mut self.child {
                if let Ok(Some(status)) = child.try_wait() {
                    bail!("localsearch-cli 가 즉시 종료됨 (exit: {status}). 로그: {}", log_path.display());
                }
            }
            if client.status().await.is_ok() {
                return Ok(client);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        self.stop().await;
        bail!("localsearch-cli 가 120초 내에 준비되지 않음")
    }

    pub async fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill().await;
        }
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

    /// 실제 바이너리로 사이드카를 띄워 status 까지 확인하는 e2e (기본 무시).
    /// 실행: 아래 환경변수 지정 후
    ///   LOCALSEARCH_CLI_BIN=<.../localsearch-cli> \
    ///   LOCALSEARCH_MODELS_DIR=<harrier 부모> \
    ///   ORT_DYLIB_PATH=/opt/homebrew/lib/libonnxruntime.dylib \
    ///   cargo test --lib localsearch::tests::e2e_sidecar_starts_and_responds -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn e2e_sidecar_starts_and_responds() {
        let bin = std::env::var("LOCALSEARCH_CLI_BIN").expect("LOCALSEARCH_CLI_BIN 필요");
        let models = std::env::var("LOCALSEARCH_MODELS_DIR").expect("LOCALSEARCH_MODELS_DIR 필요");
        let cfg = LocalSearchConfig {
            binary: bin.into(),
            models_dir: models.into(),
            db_dir: std::env::temp_dir().join("ls_e2e_db"),
            ort_dylib: std::env::var("ORT_DYLIB_PATH").ok().map(Into::into),
            port: 11237,
        };

        let mut server = LocalSearchServer::new();
        let client = server.start(&cfg).await.expect("사이드카 기동 실패");
        let count = client.indexed_count().await.expect("status 응답 실패");
        eprintln!("[e2e] indexed_count = {count}");
        server.stop().await;
    }

    /// 실제 바이너리로 폴더를 인덱싱하는 e2e (기본 무시). 환경변수는 사이드카 e2e 와 동일.
    #[test]
    #[ignore]
    fn e2e_run_index_indexes_a_folder() {
        let bin = std::env::var("LOCALSEARCH_CLI_BIN").expect("LOCALSEARCH_CLI_BIN 필요");
        let models = std::env::var("LOCALSEARCH_MODELS_DIR").expect("LOCALSEARCH_MODELS_DIR 필요");
        let docs = std::env::temp_dir().join("ls_e2e_index_docs");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::write(docs.join("a.md"), "# 휴가\n\n연차는 입사 1년 후 15일이다.").unwrap();

        let cfg = LocalSearchConfig {
            binary: bin.into(),
            models_dir: models.into(),
            db_dir: std::env::temp_dir().join("ls_e2e_index_db"),
            ort_dylib: std::env::var("ORT_DYLIB_PATH").ok().map(Into::into),
            port: 11238,
        };

        let s = run_index(&cfg, &docs.to_string_lossy()).expect("인덱싱 실패");
        eprintln!("[e2e] {s:?}");
        assert!(s.indexed >= 1);
    }

    #[test]
    fn index_args_places_index_subcommand_with_path() {
        let cfg = LocalSearchConfig {
            binary: "/v/ls".into(),
            models_dir: "/m".into(),
            db_dir: "/d".into(),
            ort_dylib: None,
            port: 11234,
        };

        let args = index_args(&cfg, "/docs/folder");

        assert_eq!(
            args,
            vec!["--models-dir", "/m", "--db-dir", "/d", "index", "/docs/folder"]
        );
    }

    #[test]
    fn parse_index_summary_extracts_counts() {
        let out = "  [ok] a.md → 1 chunks\n인덱싱 완료: 5 chunks, skipped=2, errors=1\n";

        let s = parse_index_summary(out).unwrap();

        assert_eq!(s.indexed, 5);
        assert_eq!(s.skipped, 2);
        assert_eq!(s.errors, 1);
    }

    #[test]
    fn parse_index_summary_is_none_without_summary_line() {
        assert!(parse_index_summary("진행 로그만 있고 완료줄 없음").is_none());
    }

    #[test]
    fn serve_args_orders_global_dirs_before_subcommand() {
        // clap 글로벌 인자(--models-dir/--db-dir)는 서브커맨드(serve) 앞에 둔다.
        let cfg = LocalSearchConfig {
            binary: "/v/localsearch-cli".into(),
            models_dir: "/m".into(),
            db_dir: "/d".into(),
            ort_dylib: Some("/lib/libonnxruntime.dylib".into()),
            port: 11234,
        };

        let args = serve_args(&cfg);

        assert_eq!(
            args,
            vec!["--models-dir", "/m", "--db-dir", "/d", "serve", "--port", "11234"]
        );
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
