#!/usr/bin/env bash
# 후보 모델별 한국어 툴콜 정확도 + 실측 레이턴시 테스트.
# 사용법: ./toolcall_test.sh <model.gguf> <label>
set -u
LLAMA_DIR="/c/Users/EST/Downloads/llama-b9334-bin-win-vulkan-x64"
PORT=8737
MODEL="$1"
LABEL="$2"
DIR="$(cd "$(dirname "$0")" && pwd)"
TOOLS=$(cat "$DIR/tools.json")

"$LLAMA_DIR/llama-server.exe" -m "$MODEL" --port $PORT -ngl 99 --device Vulkan0 \
  -c 8192 --jinja --no-webui >/tmp/llama_$LABEL.log 2>&1 &
SRV=$!
trap "kill $SRV 2>/dev/null" EXIT

for i in $(seq 1 60); do
  curl -s -o /dev/null "http://127.0.0.1:$PORT/health" && break
  sleep 1
done
echo "### $LABEL ready after ${i}s model load"

SYS="너는 사용자의 PC에서 동작하는 로컬 에이전트다. 사용자의 요청을 수행하기 위해 필요하면 도구를 호출한다. 도구가 필요한 작업이면 반드시 도구를 호출하고, 잡담에는 도구 없이 한국어로 답한다."

run_case() {
  local name="$1"; local user="$2"; local expect="$3"
  local body
  body=$(python3 - "$SYS" "$user" "$TOOLS" <<'EOF'
import json, sys
sys.stdout.write(json.dumps({
  "model": "default",
  "messages": [{"role":"system","content":sys.argv[1]},{"role":"user","content":sys.argv[2]}],
  "tools": json.loads(sys.argv[3]),
  "tool_choice": "auto",
  "temperature": 0.2,
  "max_tokens": 1024
}))
EOF
)
  local t0=$(date +%s.%N)
  local resp=$(curl -s "http://127.0.0.1:$PORT/v1/chat/completions" -H "Content-Type: application/json" -d "$body")
  local t1=$(date +%s.%N)
  local elapsed=$(python3 -c "print(f'{$t1-$t0:.1f}')")
  local got=$(echo "$resp" | python3 -c "
import json,sys
try:
    r=json.load(sys.stdin)
    m=r['choices'][0]['message']
    tcs=m.get('tool_calls') or []
    if tcs:
        print(';'.join(t['function']['name']+' '+t['function']['arguments'].replace(chr(10),' ') for t in tcs))
    else:
        print('NO_TOOL: '+(m.get('content') or '')[:120].replace(chr(10),' '))
except Exception as e:
    print('PARSE_ERR', e)
")
  local mark="FAIL"
  case "$got" in "$expect"*) mark="PASS";; esac
  if [ "$expect" = "NO_TOOL" ]; then case "$got" in NO_TOOL*) mark="PASS";; esac; fi
  echo "[$mark] ${elapsed}s | $name | expect=$expect | got=$got"
}

run_case "파일검색" "내 다운로드 폴더(C:\\Users\\EST\\Downloads)에서 png 이미지들 찾아줘" "search_files"
run_case "PDF" "C:\\docs\\계약서.pdf 내용 요약해줘" "pdf_extract_text"
run_case "캡처" "지금 화면 캡처 좀 해줘" "screen_capture"
run_case "리사이즈" "C:\\img\\photo.jpg 를 가로 800픽셀로 줄여줘" "image_transform"
run_case "잡담" "오늘 기분 어때?" "NO_TOOL"

kill $SRV 2>/dev/null
wait $SRV 2>/dev/null
echo "### $LABEL done"
