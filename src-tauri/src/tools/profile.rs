use super::{opt_str, Tool, ToolCtx};
use anyhow::{bail, Result};
use serde_json::{json, Value};

pub struct UpdateProfile;

impl Tool for UpdateProfile {
    fn name(&self) -> &'static str {
        "update_profile"
    }
    fn description(&self) -> &'static str {
        "대화에서 알게 된 사용자 이름 또는 나(에이전트)의 이름을 저장한다. 이름을 새로 알게 되거나 바뀌면 즉시 호출."
    }
    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "user_name": { "type": "string", "description": "사용자의 이름/호칭" },
                "agent_name": { "type": "string", "description": "사용자가 지어준 나의 이름" }
            },
            "required": []
        })
    }
    fn execute(&self, args: &Value, ctx: &ToolCtx) -> Result<String> {
        let user = opt_str(args, "user_name")
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let agent = opt_str(args, "agent_name")
            .map(str::trim)
            .filter(|s| !s.is_empty());
        if user.is_none() && agent.is_none() {
            bail!("user_name 또는 agent_name 중 하나는 필요함");
        }
        ctx.update_config(|cfg| {
            if let Some(u) = user {
                cfg.user_name = u.to_string();
            }
            if let Some(a) = agent {
                cfg.agent_name = a.to_string();
            }
        })?;
        let mut saved = Vec::new();
        if let Some(u) = user {
            saved.push(format!("사용자 이름 '{u}'"));
        }
        if let Some(a) = agent {
            saved.push(format!("내 이름 '{a}'"));
        }
        Ok(format!(
            "{} 저장 완료. 앞으로 이 이름을 기억한다.",
            saved.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::tools::ToolCtx;

    #[test]
    fn saves_both_names() {
        let ctx = ToolCtx::noop(AppConfig::default());
        let out = UpdateProfile
            .execute(&json!({"user_name": "태경", "agent_name": "앨리"}), &ctx)
            .unwrap();
        assert!(out.contains("저장 완료"), "{out}");
        let cfg = ctx.config.lock().unwrap();
        assert_eq!(cfg.user_name, "태경");
        assert_eq!(cfg.agent_name, "앨리");
    }

    #[test]
    fn partial_update_keeps_other_name() {
        let mut cfg = AppConfig::default();
        cfg.agent_name = "앨리".into();
        let ctx = ToolCtx::noop(cfg);
        UpdateProfile
            .execute(&json!({"user_name": "태경"}), &ctx)
            .unwrap();
        let cfg = ctx.config.lock().unwrap();
        assert_eq!(cfg.agent_name, "앨리");
        assert_eq!(cfg.user_name, "태경");
    }

    #[test]
    fn rejects_empty_args() {
        let ctx = ToolCtx::noop(AppConfig::default());
        assert!(UpdateProfile.execute(&json!({}), &ctx).is_err());
        assert!(UpdateProfile
            .execute(&json!({"user_name": "  "}), &ctx)
            .is_err());
    }
}
