use serde::{Deserialize, Serialize};

/// OpenAI 호환 chat 메시지. llama-server /v1/chat/completions 와 그대로 직렬화된다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// 첨부 이미지의 로컬(캐시) 경로. 세션에는 경로만 저장되고,
    /// llama-server 로 보낼 때만 base64 image_url 로 인라인된다(client.rs).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<String>>,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            images: None,
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
            images: None,
        }
    }
    pub fn assistant(content: Option<String>, tool_calls: Option<Vec<ToolCall>>) -> Self {
        Self {
            role: "assistant".into(),
            content,
            tool_calls,
            tool_call_id: None,
            images: None,
        }
    }
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            images: None,
        }
    }
    /// 첨부 이미지가 있는 사용자 메시지. content 텍스트 끝에 경로 마커를 붙여
    /// 모델이 경로 기반 도구(remove_background 등)를 호출할 수 있게 한다.
    pub fn user_with_images(text: impl Into<String>, images: Vec<String>) -> Self {
        let text = text.into();
        let marker = format!("\n\n[첨부 이미지: {}]", images.join(", "));
        Self {
            role: "user".into(),
            content: Some(format!("{text}{marker}")),
            tool_calls: None,
            tool_call_id: None,
            images: Some(images),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default = "default_tool_type")]
    pub call_type: String,
    pub function: FunctionCall,
}

fn default_tool_type() -> String {
    "function".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// JSON 문자열 (OpenAI 규격)
    pub arguments: String,
}

/// 프론트엔드로 흘리는 에이전트 이벤트. `agent-event` 채널 단일 페이로드.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentEvent {
    /// 모델의 추론(생각) 토큰 스트림
    ThinkingDelta {
        session_id: String,
        delta: String,
    },
    /// 사용자에게 보여줄 응답 토큰 스트림
    TextDelta {
        session_id: String,
        delta: String,
    },
    /// 모델이 도구 호출을 결정함
    ToolCallStart {
        session_id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    /// 도구 실행 완료
    ToolCallEnd {
        session_id: String,
        call_id: String,
        name: String,
        ok: bool,
        result: String,
    },
    /// 한 턴(사용자 발화 1회 처리) 종료
    TurnEnd {
        session_id: String,
        elapsed_ms: u64,
    },
    Error {
        session_id: String,
        message: String,
    },
    /// RAG 근거로 사용된 출처 문서 목록 (말풍선 하단 표시용)
    Sources {
        session_id: String,
        sources: Vec<String>,
    },
    /// llama-server 상태 변화 (loading | ready | down)
    ServerStatus {
        status: String,
        detail: String,
    },
    /// 로컬 검색 인덱싱/사이드카 상태 (indexing | ready | disabled | error)
    LocalsearchStatus {
        status: String,
        detail: String,
    },
    /// 설정 변경 방송 (워크스페이스/페르소나 등). UI ↔ 에이전트 루프 실시간 동기화용 —
    /// 설정 패널 저장이든 도구(set_workspace) 호출이든 같은 이벤트가 흐른다.
    ConfigChanged {
        config: crate::config::AppConfig,
    },
}

/// LLM 한 번 호출의 누적 결과
#[derive(Debug, Default, Clone)]
pub struct CompletionResult {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_roundtrip_through_json() {
        let m = ChatMessage::user_with_images("이거 설명해줘", vec!["/cache/a.png".into()]);
        let s = serde_json::to_string(&m).unwrap();
        let back: ChatMessage = serde_json::from_str(&s).unwrap();
        assert_eq!(back.images.as_deref(), Some(&["/cache/a.png".to_string()][..]));
        assert!(back.content.as_deref().unwrap().contains("/cache/a.png"));
    }

    #[test]
    fn text_only_message_has_no_images_field() {
        let m = ChatMessage::user("안녕");
        let s = serde_json::to_string(&m).unwrap();
        assert!(!s.contains("images"), "텍스트 전용 메시지는 images 키가 없어야 함: {s}");
    }

    #[test]
    fn user_with_images_embeds_path_marker() {
        let m = ChatMessage::user_with_images(
            "배경 제거해줘",
            vec!["/c/x.png".into(), "/c/y.png".into()],
        );
        let c = m.content.as_deref().unwrap();
        assert!(c.starts_with("배경 제거해줘"));
        assert!(c.contains("[첨부 이미지: /c/x.png, /c/y.png]"));
    }
}
