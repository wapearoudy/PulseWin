//! Read the already-open quota page from a single verified window, in memory.

use std::time::{Duration, Instant};

use windows::core::{factory, Interface, RuntimeType};
use windows::Globalization::Language;
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::{Buffer, DataReader, DataWriter};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::Dxgi::{IDXGIAdapter, IDXGIDevice};
use windows::Win32::System::WinRT::{
    Direct3D11::CreateDirect3D11DeviceFromDXGIDevice,
    Graphics::Capture::IGraphicsCaptureItemInterop, RoInitialize, RoUninitialize,
    RO_INIT_MULTITHREADED,
};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
use windows_future::{AsyncStatus, IAsyncOperation};

use super::desktop_page::{locate_visual_rows, OcrLine};

const CAPTURE_BUDGET: Duration = Duration::from_secs(2);
const READ_BUDGET: Duration = Duration::from_secs(3);
const MAX_OCR_PIXELS: usize = 10_000_000;

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        // Constructed only after initializing WinRT on this thread.
        unsafe { RoUninitialize() }
    }
}

struct Capture {
    session: Option<GraphicsCaptureSession>,
    pool: Direct3D11CaptureFramePool,
}

impl Capture {
    fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.Close();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
        let _ = self.pool.Close();
    }
}

struct Image {
    width: usize,
    height: usize,
    bgra: Vec<u8>,
}

/// The caller supplies the verified, visible Antigravity top-level HWND.
pub(super) fn read_model_page(window: HWND) -> Option<Vec<String>> {
    let deadline = Instant::now() + READ_BUDGET;
    // SAFETY: WinRT is initialized and uninitialized on this same blocking worker.
    unsafe { RoInitialize(RO_INIT_MULTITHREADED).ok()? };
    let _apartment = Apartment;
    let image = capture(window, deadline)?;
    let broad_bounds = (
        image.width * 33 / 100,
        image.height * 8 / 100,
        image.width * 88 / 100,
        image.height * 91 / 100,
    );

    for locale in ["zh-Hans", "en-US"] {
        if Instant::now() >= deadline {
            return None;
        }
        let Ok(engine) = Language::CreateLanguage(&locale.into())
            .and_then(|language| OcrEngine::TryCreateFromLanguage(&language))
        else {
            continue;
        };
        let Some(broad) = recognize(&image, broad_bounds, 2, &engine, deadline) else {
            continue;
        };
        let Some(rows) = locate_visual_rows(&broad, image.width, image.height) else {
            continue;
        };
        let bounds = (rows.left, rows.top, rows.right, rows.bottom);
        if let Some(narrow) = recognize(&image, bounds, 3, &engine, deadline) {
            if let Some(text) = rows.finish(&narrow) {
                return Some(text);
            }
        }
    }
    None
}

fn capture(window: HWND, deadline: Instant) -> Option<Image> {
    // SAFETY: the caller verified the HWND. `CreateForWindow` captures only that
    // window, with the operating system's capture indicator and policy intact.
    let item: GraphicsCaptureItem = unsafe {
        if !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
            return None;
        }
        factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
            .ok()?
            .CreateForWindow(window)
            .ok()?
    };
    let size = item.Size().ok()?;
    if !(100..=4096).contains(&size.Width) || !(100..=2400).contains(&size.Height) {
        return None;
    }
    let device = direct3d_device()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        1,
        size,
    )
    .ok()?;
    let mut capture = Capture {
        session: None,
        pool,
    };
    let session = capture.pool.CreateCaptureSession(&item).ok()?;
    capture.session = Some(session);
    capture.session.as_ref()?.StartCapture().ok()?;

    let frame_deadline = deadline.min(Instant::now() + CAPTURE_BUDGET);
    let frame = loop {
        if Instant::now() >= frame_deadline {
            return None;
        }
        if let Ok(frame) = capture.pool.TryGetNextFrame() {
            break frame;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    capture.stop();
    let image = (|| {
        let content = frame.ContentSize().ok()?;
        if (content.Width, content.Height) != (size.Width, size.Height) {
            return None;
        }
        let copy = SoftwareBitmap::CreateCopyFromSurfaceAsync(&frame.Surface().ok()?).ok()?;
        let bitmap = await_result(copy, deadline)?;
        let (width, height) = (bitmap.PixelWidth().ok()?, bitmap.PixelHeight().ok()?);
        if (width, height) != (size.Width, size.Height) {
            return None;
        }
        let buffer = Buffer::Create((width * height * 4) as u32).ok()?;
        bitmap.CopyToBuffer(&buffer).ok()?;
        let reader = DataReader::FromBuffer(&buffer).ok()?;
        let mut bgra = vec![0; (width * height * 4) as usize];
        reader.ReadBytes(&mut bgra).ok()?;
        Some(Image {
            width: width as usize,
            height: height as usize,
            bgra,
        })
    })();
    let _ = frame.Close();
    image
}

fn direct3d_device() -> Option<IDirect3DDevice> {
    let mut d3d = None;
    // SAFETY: the output points to a valid local Option for the entire call.
    unsafe {
        D3D11CreateDevice(
            None::<&IDXGIAdapter>,
            D3D_DRIVER_TYPE_HARDWARE,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            None,
        )
        .ok()?;
        let dxgi: IDXGIDevice = d3d?.cast().ok()?;
        CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .ok()?
            .cast()
            .ok()
    }
}

fn await_result<T: RuntimeType + 'static>(
    operation: IAsyncOperation<T>,
    deadline: Instant,
) -> Option<T> {
    while Instant::now() < deadline {
        match operation.Status().ok()? {
            AsyncStatus::Completed => return operation.GetResults().ok(),
            AsyncStatus::Started => std::thread::sleep(Duration::from_millis(10)),
            _ => return None,
        }
    }
    let _ = operation.Cancel();
    None
}

fn recognize(
    image: &Image,
    (left, top, right, bottom): (usize, usize, usize, usize),
    scale: usize,
    engine: &OcrEngine,
    deadline: Instant,
) -> Option<Vec<OcrLine>> {
    if left >= right || top >= bottom || right > image.width || bottom > image.height {
        return None;
    }
    let (width, height) = ((right - left) * scale, (bottom - top) * scale);
    let max_side = OcrEngine::MaxImageDimension().ok()? as usize;
    if width > max_side || height > max_side || width * height > MAX_OCR_PIXELS {
        return None;
    }
    let mut scaled = Vec::with_capacity(width * height * 4);
    for y in top..bottom {
        if Instant::now() >= deadline {
            return None;
        }
        let row_start = scaled.len();
        for x in left..right {
            let pixel = (y * image.width + x) * 4;
            for _ in 0..scale {
                scaled.extend_from_slice(&image.bgra[pixel..pixel + 3]);
                scaled.push(255);
            }
        }
        for _ in 1..scale {
            scaled.extend_from_within(row_start..row_start + width * 4);
        }
    }

    let writer = DataWriter::new().ok()?;
    writer.WriteBytes(&scaled).ok()?;
    let bytes = writer.DetachBuffer().ok()?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &bytes,
        BitmapPixelFormat::Bgra8,
        width as i32,
        height as i32,
        BitmapAlphaMode::Ignore,
    )
    .ok()?;
    let result = await_result(engine.RecognizeAsync(&bitmap).ok()?, deadline)?;
    let lines = result.Lines().ok()?;
    let count = lines.Size().ok()?;
    if count > 512 {
        return None;
    }
    let mut geometry = Vec::new();
    for index in 0..count {
        let line = lines.GetAt(index).ok()?;
        let text = line.Text().ok()?;
        if text.is_empty() || text.len() > 256 {
            continue;
        }
        let words = line.Words().ok()?;
        let mut bounds: Option<(f32, f32, f32, f32)> = None;
        for index in 0..words.Size().ok()? {
            let rect = words.GetAt(index).ok()?.BoundingRect().ok()?;
            let (x, y, right, bottom) = (rect.X, rect.Y, rect.X + rect.Width, rect.Y + rect.Height);
            bounds = Some(match bounds {
                Some((l, t, r, b)) => (l.min(x), t.min(y), r.max(right), b.max(bottom)),
                None => (x, y, right, bottom),
            });
        }
        if let Some((l, t, r, b)) = bounds {
            let scale = scale as f32;
            geometry.push(OcrLine {
                text: text.to_string_lossy(),
                x: left as f32 + l / scale,
                y: top as f32 + t / scale,
                width: (r - l) / scale,
                height: (b - t) / scale,
            });
        }
    }
    Some(geometry)
}
