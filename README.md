# Local Agent

CPU + iGPU 환경에서 동작하는 로컬 온디바이스 LLM 에이전트 데스크톱 앱.
llama.cpp(Vulkan)를 사이드카로 구동하고, 모델이 도구(파일 제어/검색, 이미지 처리, PDF, 화면 캡처)를
스스로 호출해 사용자 요청을 수행한다. Tauri 2 + React 19 + TypeScript.

## 시작하기

### 요구 사항
- [Rust](https://rustup.rs/) 1.77.2+
- [Node.js](https://nodejs.org/) 18+
- [pnpm](https://pnpm.io/) 9+

### 설치 및 실행
```bash
pnpm install
pnpm tauri dev
```

### 빌드
```bash
pnpm tauri build
```

## 프로젝트 구조

```
src/                  React 프론트엔드
├── App.tsx           메인 앱 컴포넌트
├── main.tsx          엔트리 포인트
├── types.ts          공통 타입
└── styles/           스타일시트

src-tauri/            Rust 백엔드
├── src/
│   ├── lib.rs        Tauri 앱 설정
│   ├── commands.rs   IPC 커맨드
│   └── models.rs     데이터 모델
└── tauri.conf.json   Tauri 설정

skills/               개발 스킬
└── experimental/     실험적 스킬
```
