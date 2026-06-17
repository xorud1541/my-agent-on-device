# collect-payload.ps1 — 단일 인스톨러용 payload 수집 (검토용 초안, 2026-06-15)
#
# 설계: docs/superpowers/specs/2026-06-15-packaging-single-installer-design.md (§6·§8)
# 역할: 흩어진 바이너리/모델을 installer/payload/{llama,localsearch,models} 로 모은다.
#       이후 local-agent.iss 의 [Files] 가 이 트리를 설치 루트로 복사.
#
# ⚠️ 이 스크립트는 Windows 빌드 머신에서 실행한다(현재 미검증 — macOS 개발 환경에서 작성).
#    소스 경로는 빌드 머신마다 다르므로 파라미터로 넘기거나 아래 기본값을 수정한다.
#
# 사용 예:
#   pwsh -File collect-payload.ps1 `
#     -LlamaDir   "C:\llama-bin-win-vulkan-x64" `
#     -LocalSearchCli "...\LocalSearch\src-tauri\target\release\localsearch-cli.exe" `
#     -PdfiumDll  "...\pdfium.dll" `
#     -ModelsSrc  "C:\models" `
#     -OnnxDll    "..\src-tauri\vendor\onnxruntime\onnxruntime.dll" `
#     -IncludeVision -IncludeRemoveBg   # 선택 기능 동봉 여부 (§9 결정 #2)

param(
  [string]$LlamaDir        = "",   # llama-server.exe + ggml-*.dll 폴더
  [string]$LocalSearchCli  = "",   # release localsearch-cli.exe 경로
  [string]$PdfiumDll       = "",   # pdfium.dll 경로 (검색의 PDF 색인용)
  [string]$OnnxDll         = "..\src-tauri\vendor\onnxruntime\onnxruntime.dll",
  [string]$ModelsSrc       = "",   # 모델 원본 폴더 (gguf/ort/harrier 들어있는 곳)
  [string]$MainModel       = "Qwen3.5-2B-Q4_K_M.gguf",
  [string]$MmprojModel     = "mmproj-Qwen3.5-2B-BF16.gguf",
  [string]$HarrierDir      = "harrier-v1-270m-onnx",
  [switch]$IncludeVision,          # mmproj(비전) 동봉 (~671MB)
  [switch]$IncludeRemoveBg,        # removeBG.ort 동봉 (~116MB)
  [string]$OutDir          = "$PSScriptRoot\payload"
)

$ErrorActionPreference = "Stop"

function Need($path, $what) {
  if ([string]::IsNullOrWhiteSpace($path) -or -not (Test-Path $path)) {
    throw "필수 경로 없음 ($what): '$path' — 파라미터로 지정하세요."
  }
}
function CopyInto($src, $destDir) {
  New-Item -ItemType Directory -Force -Path $destDir | Out-Null
  Copy-Item -Path $src -Destination $destDir -Recurse -Force
  Write-Host "  + $src -> $destDir"
}

Write-Host "[payload] 출력: $OutDir"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# 1) 추론 엔진 → payload/llama/
Need $LlamaDir "llama 빌드 폴더"
New-Item -ItemType Directory -Force -Path "$OutDir\llama" | Out-Null
Copy-Item "$LlamaDir\*" "$OutDir\llama\" -Recurse -Force
Write-Host "  + llama: $LlamaDir -> payload\llama"

# 2) 검색 사이드카 + pdfium → payload/localsearch/
Need $LocalSearchCli "localsearch-cli.exe"
Need $PdfiumDll "pdfium.dll"
CopyInto $LocalSearchCli "$OutDir\localsearch"
CopyInto $PdfiumDll      "$OutDir\localsearch"

# 3) onnxruntime.dll → payload\ (설치 시 exe 옆) — .iss 가 vendor 에서 직접 가져가도 됨
Need $OnnxDll "onnxruntime.dll"
CopyInto $OnnxDll $OutDir

# 4) 모델 → payload/models/
Need $ModelsSrc "모델 원본 폴더"
New-Item -ItemType Directory -Force -Path "$OutDir\models" | Out-Null
Need (Join-Path $ModelsSrc $MainModel) "메인 모델($MainModel)"
CopyInto (Join-Path $ModelsSrc $MainModel) "$OutDir\models"
Need (Join-Path $ModelsSrc $HarrierDir) "Harrier 임베딩 모델($HarrierDir)"
CopyInto (Join-Path $ModelsSrc $HarrierDir) "$OutDir\models"
if ($IncludeVision)   { CopyInto (Join-Path $ModelsSrc $MmprojModel) "$OutDir\models" }
if ($IncludeRemoveBg) { CopyInto (Join-Path $ModelsSrc "removeBG.ort") "$OutDir\models" }

# 요약 + 총 용량
$bytes = (Get-ChildItem $OutDir -Recurse -File | Measure-Object Length -Sum).Sum
Write-Host ("[payload] 완료. 총 {0:N1} GB (압축 전)" -f ($bytes / 1GB))
Write-Host "[payload] 다음: ISCC.exe local-agent.iss 로 인스톨러 빌드"
