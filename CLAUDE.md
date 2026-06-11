# Local Agent

## 기술 스택
- **Frontend**: React 19 + TypeScript + Vite
- **Backend**: Rust + Tauri 2
- **Package Manager**: pnpm
- **Build Target**: Windows (NSIS)

## 프로젝트 구조
```
src/              → React 프론트엔드
src-tauri/        → Rust 백엔드
src-tauri/src/    → Tauri 커맨드, 모델
skills/           → 개발 스킬
```

## 개발 명령어
```bash
pnpm tauri dev        # 개발 서버 + Tauri 창
pnpm tauri build      # 릴리즈 빌드 (NSIS 설치파일)
```

## 규칙
- 스킬을 추가/삭제/이름변경할 때 README.md도 같이 업데이트할 것
- Tauri 커맨드는 `src-tauri/src/commands.rs`에 정의
- 프론트엔드↔백엔드 통신은 `invoke()` 사용
