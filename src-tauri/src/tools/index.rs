use super::{req_str, Tool, ToolCtx};
use crate::localsearch::{run_index, LocalSearchConfig};
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::path::Path;

/// 폴더를 로컬 검색 색인에 추가하는 도구.
/// 인덱싱은 사이드카(serve)가 아니라 `localsearch-cli index` 서브프로세스로 수행한다.
pub struct IndexFolder;

impl Tool for IndexFolder {
    fn name(&self) -> &'static str {
        "index_folder"
    }

    fn description(&self) -> &'static str {
        "지정한 폴더의 문서를 로컬 검색 색인에 추가한다 (pdf/docx/txt/md/csv). \
         인덱싱 후 그 폴더 내용을 자연어로 검색할 수 있다. path 에 폴더 경로를 준다."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "인덱싱할 폴더 경로 (절대경로 권장)" }
            },
            "required": ["path"]
        })
    }

    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let raw = req_str(args, "path")?;
        // 상대경로면 워크스페이스 기준으로 해석 (읽기성 작업이라 워크스페이스 밖도 허용)
        let path = {
            let p = Path::new(raw);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                ctx.workspace().join(raw)
            }
        };
        if !path.is_dir() {
            return Err(anyhow!(
                "폴더가 아니거나 존재하지 않습니다: {}. 이 사실을 사용자에게 알리세요.",
                path.display()
            ));
        }

        let cfg = {
            let app = ctx.config.lock().unwrap();
            LocalSearchConfig::from_app(&app)
        }
        .ok_or_else(|| {
            anyhow!(
                "로컬 검색이 아직 구성되지 않았습니다 (검색 엔진 경로 미설정). \
                 이 사실을 사용자에게 그대로 알리세요."
            )
        })?;

        let s = run_index(&cfg, &path.to_string_lossy())?;
        Ok(format!(
            "인덱싱 완료: {} chunks (건너뜀 {}, 오류 {}). 이제 '{}' 내용을 검색할 수 있습니다.",
            s.indexed,
            s.skipped,
            s.errors,
            path.display()
        ))
    }
}
