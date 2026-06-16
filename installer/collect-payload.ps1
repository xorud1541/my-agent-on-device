<#
.SYNOPSIS
  흩어진 사이드카 바이너리·모델을 installer\payload\ 트리로 모은다.

.DESCRIPTION
  단일 동봉 InnoSetup(local-agent.iss)이 복사할 payload 트리를 구성한다.
    payload\llama\        llama-server.exe + ggml/llama *.dll (Vulkan)
    payload\localsearch\  localsearch-cli.exe + pdfium.dll
    payload\models\       Qwen GGUF (+mmproj), removeBG.ort, harrier-v1-270m-onnx\

  앱(local-agent.exe / region-capture.exe)과 onnxruntime.dll 은 빌드 산출물/vendor 에서
  .iss 가 직접 가져가므로 여기서 다루지 않는다.

  소스 경로는 이 개발 PC 기준 기본값. 다른 머신이면 -파라미터로 덮어쓴다.
  필수 소스가 없으면 즉시 에러로 중단한다(조용한 누락 방지).

.EXAMPLE
  pwsh -File installer\collect-payload.ps1
  pwsh -File installer\collect-payload.ps1 -LlamaDir D:\llama -Force
#>
[CmdletBinding()]
param(
  [string]$LlamaDir       = "$HOME\Downloads\llama-b9334-bin-win-vulkan-x64",
  [string]$ModelDir       = "$HOME\.lmstudio\models\lmstudio-community\Qwen3.5-2B-GGUF",
  [string]$AliceModels    = "$HOME\.alice\models",
  [string]$LocalSearchCli = "$env:LOCALAPPDATA\alian\localsearch-cli.exe",
  [string]$Pdfium         = "$env:LOCALAPPDATA\pdf2md\pdfium-7690\pdfium.dll",
  # 비전(mmproj) 동봉 여부. 기본 포함(전부 동봉 결정).
  [switch]$NoVision,
  # 기존 payload\ 를 지우고 새로 구성.
  [switch]$Force
)

$ErrorActionPreference = 'Stop'
$payload = Join-Path $PSScriptRoot 'payload'

function Require-Path($p, $what) {
  if (-not (Test-Path -LiteralPath $p)) {
    throw "필수 소스를 찾을 수 없습니다 ($what): $p"
  }
}

function Copy-Into($src, $destDir, $what) {
  Require-Path $src $what
  New-Item -ItemType Directory -Force -Path $destDir | Out-Null
  Copy-Item -LiteralPath $src -Destination $destDir -Force
  Write-Host ("  + {0}  ->  {1}" -f (Split-Path $src -Leaf), $destDir)
}

# --- payload 초기화 ---------------------------------------------------------
if ($Force -and (Test-Path $payload)) { Remove-Item -Recurse -Force $payload }
New-Item -ItemType Directory -Force -Path $payload | Out-Null

# --- 1) llama: server + 모든 dll (CPU 변형은 런타임 선택이라 전부 포함) -------
Write-Host "[1/3] llama 추론 엔진 수집..."
$llamaOut = Join-Path $payload 'llama'
New-Item -ItemType Directory -Force -Path $llamaOut | Out-Null
Copy-Into (Join-Path $LlamaDir 'llama-server.exe') $llamaOut 'llama-server.exe'
$dlls = Get-ChildItem -LiteralPath $LlamaDir -Filter '*.dll' -File
if (-not $dlls) { throw "llama dll 을 찾을 수 없습니다: $LlamaDir\*.dll" }
$dlls | ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination $llamaOut -Force }
Write-Host ("  + {0} dll" -f $dlls.Count)

# --- 2) localsearch: cli + pdfium -------------------------------------------
Write-Host "[2/3] localsearch 사이드카 수집..."
$lsOut = Join-Path $payload 'localsearch'
Copy-Into $LocalSearchCli $lsOut 'localsearch-cli.exe'
Copy-Into $Pdfium         $lsOut 'pdfium.dll'

# --- 3) models --------------------------------------------------------------
Write-Host "[3/3] 모델 수집..."
$modelsOut = Join-Path $payload 'models'
New-Item -ItemType Directory -Force -Path $modelsOut | Out-Null
Copy-Into (Join-Path $ModelDir 'Qwen3.5-2B-Q4_K_M.gguf') $modelsOut '메인 모델 gguf'
if (-not $NoVision) {
  Copy-Into (Join-Path $ModelDir 'mmproj-Qwen3.5-2B-BF16.gguf') $modelsOut '비전 mmproj'
} else {
  Write-Host "  (비전 mmproj 생략 — -NoVision)"
}
Copy-Into (Join-Path $AliceModels 'removeBG.ort') $modelsOut '배경제거 모델'

# harrier 임베딩 모델 폴더 통째 복사
$harrierSrc = Join-Path $AliceModels 'harrier-v1-270m-onnx'
Require-Path $harrierSrc 'harrier-v1-270m-onnx'
$harrierDst = Join-Path $modelsOut 'harrier-v1-270m-onnx'
if (Test-Path $harrierDst) { Remove-Item -Recurse -Force $harrierDst }
Copy-Item -LiteralPath $harrierSrc -Destination $modelsOut -Recurse -Force
Write-Host "  + harrier-v1-270m-onnx\ (재귀)"

# --- 요약 -------------------------------------------------------------------
$size = (Get-ChildItem -Recurse -File $payload | Measure-Object Length -Sum).Sum
Write-Host ""
Write-Host ("payload 구성 완료: {0}  ({1:N1} GB)" -f $payload, ($size / 1GB))
