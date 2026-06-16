//! 자체 영역 캡처 오버레이 — 별도 프로세스 헬퍼.
//!
//! 사용: `region-capture <out.png> <hint_x> <hint_y>`
//! 흐름: 힌트 좌표(논리 px)가 속한 모니터를 xcap 으로 캡처 → 그 모니터를 덮는
//! 풀스크린 네이티브 창(winit+softbuffer)에 캡처본을 어둡게 깔고 → 드래그로 영역 선택
//! → 원본 해상도에서 크롭해 <out.png> 저장 후 종료. Esc/우클릭 = 저장 없이 종료(취소).
//!
//! webview 를 전혀 쓰지 않는 순수 네이티브 창이며(과거 2번째 webview 창은 macOS WebKit
//! 크래시를 유발), 별도 프로세스라 어떤 실패도 본 앱과 격리된다. winit/softbuffer/xcap
//! 모두 크로스플랫폼이라 macOS·Windows 단일 코드패스다.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::num::NonZeroU32;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};

/// 음영 밝기 (255 = 원본). 140 ≈ 55% — "살짝 어두운" 느낌.
const DIM: u32 = 140;
/// 선택 테두리 색 (앱 액센트 amber 계열, 0RGB)
const BORDER: u32 = 0x00E8A33D;
/// 이보다 작은 드래그는 무시(오클릭 방지)
const MIN_SEL: f64 = 5.0;

struct MonitorRect {
    /// 논리 좌표(글로벌) 기준 모니터 원점/크기 — winit 창 배치와 좌표 매핑의 기준
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

struct App {
    out_path: std::path::PathBuf,
    src: image::RgbaImage, // 원본(물리 해상도) — 크롭은 항상 여기서
    mon: MonitorRect,
    window: Option<Arc<Window>>,
    surface: Option<softbuffer::Surface<Arc<Window>, Arc<Window>>>,
    /// 창 크기로 변환된 어두운/밝은 프레임 (행 단위 복사로 드래그 리렌더가 싸다)
    dimmed: Vec<u32>,
    bright: Vec<u32>,
    buf_size: (u32, u32),
    drag_start: Option<(f64, f64)>,
    cursor: (f64, f64),
    saved: bool,
}

impl App {
    /// 창 픽셀 (px,py) → 원본 이미지 픽셀 좌표로 변환할 오프셋/배율을 구한다.
    /// 창은 모니터의 물리 해상도(PhysicalSize)로 생성되고 캡처본(src)도 물리 해상도라
    /// 둘은 1:1 이다. 따라서 배율(scale_factor) 재적용 없이, 창이 모니터를 다 못 덮어
    /// 생기는 물리 위치 차이만 보정하면 된다. (HiDPI 배율 이중 적용 문제를 원천 차단)
    fn mapping(&self, window: &Window) -> (f64, f64, f64) {
        let pos = window
            .inner_position()
            .map(|p| (p.x as f64, p.y as f64))
            .unwrap_or((self.mon.x, self.mon.y));
        let off_x = pos.0 - self.mon.x;
        let off_y = pos.1 - self.mon.y;
        (off_x, off_y, 1.0)
    }

    /// 창 크기에 맞춘 dimmed/bright 프레임 생성 (오버레이 열릴 때 1회)
    fn build_frames(&mut self, ww: u32, wh: u32) {
        if self.buf_size == (ww, wh) && !self.dimmed.is_empty() {
            return;
        }
        let window = self.window.as_ref().expect("window").clone();
        let (off_x, off_y, px_scale) = self.mapping(&window);
        let (iw, ih) = (self.src.width() as i64, self.src.height() as i64);
        let raw = self.src.as_raw();
        let n = (ww as usize) * (wh as usize);
        let mut bright = vec![0u32; n];
        let mut dimmed = vec![0u32; n];
        for y in 0..wh as usize {
            let sy = (off_y + y as f64 * px_scale) as i64;
            let sy = sy.clamp(0, ih - 1) as usize;
            let row = y * ww as usize;
            for x in 0..ww as usize {
                let sx = (off_x + x as f64 * px_scale) as i64;
                let sx = sx.clamp(0, iw - 1) as usize;
                let p = (sy * iw as usize + sx) * 4;
                let (r, g, b) = (raw[p] as u32, raw[p + 1] as u32, raw[p + 2] as u32);
                bright[row + x] = (r << 16) | (g << 8) | b;
                dimmed[row + x] =
                    (((r * DIM) >> 8) << 16) | (((g * DIM) >> 8) << 8) | ((b * DIM) >> 8);
            }
        }
        self.bright = bright;
        self.dimmed = dimmed;
        self.buf_size = (ww, wh);
    }

    fn sel_rect(&self) -> Option<(usize, usize, usize, usize)> {
        let (sx, sy) = self.drag_start?;
        let (cx, cy) = self.cursor;
        let x0 = sx.min(cx).max(0.0) as usize;
        let y0 = sy.min(cy).max(0.0) as usize;
        let x1 = sx.max(cx) as usize;
        let y1 = sy.max(cy) as usize;
        Some((x0, y0, x1, y1))
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return;
        };
        self.build_frames(size.width, size.height);
        let sel = self.sel_rect(); // surface 가변 차용 전에 계산 (차용 충돌 방지)
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = surface.buffer_mut() else {
            return;
        };
        buffer.copy_from_slice(&self.dimmed);

        if let Some((x0, y0, x1, y1)) = sel {
            let ww = size.width as usize;
            let wh = size.height as usize;
            let (x0, y0) = (x0.min(ww - 1), y0.min(wh - 1));
            let (x1, y1) = (x1.min(ww - 1), y1.min(wh - 1));
            // 선택 영역은 원본 밝기로
            for y in y0..=y1 {
                let row = y * ww;
                buffer[row + x0..row + x1 + 1]
                    .copy_from_slice(&self.bright[row + x0..row + x1 + 1]);
            }
            // 테두리 2px
            for t in 0..2usize {
                let (ty, by) = ((y0 + t).min(wh - 1), y1.saturating_sub(t));
                for x in x0..=x1 {
                    buffer[ty * ww + x] = BORDER;
                    buffer[by * ww + x] = BORDER;
                }
                let (lx, rx) = ((x0 + t).min(ww - 1), x1.saturating_sub(t));
                for y in y0..=y1 {
                    buffer[y * ww + lx] = BORDER;
                    buffer[y * ww + rx] = BORDER;
                }
            }
        }
        let _ = buffer.present();
    }

    /// 드래그 종료: 선택 영역을 원본에서 크롭해 저장
    fn finish(&mut self, event_loop: &ActiveEventLoop) {
        let Some((x0, y0, x1, y1)) = self.sel_rect() else {
            return;
        };
        self.drag_start = None;
        if ((x1 - x0) as f64) < MIN_SEL || ((y1 - y0) as f64) < MIN_SEL {
            self.redraw();
            return; // 너무 작은 선택 — 다시 시도하게 둔다
        }
        let window = self.window.as_ref().expect("window").clone();
        let (off_x, off_y, px_scale) = self.mapping(&window);
        let (iw, ih) = (self.src.width() as f64, self.src.height() as f64);
        let ix0 = (off_x + x0 as f64 * px_scale).clamp(0.0, iw - 1.0) as u32;
        let iy0 = (off_y + y0 as f64 * px_scale).clamp(0.0, ih - 1.0) as u32;
        let ix1 = (off_x + x1 as f64 * px_scale).clamp(0.0, iw) as u32;
        let iy1 = (off_y + y1 as f64 * px_scale).clamp(0.0, ih) as u32;
        let (cw, ch) = ((ix1 - ix0).max(1), (iy1 - iy0).max(1));
        let cropped = image::imageops::crop_imm(&self.src, ix0, iy0, cw, ch).to_image();
        if cropped.save(&self.out_path).is_ok() {
            self.saved = true;
        }
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        event_loop.set_control_flow(ControlFlow::Wait);
        let attrs = Window::default_attributes()
            .with_title("영역 캡처")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            // xcap 가 주는 모니터 좌표/크기는 물리 픽셀이므로 Physical 로 그대로 넘긴다.
            // (Logical 로 넘기면 HiDPI 에서 winit 이 scale_factor 를 재적용해 창이 배율²
            //  만큼 커지고 버퍼 할당이 폭증한다 — 150% 4K 에서 5760×3240 OOM)
            .with_position(winit::dpi::Position::Physical(PhysicalPosition::new(
                self.mon.x as i32,
                self.mon.y as i32,
            )))
            .with_inner_size(winit::dpi::Size::Physical(PhysicalSize::new(
                self.mon.w as u32,
                self.mon.h as u32,
            )));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(_) => {
                event_loop.exit();
                return;
            }
        };
        window.set_cursor(CursorIcon::Crosshair);
        window.focus_window();
        let Ok(context) = softbuffer::Context::new(window.clone()) else {
            event_loop.exit();
            return;
        };
        let Ok(surface) = softbuffer::Surface::new(&context, window.clone()) else {
            event_loop.exit();
            return;
        };
        self.surface = Some(surface);
        self.window = Some(window.clone());
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                self.buf_size = (0, 0); // 프레임 재생성
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if self.drag_start.is_some() {
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Left, ElementState::Pressed) => {
                    self.drag_start = Some(self.cursor);
                }
                (MouseButton::Left, ElementState::Released) => self.finish(event_loop),
                (MouseButton::Right, ElementState::Pressed) => event_loop.exit(), // 취소
                _ => {}
            },
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && event.logical_key == Key::Named(NamedKey::Escape)
                {
                    event_loop.exit(); // 취소
                }
            }
            WindowEvent::CloseRequested => event_loop.exit(),
            _ => {}
        }
    }
}

fn main() {
    std::process::exit(run());
}

fn run() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: region-capture <out.png> <hint_x> <hint_y>");
        return 2;
    }
    let out_path = std::path::PathBuf::from(&args[1]);
    let hint_x: i32 = args[2].parse().unwrap_or(0);
    let hint_y: i32 = args[3].parse().unwrap_or(0);

    // 1) 힌트가 속한 모니터 캡처 (창 생성 전 — 오버레이 자신이 찍히지 않게)
    let monitor = xcap::Monitor::from_point(hint_x, hint_y)
        .ok()
        .or_else(|| xcap::Monitor::all().ok().and_then(|m| m.into_iter().next()));
    let Some(monitor) = monitor else {
        eprintln!("모니터를 찾지 못함");
        return 3;
    };
    let mon = MonitorRect {
        x: monitor.x().unwrap_or(0) as f64,
        y: monitor.y().unwrap_or(0) as f64,
        w: monitor.width().unwrap_or(0) as f64,
        h: monitor.height().unwrap_or(0) as f64,
    };
    let captured = match monitor.capture_image() {
        Ok(img) => img,
        Err(e) => {
            eprintln!("캡처 실패: {e}");
            return 3;
        }
    };
    let (cw, ch) = (captured.width(), captured.height());
    let Some(src) = image::RgbaImage::from_raw(cw, ch, captured.into_raw()) else {
        eprintln!("캡처 버퍼 변환 실패");
        return 3;
    };

    // 2) 오버레이 이벤트 루프
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // 독 아이콘 없이 키 입력을 받는 보조 앱으로
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = match builder.build() {
        Ok(el) => el,
        Err(e) => {
            eprintln!("이벤트 루프 생성 실패: {e}");
            return 3;
        }
    };

    let mut app = App {
        out_path,
        src,
        mon,
        window: None,
        surface: None,
        dimmed: Vec::new(),
        bright: Vec::new(),
        buf_size: (0, 0),
        drag_start: None,
        cursor: (0.0, 0.0),
        saved: false,
    };
    if event_loop.run_app(&mut app).is_err() {
        return 3;
    }
    if app.saved {
        0
    } else {
        1 // 취소
    }
}
