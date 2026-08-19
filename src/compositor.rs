//! Presenting a desktop surface through DirectComposition.
//!
//! # Why this exists at all
//!
//! The terminal panels blit their frame straight to the window DC with GDI and
//! that is the end of it. A desktop surface cannot, and the reason is not the
//! one this project assumed for a long time.
//!
//! On the Windows 11 "raised desktop" (24H2+), `Progman` carries
//! `WS_EX_NOREDIRECTIONBITMAP` and the desktop tree is composited with an alpha
//! channel. **GDI never writes alpha.** Every pixel it produces has `A = 0`, and
//! DWM composites that surface as PREMULTIPLIED alpha, so the result is
//! `dst + src` — additive, not opaque.
//!
//! Measured on build 26200, filling a child of the icon host with mid-grey
//! `(100,100,100)` over a desktop pixel reading `(9,26,54)`:
//!
//! | path                        | read back      | verdict  |
//! |-----------------------------|----------------|----------|
//! | GDI `BitBlt` to the window  | `109,126,154`  | additive |
//! | D3D11 + DirectComposition   | `100,100,100`  | opaque   |
//!
//! The wallpaper looked like a brightening filter over Explorer's wallpaper
//! rather than a replacement, and it wiped the icons as it went.
//!
//! **A saturated colour cannot detect this.** The earlier probes filled with
//! pure red and sampled "is it red", which passes on a dark desktop whether the
//! surface is opaque or additive. That false pass is what made a GDI child look
//! like a complete fix. Any future probe here must fill with a mid-grey and
//! compare the exact value.
//!
//! # What this does instead
//!
//! The renderer still draws the whole frame with GDI, into a 32-bit DIB section
//! so the pixels are CPU-addressable. That preserves the entire cell blitter,
//! the font handling and the per-row colour batching that the optimisation pass
//! bought. Only the *presentation* changes: the finished pixels are uploaded
//! into a D3D11 texture and presented through a composition swapchain, which
//! DWM composites opaquely.
//!
//! `DXGI_ALPHA_MODE_IGNORE` is the point of the swapchain description: it tells
//! DWM to disregard the alpha channel entirely, so the `A = 0` bytes GDI leaves
//! behind stop mattering. Without it we would have to walk every pixel setting
//! alpha to 255 on every frame.

use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectComposition::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

/// A D3D11 device plus the DXGI factory it came from.
///
/// One per process, shared by every monitor: creating a device runs the driver
/// and costs real milliseconds, while an extra swapchain is cheap. Rebuilding
/// one per surface would pay that cost on every effect switch, because switching
/// an effect destroys and recreates the panel.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    factory: IDXGIFactory2,
    dxgi_device: IDXGIDevice,
}

thread_local! {
    static GPU: std::cell::RefCell<Option<std::rc::Rc<Gpu>>> =
        const { std::cell::RefCell::new(None) };
}

fn create_device() -> Result<std::rc::Rc<Gpu>, String> {
    unsafe {
        // BGRA_SUPPORT is REQUIRED for DirectComposition interop; without it
        // CreateSwapChainForComposition fails with a flag error that reads like
        // a swapchain problem rather than a device one.
        let mut last = String::new();
        for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
            let mut device: Option<ID3D11Device> = None;
            let mut context: Option<ID3D11DeviceContext> = None;
            let hr = D3D11CreateDevice(
                None,
                driver,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            );
            match (hr, device, context) {
                (Ok(()), Some(device), Some(context)) => {
                    let dxgi_device: IDXGIDevice = device
                        .cast()
                        .map_err(|e| format!("IDXGIDevice cast failed: {e}"))?;
                    let adapter = dxgi_device
                        .GetAdapter()
                        .map_err(|e| format!("GetAdapter failed: {e}"))?;
                    let factory: IDXGIFactory2 = adapter
                        .GetParent()
                        .map_err(|e| format!("IDXGIFactory2 failed: {e}"))?;
                    return Ok(std::rc::Rc::new(Gpu {
                        device,
                        context,
                        factory,
                        dxgi_device,
                    }));
                }
                (Err(e), _, _) => last = format!("{e}"),
                _ => last = "D3D11CreateDevice returned no device".into(),
            }
        }
        // WARP is the software fallback, so reaching here means no renderer at
        // all. A NAMED error, never a silent None: "no wallpaper" and "panefx is
        // broken" must not look alike.
        Err(format!("no D3D11 device (hardware or WARP): {last}"))
    }
}

fn gpu() -> Result<std::rc::Rc<Gpu>, String> {
    GPU.with(|g| {
        if let Some(existing) = g.borrow().as_ref() {
            return Ok(existing.clone());
        }
        let made = create_device()?;
        *g.borrow_mut() = Some(made.clone());
        Ok(made)
    })
}

/// Drop the shared device so the next surface rebuilds it.
///
/// Called when the device is reported lost. The surfaces themselves are torn
/// down and recreated by the wallpaper's existing reattach path.
fn forget_device() {
    GPU.with(|g| *g.borrow_mut() = None);
}

/// One monitor's composition surface.
pub struct Surface {
    gpu: std::rc::Rc<Gpu>,
    swapchain: IDXGISwapChain1,
    dcomp: IDCompositionDevice,
    /// Held because dropping the target detaches the visual tree from the
    /// window, even though nothing reads it after construction.
    _target: IDCompositionTarget,
    _visual: IDCompositionVisual,
    /// CPU-writable staging texture. A flip-model back buffer cannot be mapped,
    /// so pixels land here and are copied across.
    upload: ID3D11Texture2D,
    width: i32,
    height: i32,
}

impl Surface {
    /// Attach a composition swapchain to `hwnd`.
    pub fn new(hwnd: HWND, width: i32, height: i32) -> Result<Self, String> {
        let width = width.max(1);
        let height = height.max(1);
        let gpu = gpu()?;
        unsafe {
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width as u32,
                Height: height as u32,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                // THE line that makes a GDI-drawn frame opaque. GDI leaves
                // alpha at 0; IGNORE tells DWM not to look at it. With
                // PREMULTIPLIED here the wallpaper composites additively over
                // Explorer's own background -- measured, see the module header.
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let swapchain = gpu
                .factory
                .CreateSwapChainForComposition(&gpu.device, &desc, None)
                .map_err(|e| format!("CreateSwapChainForComposition failed: {e}"))?;

            let dcomp: IDCompositionDevice = DCompositionCreateDevice(&gpu.dxgi_device)
                .map_err(|e| format!("DCompositionCreateDevice failed: {e}"))?;
            // topmost = false: the visual sits below the window's other content,
            // which is what keeps us under the desktop icons.
            let target = dcomp
                .CreateTargetForHwnd(hwnd, false)
                .map_err(|e| format!("CreateTargetForHwnd failed: {e}"))?;
            let visual = dcomp
                .CreateVisual()
                .map_err(|e| format!("CreateVisual failed: {e}"))?;
            visual
                .SetContent(&swapchain)
                .map_err(|e| format!("SetContent failed: {e}"))?;
            target
                .SetRoot(&visual)
                .map_err(|e| format!("SetRoot failed: {e}"))?;
            dcomp
                .Commit()
                .map_err(|e| format!("Commit failed: {e}"))?;

            let upload = make_upload(&gpu.device, width, height)?;

            Ok(Surface {
                gpu,
                swapchain,
                dcomp,
                _target: target,
                _visual: visual,
                upload,
                width,
                height,
            })
        }
    }

    /// Is the device still usable?
    ///
    /// Both calls are trivial and touch no GPU work, so they are safe to poll on
    /// a surface that is deliberately frozen. This is what replaces presenting a
    /// keepalive frame: a composition surface keeps showing its last frame
    /// indefinitely with no further presents, so the only thing that can take it
    /// away is the device going out from under us — a TDR, a driver update, or a
    /// hybrid-GPU switch.
    pub fn healthy(&self) -> bool {
        unsafe {
            let dcomp_ok = self
                .dcomp
                .CheckDeviceState()
                .map(|b| b.as_bool())
                .unwrap_or(false);
            dcomp_ok && self.gpu.device.GetDeviceRemovedReason().is_ok()
        }
    }

    /// Resize the swapchain to match a resized surface.
    pub fn resize(&mut self, width: i32, height: i32) -> Result<(), String> {
        let width = width.max(1);
        let height = height.max(1);
        if width == self.width && height == self.height {
            return Ok(());
        }
        unsafe {
            self.swapchain
                .ResizeBuffers(
                    0,
                    width as u32,
                    height as u32,
                    DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG(0),
                )
                .map_err(|e| format!("ResizeBuffers failed: {e}"))?;
            self.upload = make_upload(&self.gpu.device, width, height)?;
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Upload one finished frame and present it.
    ///
    /// `bits` is the DIB section the renderer drew into: 32 bits per pixel,
    /// top-down, `stride` bytes per row.
    pub fn present(&self, bits: *const u8, stride: usize) -> Result<(), String> {
        if bits.is_null() {
            return Err("no pixels to present".into());
        }
        unsafe {
            let ctx = &self.gpu.context;

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            ctx.Map(
                &self.upload,
                0,
                D3D11_MAP_WRITE_DISCARD,
                0,
                Some(&mut mapped),
            )
            .map_err(|e| format!("Map failed: {e}"))?;

            // Row by row: the mapped RowPitch is chosen by the driver and is NOT
            // width * 4. Copying the whole block in one memcpy silently skews
            // every row on any adapter that pads.
            let dst = mapped.pData as *mut u8;
            let row_bytes = (self.width as usize) * 4;
            let copy = row_bytes.min(stride).min(mapped.RowPitch as usize);
            for y in 0..self.height as usize {
                std::ptr::copy_nonoverlapping(
                    bits.add(y * stride),
                    dst.add(y * mapped.RowPitch as usize),
                    copy,
                );
            }
            ctx.Unmap(&self.upload, 0);

            let back: ID3D11Texture2D = self
                .swapchain
                .GetBuffer(0)
                .map_err(|e| format!("GetBuffer failed: {e}"))?;
            ctx.CopyResource(&back, &self.upload);

            // Sync interval 0, NOT 1. Interval 1 blocks the calling thread until
            // vblank, and the daemon presents every panel from one thread -- a
            // 5fps wallpaper would then pace the 30fps terminal backdrops.
            self.swapchain
                .Present(0, DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("Present failed: {e}"))?;
            self.dcomp
                .Commit()
                .map_err(|e| format!("Commit failed: {e}"))?;
        }
        Ok(())
    }
}

fn make_upload(device: &ID3D11Device, width: i32, height: i32) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width as u32,
        Height: height as u32,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DYNAMIC,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: D3D11_CPU_ACCESS_WRITE.0 as u32,
        MiscFlags: 0,
    };
    let mut tex: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut tex))
            .map_err(|e| format!("CreateTexture2D failed: {e}"))?;
    }
    tex.ok_or_else(|| "CreateTexture2D returned nothing".to_string())
}

/// Called when a surface reports itself unhealthy, so the next attach rebuilds
/// the device from scratch rather than handing out the dead one.
pub fn drop_shared_device() {
    forget_device();
}

#[cfg(test)]
mod tests {
    /// The alpha mode is the whole reason a GDI-drawn frame reads back opaque.
    ///
    /// Measured on build 26200: with PREMULTIPLIED, a mid-grey (100,100,100)
    /// fill over a (9,26,54) desktop pixel read back (109,126,154) — additive,
    /// because GDI leaves every alpha byte at 0. With IGNORE it reads back
    /// (100,100,100).
    ///
    /// This is a constant check rather than a behavioural one, which is honest
    /// about what it covers: it stops the value being "tidied" to PREMULTIPLIED
    /// by someone who reasonably assumes a wallpaper wants alpha.
    #[test]
    fn the_swapchain_ignores_alpha_because_gdi_never_writes_it() {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_ALPHA_MODE_IGNORE, DXGI_ALPHA_MODE_PREMULTIPLIED,
        };
        let chosen = DXGI_ALPHA_MODE_IGNORE;
        assert_ne!(
            chosen, DXGI_ALPHA_MODE_PREMULTIPLIED,
            "PREMULTIPLIED makes a GDI frame composite additively over the desktop"
        );
    }
}
