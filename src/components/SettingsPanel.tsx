import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { AppConfig, ModelEntry } from "../types";

interface Props {
  onClose: () => void;
}

function gb(bytes: number) {
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

export function SettingsPanel({ onClose }: Props) {
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [models, setModels] = useState<ModelEntry[]>([]);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    invoke<AppConfig>("get_config").then(setConfig);
    invoke<ModelEntry[]>("list_models").then(setModels);
  }, []);

  const save = async () => {
    if (!config) return;
    setSaving(true);
    try {
      await invoke("set_config", { newConfig: config });
      onClose();
    } catch (e) {
      alert(`설정 저장 실패: ${e}`);
    } finally {
      setSaving(false);
    }
  };

  if (!config) return null;

  return (
    <>
      <div className="settings-backdrop" onClick={onClose} />
      <aside className="settings">
        <h2>SETTINGS</h2>

        <label>모델 (~/.lmstudio/models)</label>
        <select
          value={config.model_path}
          onChange={(e) => setConfig({ ...config, model_path: e.target.value })}
        >
          {!models.some((m) => m.path === config.model_path) && (
            <option value={config.model_path}>{config.model_path}</option>
          )}
          {models.map((m) => (
            <option key={m.path} value={m.path}>
              {m.name} ({gb(m.size_bytes)})
            </option>
          ))}
        </select>

        <label>컨텍스트 길이</label>
        <input
          type="number"
          value={config.ctx_size}
          onChange={(e) => setConfig({ ...config, ctx_size: Number(e.target.value) })}
        />

        <label>Temperature</label>
        <input
          type="number"
          step="0.1"
          min="0"
          max="2"
          value={config.temperature}
          onChange={(e) => setConfig({ ...config, temperature: Number(e.target.value) })}
        />

        <label>호출당 최대 출력 토큰 (레이턴시 상한)</label>
        <input
          type="number"
          value={config.max_output_tokens}
          onChange={(e) => setConfig({ ...config, max_output_tokens: Number(e.target.value) })}
        />

        <label>턴당 최대 도구 호출</label>
        <input
          type="number"
          value={config.max_tool_rounds}
          onChange={(e) => setConfig({ ...config, max_tool_rounds: Number(e.target.value) })}
        />

        <label>디바이스</label>
        <input
          value={config.device}
          onChange={(e) => setConfig({ ...config, device: e.target.value })}
        />

        <div className="row-btns">
          <button className="btn-ghost" onClick={onClose}>
            취소
          </button>
          <button className="btn-primary" onClick={save} disabled={saving}>
            {saving ? "적용 중…" : "저장"}
          </button>
        </div>
        <p className="note">
          모델/디바이스/컨텍스트 변경 시 llama-server 가 재시작되며 모델을 다시 로드합니다 (수 초~수십 초).
        </p>
      </aside>
    </>
  );
}
