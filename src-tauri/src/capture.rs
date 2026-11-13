use std::time::Duration;

use windows::core::{factory, Interface};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, HWND, POINT, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, GetWindowRect};

#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    fn is_uniform(&self) -> bool {
        let Some(first) = self.rgba.chunks_exact(4).next() else {
            return true;
        };
        self.rgba
            .chunks_exact(4)
            .step_by(101)
            .all(|pixel| pixel == first)
    }
}

pub struct Capturer {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    _item: GraphicsCaptureItem,
}

fn hwnd_from(raw: i64) -> HWND {
    HWND(raw as usize as *mut core::ffi::c_void)
}

impl Capturer {
    pub fn attach(hwnd: i64) -> Result<Self, String> {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let levels = [D3D_FEATURE_LEVEL_11_0];

        unsafe {
            D3D11CreateDevice(
                None::<&IDXGIAdapter>,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&levels),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(|e| e.to_string())?;
        }

        let device = device.ok_or("D3D11 device was not created")?;
        let context = context.ok_or("D3D11 context was not created")?;

        let dxgi: IDXGIDevice = device.cast().map_err(|e| e.to_string())?;
        let inspectable =
            unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi) }.map_err(|e| e.to_string())?;
        let direct_device: IDirect3DDevice = inspectable.cast().map_err(|e| e.to_string())?;

        let interop: IGraphicsCaptureItemInterop =
            factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(|e| e.to_string())?;
        let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd_from(hwnd)) }
            .map_err(|e| e.to_string())?;
        let size = item.Size().map_err(|e| e.to_string())?;

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &direct_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            size,
        )
        .map_err(|e| e.to_string())?;

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| e.to_string())?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(|e| e.to_string())?;

        Ok(Self {
            device,
            context,
            pool,
            session,
            _item: item,
        })
    }

    pub fn try_frame(&mut self) -> Result<Option<Frame>, String> {
        match self.pool.TryGetNextFrame() {
            Ok(frame) => {
                let surface = frame.Surface().map_err(|e| e.to_string())?;
                let access: IDirect3DDxgiInterfaceAccess =
                    surface.cast().map_err(|e| e.to_string())?;
                let texture: ID3D11Texture2D =
                    unsafe { access.GetInterface() }.map_err(|e| e.to_string())?;
                let frame = self.read_texture(&texture)?;
                if frame.is_uniform() {
                    return Ok(None);
                }
                Ok(Some(frame))
            }
            Err(_) => Ok(None),
        }
    }

    pub fn next_frame(&mut self) -> Result<Frame, String> {
        for _ in 0..60 {
            if let Some(frame) = self.try_frame()? {
                return Ok(frame);
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        Err("no frame arrived".into())
    }

    fn read_texture(&self, texture: &ID3D11Texture2D) -> Result<Frame, String> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }

        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        desc.ArraySize = 1;
        desc.MipLevels = 1;
        desc.SampleDesc.Count = 1;
        desc.SampleDesc.Quality = 0;

        let mut staging: Option<ID3D11Texture2D> = None;
        unsafe { self.device.CreateTexture2D(&desc, None, Some(&mut staging)) }
            .map_err(|e| e.to_string())?;
        let staging = staging.ok_or("staging texture was not created")?;

        unsafe {
            self.context.CopyResource(&staging, texture);
        }

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context
                .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
        }
        .map_err(|e| e.to_string())?;

        let width = desc.Width;
        let height = desc.Height;
        let row_pitch = mapped.RowPitch as usize;
        let mut rgba = vec![0u8; width as usize * height as usize * 4];

        unsafe {
            let source = mapped.pData as *const u8;
            for y in 0..height as usize {
                let row = source.add(y * row_pitch);
                for x in 0..width as usize {
                    let pixel = row.add(x * 4);
                    let out = (y * width as usize + x) * 4;
                    rgba[out] = *pixel.add(2);
                    rgba[out + 1] = *pixel.add(1);
                    rgba[out + 2] = *pixel;
                    rgba[out + 3] = 255;
                }
            }
            self.context.Unmap(&staging, 0);
        }

        Ok(Frame {
            width,
            height,
            rgba,
        })
    }
}

impl Drop for Capturer {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}

fn client_metrics(hwnd: i64) -> Option<(u32, u32, u32, u32)> {
    let handle = hwnd_from(hwnd);
    let mut client = RECT::default();
    let mut origin = POINT { x: 0, y: 0 };

    unsafe {
        GetClientRect(handle, &mut client).ok()?;
        if !ClientToScreen(handle, &mut origin).as_bool() {
            return None;
        }
    }

    // WGC frames match the visible window, so invisible DWM resize borders must
    // not be counted. Fall back to GetWindowRect if DWM cannot answer.
    let mut bounds = RECT::default();
    let mut visible = RECT::default();
    let dwm = unsafe {
        DwmGetWindowAttribute(
            handle,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut bounds as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    };
    if dwm.is_ok() {
        visible = bounds;
    } else {
        unsafe { GetWindowRect(handle, &mut visible).ok()? };
    }

    let width = (client.right - client.left).max(0) as u32;
    let height = (client.bottom - client.top).max(0) as u32;
    let offset_x = (origin.x - visible.left).max(0) as u32;
    let offset_y = (origin.y - visible.top).max(0) as u32;
    Some((offset_x, offset_y, width, height))
}

pub fn crop_to_client(frame: Frame, hwnd: i64) -> Result<Frame, String> {
    let (offset_x, offset_y, client_w, client_h) =
        client_metrics(hwnd).ok_or("cannot read window metrics")?;

    let width = client_w.min(frame.width.saturating_sub(offset_x));
    let height = client_h.min(frame.height.saturating_sub(offset_y));
    if width == 0 || height == 0 {
        return Err("client area is outside the captured frame".into());
    }

    let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        let start = (((y + offset_y) * frame.width + offset_x) * 4) as usize;
        let end = start + width as usize * 4;
        rgba.extend_from_slice(&frame.rgba[start..end]);
    }

    Ok(Frame {
        width,
        height,
        rgba,
    })
}

pub fn client_frame(hwnd: i64) -> Result<Frame, String> {
    let mut capturer = Capturer::attach(hwnd)?;
    let frame = capturer.next_frame()?;
    crop_to_client(frame, hwnd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires the game window to be open"]
    fn captures_game_client_area() {
        let selector = crate::settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window =
            crate::window_target::resolve_selector(&selector).expect("game window not found");

        let mut capturer = Capturer::attach(window.hwnd).expect("attach failed");
        let raw = capturer.next_frame().expect("frame failed");
        println!(
            "raw {}x{}, metrics {:?}, info {:?}",
            raw.width,
            raw.height,
            client_metrics(window.hwnd),
            (window.client.w, window.client.h, window.screen.x, window.screen.y)
        );

        let frame = crop_to_client(raw, window.hwnd).expect("capture failed");

        println!(
            "captured {}x{} (client {}x{})",
            frame.width, frame.height, window.client.w, window.client.h
        );
        assert!(frame.width > 100 && frame.height > 100);
        let lit = frame.rgba.iter().filter(|byte| **byte > 0).count();
        assert!(lit > frame.rgba.len() / 10, "captured frame looks blank");
    }

    fn write_bmp(frame: &Frame, path: &std::path::Path) {        let width = frame.width;
        let height = frame.height;
        let row = width as usize * 4;
        let size = 54 + row * height as usize;
        let mut out = Vec::with_capacity(size);
        out.extend_from_slice(b"BM");
        out.extend_from_slice(&(size as u32).to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&54u32.to_le_bytes());
        out.extend_from_slice(&40u32.to_le_bytes());
        out.extend_from_slice(&(width as i32).to_le_bytes());
        out.extend_from_slice(&(height as i32).to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&32u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&(row as u32 * height).to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&2835u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for y in (0..height as usize).rev() {
            for x in 0..width as usize {
                let index = (y * width as usize + x) * 4;
                out.push(frame.rgba[index + 2]);
                out.push(frame.rgba[index + 1]);
                out.push(frame.rgba[index]);
                out.push(255);
            }
        }

        std::fs::write(path, out).expect("write bmp");
        println!("wrote {} ({}x{})", path.display(), width, height);
    }

    #[test]
    #[ignore = "diagnostic: writes data/debug-frame.bmp"]
    fn dumps_a_frame_for_inspection() {
        let selector = crate::settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = crate::window_target::resolve_selector(&selector).expect("game window");
        let frame = client_frame(window.hwnd).expect("capture failed");

        let path = crate::data_dir().expect("data dir").join("debug-frame.bmp");
        write_bmp(&frame, &path);
    }

    #[test]
    #[ignore = "diagnostic: writes data/debug-frame.bmp plus an 8x view of a crop (MSCOUT_CROP=x,y,w,h)"]
    fn dumps_exp_crop() {
        let selector = crate::settings::WindowSelector {
            title: "MapleStory".into(),
            process: "MapleStory.exe".into(),
        };
        let window = crate::window_target::resolve_selector(&selector).expect("game window");
        let frame = client_frame(window.hwnd).expect("capture failed");

        let dir = crate::data_dir().expect("data dir");
        let frame_name = std::env::var("MSCOUT_FRAME").unwrap_or_else(|_| "debug-frame.bmp".into());
        write_bmp(&frame, &dir.join(&frame_name));

        let region = match std::env::var("MSCOUT_CROP") {
            Ok(spec) if spec == "panel" => {
                crate::ocr::detect_minimap_panel(&frame).expect("minimap panel")
            }
            Ok(spec) if spec == "label" => {
                let panel = crate::ocr::detect_minimap_panel(&frame).expect("minimap panel");
                println!("panel {panel:?}");
                crate::ocr::detect_map_label(&frame, panel).expect("map label")
            }
            Ok(spec) if spec.starts_with("line") => {
                let index: usize = spec[4..].parse().expect("line index");
                let panel = crate::ocr::detect_minimap_panel(&frame).expect("minimap panel");
                let label = crate::ocr::detect_map_label(&frame, panel).expect("map label");
                let gray = crate::ocr::crop_gray(&frame, label).expect("label crop");
                let lines = crate::ocr::text_lines(&gray);
                println!("label {label:?} -> {} lines {lines:?}", lines.len());
                let (x, y, w, h) = lines[index];
                crate::settings::Region {
                    x: label.x + x,
                    y: label.y + y,
                    w,
                    h,
                }
            }
            Ok(spec) => {
                let parts: Vec<i32> = spec.split(',').map(|p| p.trim().parse().expect("number")).collect();
                crate::settings::Region {
                    x: parts[0],
                    y: parts[1],
                    w: parts[2],
                    h: parts[3],
                }
            }
            Err(_) => {
                let profile = crate::profile::load_for_mode(
                    1920,
                    1200,
                    crate::settings::UiMode::Default,
                )
                .expect("1920x1200 profile");
                profile.calibration.exp.expect("EXP box")
            }
        };
        println!("crop {region:?}");
        let gray = crate::ocr::crop_gray(&frame, region).expect("crop");

        let scale = 8usize;
        let (width, height) = (gray.w * scale, gray.h * scale);
        let mut rgba = vec![0u8; width * height * 4];
        for y in 0..height {
            for x in 0..width {
                let value = gray.px[(y / scale) * gray.w + (x / scale)];
                let index = (y * width + x) * 4;
                rgba[index] = value;
                rgba[index + 1] = value;
                rgba[index + 2] = value;
                rgba[index + 3] = 255;
            }
        }

        write_bmp(
            &Frame {
                width: width as u32,
                height: height as u32,
                rgba,
            },
            &dir.join("exp-crop.bmp"),
        );
    }

    #[test]
    #[ignore = "diagnostic: captures a window to data/debug-window.bmp"]
    fn dumps_overlay() {
        let title = std::env::var("MSCOUT_DUMP_TITLE").unwrap_or_else(|_| "MScout".into());
        let process = std::env::var("MSCOUT_DUMP_PROCESS").unwrap_or_else(|_| "mscout.exe".into());
        let file = std::env::var("MSCOUT_DUMP_FILE").unwrap_or_else(|_| "debug-window.bmp".into());

        let selector = crate::settings::WindowSelector {
            title,
            process: process.clone(),
        };
        let window = crate::window_target::resolve_selector(&selector)
            .or_else(|| {
                crate::window_target::resolve_selector(&crate::settings::WindowSelector {
                    title: selector.title.clone(),
                    process: String::new(),
                })
            })
            .expect("target window");

        let frame = client_frame(window.hwnd).expect("capture failed");
        let path = crate::data_dir().expect("data dir").join(file);
        write_bmp(&frame, &path);
    }

    fn magnify(gray: &crate::ocr::Gray, scale: usize) -> Frame {
        let (w, h) = (gray.w * scale, gray.h * scale);
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let value = gray.px[(y / scale) * gray.w + (x / scale)];
                let index = (y * w + x) * 4;
                rgba[index] = value;
                rgba[index + 1] = value;
                rgba[index + 2] = value;
                rgba[index + 3] = 255;
            }
        }
        Frame {
            width: w as u32,
            height: h as u32,
            rgba,
        }
    }

    #[test]
    #[ignore = "diagnostic: dumps the detected minimap panel and label, magnified"]
    fn dumps_map_label() {
        let selector = crate::settings::WindowSelector {
            title: String::new(),
            process: std::env::var("MSCOUT_DUMP_PROCESS").unwrap_or_else(|_| "MapleStory.exe".into()),
        };
        let window = crate::window_target::resolve_selector(&selector).expect("game window");
        let frame = client_frame(window.hwnd).expect("capture failed");
        let dir = crate::data_dir().expect("data dir");

        let panel = crate::ocr::detect_minimap_panel(&frame).expect("minimap panel");
        let label = crate::ocr::detect_map_label(&frame, panel).expect("map label");
        println!("panel={panel:?} label={label:?}");

        write_bmp(
            &magnify(&crate::ocr::crop_gray(&frame, panel).expect("panel crop"), 3),
            &dir.join("panel-crop.bmp"),
        );
        write_bmp(
            &magnify(&crate::ocr::crop_gray(&frame, label).expect("label crop"), 5),
            &dir.join("label-crop.bmp"),
        );

        for (index, line) in crate::ocr::map_label_lines(&frame)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            println!("line {index}: {}x{}", line.w, line.h);
            write_bmp(&magnify(line, 5), &dir.join(format!("label-line-{index}.bmp")));
        }
    }
}
