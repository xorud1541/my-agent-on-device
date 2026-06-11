use crate::models::ChatMessage;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 세션 목록에 보여줄 메타데이터 (파일에서 messages 를 뺀 것)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    /// 첫 사용자 발화에서 만든 제목
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
    /// 사용자 발화 수 (= 턴 수)
    pub turns: usize,
}

/// 디스크에 저장되는 세션 파일 전체
#[derive(Debug, Serialize, Deserialize)]
struct StoredSession {
    id: String,
    title: String,
    created_at: String,
    updated_at: String,
    messages: Vec<ChatMessage>,
}

/// 세션 영속 저장소. 세션당 JSON 파일 1개.
/// 기본 위치: %APPDATA%/com.estsoft.local-agent/sessions
pub struct SessionStore {
    dir: PathBuf,
}

impl SessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// 실사용 위치. LOCAL_AGENT_SESSIONS_DIR 환경변수로 재지정 가능(테스트 격리용).
    pub fn open_default() -> Self {
        if let Ok(dir) = std::env::var("LOCAL_AGENT_SESSIONS_DIR") {
            if !dir.trim().is_empty() {
                return Self::new(dir);
            }
        }
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        Self::new(base.join("com.estsoft.local-agent").join("sessions"))
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        // 세션 id 는 우리가 만든 uuid 뿐이지만, IPC 로 들어오는 값이므로 경로 탈출을 막는다
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("잘못된 세션 id: {id}");
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// 턴 종료마다 호출. 사용자 발화가 없는 세션(빈 새 대화)은 저장하지 않는다.
    pub fn save(&self, id: &str, messages: &[ChatMessage]) -> Result<()> {
        if !messages.iter().any(|m| m.role == "user") {
            return Ok(());
        }
        let path = self.path(id)?;
        std::fs::create_dir_all(&self.dir)?;
        let now = chrono::Local::now().to_rfc3339();
        // 기존 파일이 있으면 생성 시각을 보존한다
        let created_at = self
            .read_stored(id)
            .map(|s| s.created_at)
            .unwrap_or_else(|| now.clone());
        let stored = StoredSession {
            id: id.to_string(),
            title: title_from(messages),
            created_at,
            updated_at: now,
            messages: messages.to_vec(),
        };
        std::fs::write(&path, serde_json::to_string(&stored)?)
            .with_context(|| format!("세션 저장 실패: {}", path.display()))?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Option<Vec<ChatMessage>> {
        self.read_stored(id).map(|s| s.messages)
    }

    /// 최근 수정 순 세션 목록
    pub fn list(&self) -> Vec<SessionMeta> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut out: Vec<SessionMeta> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let text = std::fs::read_to_string(e.path()).ok()?;
                let s: StoredSession = serde_json::from_str(&text).ok()?;
                Some(SessionMeta {
                    turns: s.messages.iter().filter(|m| m.role == "user").count(),
                    id: s.id,
                    title: s.title,
                    created_at: s.created_at,
                    updated_at: s.updated_at,
                })
            })
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        let path = self.path(id)?;
        std::fs::remove_file(&path)
            .with_context(|| format!("세션 삭제 실패: {}", path.display()))?;
        Ok(())
    }

    fn read_stored(&self, id: &str) -> Option<StoredSession> {
        let text = std::fs::read_to_string(self.path(id).ok()?).ok()?;
        serde_json::from_str(&text).ok()
    }
}

/// 첫 사용자 발화 → 한 줄 제목 (공백 정리 + 48자 클립)
fn title_from(messages: &[ChatMessage]) -> String {
    let first = messages
        .iter()
        .find(|m| m.role == "user")
        .and_then(|m| m.content.as_deref())
        .unwrap_or("");
    let collapsed = first.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "새 대화".into();
    }
    let clipped: String = collapsed.chars().take(48).collect();
    if clipped.chars().count() < collapsed.chars().count() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SessionStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(tmp.path());
        (tmp, store)
    }

    fn turn(user: &str, answer: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage::system("시스템"),
            ChatMessage::user(user),
            ChatMessage::assistant(Some(answer.into()), None),
        ]
    }

    #[test]
    fn save_load_roundtrip_preserves_messages() {
        let (_tmp, store) = store();
        let messages = turn("파일 찾아줘", "찾았습니다");
        store.save("abc-123", &messages).unwrap();

        let loaded = store.load("abc-123").unwrap();
        assert_eq!(loaded.len(), 3);
        assert_eq!(loaded[1].content.as_deref(), Some("파일 찾아줘"));
        assert_eq!(loaded[2].content.as_deref(), Some("찾았습니다"));
    }

    #[test]
    fn empty_session_without_user_message_is_not_saved() {
        let (_tmp, store) = store();
        store
            .save("empty-1", &[ChatMessage::system("시스템")])
            .unwrap();
        assert!(store.load("empty-1").is_none());
        assert!(store.list().is_empty());
    }

    #[test]
    fn list_returns_meta_sorted_by_updated_desc() {
        let (_tmp, store) = store();
        store.save("first-1", &turn("첫 번째 질문", "답1")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(15));
        store
            .save("second-2", &turn("두 번째 질문", "답2"))
            .unwrap();

        let metas = store.list();
        assert_eq!(metas.len(), 2);
        assert_eq!(metas[0].id, "second-2", "최근 수정이 먼저");
        assert_eq!(metas[0].title, "두 번째 질문");
        assert_eq!(metas[0].turns, 1);
    }

    #[test]
    fn resave_preserves_created_at_and_updates_title_count() {
        let (_tmp, store) = store();
        let mut messages = turn("처음 질문", "답");
        store.save("s-1", &messages).unwrap();
        let created = store.list()[0].created_at.clone();

        std::thread::sleep(std::time::Duration::from_millis(15));
        messages.push(ChatMessage::user("추가 질문"));
        messages.push(ChatMessage::assistant(Some("추가 답".into()), None));
        store.save("s-1", &messages).unwrap();

        let meta = &store.list()[0];
        assert_eq!(meta.created_at, created, "생성 시각은 보존");
        assert!(meta.updated_at > meta.created_at);
        assert_eq!(meta.turns, 2);
    }

    #[test]
    fn delete_removes_session() {
        let (_tmp, store) = store();
        store.save("gone-1", &turn("질문", "답")).unwrap();
        store.delete("gone-1").unwrap();
        assert!(store.load("gone-1").is_none());
    }

    #[test]
    fn path_traversal_ids_are_rejected() {
        let (_tmp, store) = store();
        assert!(store.save("../evil", &turn("q", "a")).is_err());
        assert!(store.load("..\\evil").is_none());
        assert!(store.delete("a/b").is_err());
    }

    #[test]
    fn title_is_collapsed_and_clipped() {
        let long = format!("아주   긴\n제목 {}", "가".repeat(100));
        let msgs = vec![ChatMessage::user(long)];
        let title = title_from(&msgs);
        assert!(
            title.starts_with("아주 긴 제목"),
            "공백/줄바꿈 정리: {title}"
        );
        assert!(title.chars().count() <= 49, "48자 + 말줄임");
        assert!(title.ends_with('…'));
    }
}
