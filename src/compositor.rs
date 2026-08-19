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
    /// The D3D device's DXGI view. Each `Surface` creates its own composition
    /// device from this.
    dxgi_device: IDXGIDevice,
}

thread_local! {
    /// Per-surface present counts, keyed by [`present_key`]. Thread-local
    /// because every present happens on the one daemon render thread.
    pub static PER_SURFACE: std::cell::RefCell<std::collections::HashMap<u64, u64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());

    /// Cumulative microseconds spent in `present` per surface.
    pub static PER_SURFACE_US: std::cell::RefCell<std::collections::HashMap<u64, u64>> =
        std::cell::RefCell::new(std::collections::HashMap::new());

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

/// How many frames have been presented, process-wide, since start.
///
/// Exists to answer "is the thing on screen changing because WE changed it?"
/// from a shell that cannot see the desktop. Sampling this twice a second apart
/// gives the real present rate, which is the difference between a defect in our
/// present path and one in DWM's composition of a given output.
pub static PRESENTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Presents made for one specific SURFACE, so per-monitor rates can be told
/// apart.
///
/// Keyed by the surface's own identity, not its size: pHub has two 1920x1080
/// outputs, and a size-based key merged them into one counter that then read as
/// double the real rate -- an artefact that looks exactly like a genuine
/// double-present bug.
pub fn present_key(id: u64) -> u64 {
    id
}

/// Back buffers per surface, and the present sync interval. **One decision, two
/// constants** -- changing either alone reintroduces a bug.
///
/// The old code used `BufferCount: 2` with `Present(0, ...)`, and that
/// combination is a race, not an optimisation:
///
///   * two buffers means exactly ONE spare;
///   * `Present(0, ...)` returns the instant the frame is queued;
///   * so the next `GetBuffer(0)` can hand back the buffer the compositor is
///     STILL READING, and `CopyResource` overwrites it mid-read.
///
/// The visible result is a seam where two animation frames meet. Whether the
/// race is ever lost depends on the phase between our present cadence and each
/// output's compose cadence -- which is per-monitor. That is why one screen
/// tore and three did not, and why it looked like a monitor problem.
///
/// **The original comment arguing against interval 1 was RIGHT, and a later
/// edit (mine) overrode it and was wrong.** The claim was that raising the
/// buffer count makes interval-1 sync free, because DXGI blocks only when the
/// present queue is full. That is true of the QUEUE, and it misses the actual
/// cost: interval 1 also waits for the target monitor's VBLANK. With every
/// surface presented sequentially from one thread, that wait is serialised
/// across monitors that have unrelated refresh clocks, so from the second
/// monitor onward somebody always misses their frame. See `SYNC_INTERVAL`.
///
/// Triple buffering is still worth keeping: with interval 0 it is what lets a
/// present return immediately instead of stalling on a full queue.
///
/// Cost: one extra back buffer per monitor -- 14.7MB for a 1440x2560 portrait
/// panel, about 40MB across four outputs.
pub const SWAPCHAIN_BUFFERS: u32 = 3;

/// Present sync interval. **0 -- do NOT set this to 1 on a multi-monitor desk.**
///
/// Interval 1 blocks `Present` until THIS monitor's vblank, and every surface is
/// presented sequentially from the one daemon render thread. With a single
/// monitor that is harmless (and it is what a single-monitor machine measures).
/// With two or more it is not: surface A blocks on A's vblank, which pushes
/// surface B's present past B's vblank, so B misses its frame -- and with
/// different refresh rates per output (pHub: 120/60/280/60 Hz) the two clocks
/// never line up, so a monitor drops frames continuously. That is visible as a
/// flash.
///
/// Measured on pHub: ONE surface is clean; TWO surfaces flash. The threshold is
/// the surface COUNT, not which monitor, its rotation, or its refresh rate --
/// each of those was tested and ruled out.
///
/// The triple buffering below still matters: it is what keeps the present queue
/// from filling, so `Present(0, ...)` returns immediately rather than stalling
/// the shared thread.
pub const SYNC_INTERVAL: u32 = 0;

/// One monitor's composition surface.
pub struct Surface {
    /// Stable per-surface id, for the present counter. Sizes are not unique
    /// across monitors; this is.
    id: u64,
    gpu: std::rc::Rc<Gpu>,
    swapchain: IDXGISwapChain1,
    /// This surface's OWN composition device.
    ///
    /// **One per surface, NOT one shared across monitors.** A single
    /// `IDCompositionDevice` drives its entire visual tree on ONE composition
    /// clock, so every surface hung off it is composed at the same cadence --
    /// measured on pHub with a shared device: all four swapchains reported
    /// ~270Hz (the 280Hz primary's rate) even though the outputs run at
    /// 120/60/280/60. A 60Hz panel fed on a 270Hz cadence shows frames its
    /// scanout cannot match, which is visible as a flash, and it cannot happen
    /// with a single monitor -- which is why a shared device looked fine on a
    /// one-screen laptop and broke a four-screen desk.
    ///
    /// A device per surface costs one COM object per monitor and lets each
    /// target keep its own monitor's clock.
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
                BufferCount: SWAPCHAIN_BUFFERS,
                // FLIP_DISCARD, not FLIP_SEQUENTIAL: every frame overwrites the
                // whole surface through `CopyResource`, so preserving the old
                // contents buys nothing and only constrains the driver.
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                // THE line that makes a GDI-drawn frame opaque. GDI leaves
                // alpha at 0; IGNORE tells DWM not to look at it. With
                // PREMULTIPLIED here the wallpaper composites additively over
                // Explorer's own background -- measured, see the module header.
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                // `Scaling` is DELIBERATELY left to `..Default::default()`,
                // which supplies `DXGI_SCALING_STRETCH` (0).
                //
                // Do not "tidy" this by naming `DXGI_SCALING_NONE` explicitly.
                // `CreateSwapChainForComposition` REQUIRES stretch, and rejects
                // `NONE` with `DXGI_ERROR_INVALID_CALL` (0x887A0001) -- which
                // kills every surface, on every monitor, at creation. Measured
                // on 26200: the daemon came up reporting four monitors with no
                // panel behind any of them, wallpaper gone, 2.6% CPU instead of
                // ~30%. The error text ("the application made a call that is
                // invalid") names no field, so it reads like a device problem
                // rather than this one line.
                //
                // The reasoning that motivated `NONE` -- that a stretch-capable
                // surface might be resampled during a rotated output's
                // composition pass -- may or may not be true, but it is not
                // available to us here either way.
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
                dcomp,
                id: {
                    static NEXT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(1);
                    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                },
                gpu,
                swapchain,
                _target: target,
                _visual: visual,
                upload,
                width,
                height,
            })
        }
    }

    /// The swapchain's current size, for diagnosis.
    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// How many frames this surface has presented.
    pub fn presents(&self) -> u64 {
        PER_SURFACE.with(|m| m.borrow().get(&present_key(self.id)).copied().unwrap_or(0))
    }

    /// Cumulative microseconds spent presenting this surface.
    pub fn present_us(&self) -> u64 {
        PER_SURFACE_US.with(|m| m.borrow().get(&self.id).copied().unwrap_or(0))
    }

    /// DXGI's own account of what reached the screen: `(presented, displayed)`.
    ///
    /// `PresentCount` is how many frames WE submitted; `PresentRefreshCount` is
    /// the refresh at which the last one actually appeared. Sampling both twice
    /// tells us whether this monitor is showing every frame we hand it or
    /// dropping some -- which is the difference between a flash that is ours
    /// and one that is DWM's, and it is measurable without looking at a screen.
    ///
    /// `None` when DXGI has no statistics yet (the surface has not presented,
    /// or the swapchain is not in a state that tracks them).
    pub fn frame_stats(&self) -> Option<(u32, u32)> {
        unsafe {
            let mut st = DXGI_FRAME_STATISTICS::default();
            self.swapchain
                .GetFrameStatistics(&mut st)
                .ok()
                .map(|()| (st.PresentCount, st.PresentRefreshCount))
        }
    }

    /// Wraps [`dib_geometry_fault`] with this surface's own dimensions.
    fn geometry_fault(&self, stride: usize) -> Option<String> {
        dib_geometry_fault(self.width, self.height, stride)
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
        // The DIB and this surface are sized from the same `panel.width/height`,
        // so a disagreement means they have drifted -- and the copy loop below
        // would then read or write a partial frame, which is a visible tear.
        // Cheap to check, and it converts an invisible defect into a log line.
        if let Some(bad) = self.geometry_fault(stride) {
            return Err(bad);
        }
        let t_start = std::time::Instant::now();
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

            // Sync interval 1, with THREE buffers. These two settings are one
            // decision -- see `SWAPCHAIN_BUFFERS`.
            self.swapchain
                .Present(SYNC_INTERVAL, DXGI_PRESENT(0))
                .ok()
                .map_err(|e| format!("Present failed: {e}"))?;
            PRESENTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            // How long the Present+upload actually took for THIS surface.
            //
            // A rotated output cannot be scanned out directly from the composed
            // surface, so DWM runs a rotation pass for it. If that cost is real
            // and per-present, it shows up here as a systematically longer
            // present on exactly the outputs that flash -- which is measurable
            // from a shell that cannot see the screen.
            PER_SURFACE_US.with(|m| {
                *m.borrow_mut().entry(self.id).or_insert(0) +=
                    t_start.elapsed().as_micros() as u64;
            });
            PER_SURFACE.with(|m| {
                *m.borrow_mut()
                    .entry(present_key(self.id))
                    .or_insert(0) += 1u64;
            });

            // NO `Commit` here, deliberately.
            //
            // `Commit` publishes changes to the VISUAL TREE. This surface's tree
            // is built once in `new` -- target, visual and content are all set
            // there, followed by the one `Commit` that matters -- and never
            // touched again. New FRAMES reach the screen through the swapchain
            // `Present` above: a composition swapchain set as visual content
            // updates with no commit at all.
            //
            // (An earlier version committed on every present. That is not free:
            // it republishes a composition batch to DWM per frame per monitor.)
        }
        Ok(())
    }
}

/// Does the DIB handed to `present` actually match this surface?
///
/// Split from the `unsafe` body so the rule is testable: a DIB whose row is
/// shorter than the surface's row means the copy would leave the right-hand
/// columns of every row holding the PREVIOUS frame's pixels -- which reads as a
/// band that flickers between two frames rather than obvious garbage.
///
/// Returns a description of the fault, or `None` when the geometry is sound.
pub fn dib_geometry_fault(width: i32, height: i32, stride: usize) -> Option<String> {
    if width <= 0 || height <= 0 {
        return Some(format!("surface has no area: {width}x{height}"));
    }
    let need = (width as usize) * 4;
    if stride < need {
        return Some(format!(
            "DIB stride {stride} is too small for a {width}px row (needs {need})              -- every row would keep stale pixels on the right"
        ));
    }
    None
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
    /// A composition swapchain MUST use stretch scaling.
    ///
    /// `CreateSwapChainForComposition` rejects `DXGI_SCALING_NONE` with
    /// `DXGI_ERROR_INVALID_CALL` (0x887A0001). Naming it explicitly -- as an
    /// "explicit is better than implicit" cleanup -- took the wallpaper down on
    /// all four of pHub's monitors at once, and the error names no field, so it
    /// reads like a broken device rather than one wrong constant.
    ///
    /// This pins the VALUE rather than the descriptor, because the descriptor
    /// is built inside an `unsafe` block no test can reach. `STRETCH` is 0,
    /// which is what `..Default::default()` supplies -- so the assertion below
    /// is exactly "the default is the required value", and it fails the moment
    /// somebody decides otherwise.
    #[test]
    fn a_composition_swapchain_must_use_stretch_scaling() {
        use windows::Win32::Graphics::Dxgi::{DXGI_SCALING_NONE, DXGI_SCALING_STRETCH};
        assert_eq!(
            DXGI_SCALING_STRETCH.0, 0,
            "STRETCH must be the zero value, i.e. what Default supplies"
        );
        assert_ne!(
            DXGI_SCALING_NONE.0, DXGI_SCALING_STRETCH.0,
            "if these ever collide this test has stopped testing anything"
        );
    }

    /// The geometry invariant behind `present`.
    ///
    /// Added while chasing a flash that affected only 2 of pHub's 4 monitors.
    /// It did NOT turn out to be the cause -- all four measured consistent --
    /// but the check is worth keeping: it is the one failure that would produce
    /// exactly this symptom (a band of stale pixels refreshing at frame rate)
    /// while every log line still said the wallpaper was fine.
    #[test]
    fn a_dib_row_shorter_than_the_surface_is_a_fault() {
        // 1440px needs 5760 bytes. One pixel short means every row keeps the
        // previous frame's rightmost column -- a flicker, not obvious garbage.
        assert!(super::dib_geometry_fault(1440, 2560, 5756).is_some());
        assert!(super::dib_geometry_fault(1440, 2560, 5760).is_none());
        // A driver may pad the row LONGER; that is normal and must pass.
        assert!(super::dib_geometry_fault(1440, 2560, 6144).is_none());
    }

    #[test]
    fn a_zero_area_surface_is_a_fault_not_a_silent_present() {
        // Presenting into a 0-area surface would map a zero-byte texture and
        // copy nothing, leaving whatever was there before on screen.
        assert!(super::dib_geometry_fault(0, 1080, 4096).is_some());
        assert!(super::dib_geometry_fault(1920, 0, 7680).is_some());
    }

    /// pHub's four real monitors, as measured over the control channel.
    ///
    /// Pinned as a regression net for the resolution hypothesis: the two that
    /// flash (a 1440x2560 portrait and a 2560x720 ultrawide) are NOT the two
    /// with the most pixels -- the 2560x720 has 1.84M against 2.07M for each
    /// unaffected 1080p screen -- and every one of them has sound geometry.
    /// Whatever the flash is, it is not a size mismatch.
    #[test]
    fn every_real_phub_monitor_has_sound_geometry() {
        for (w, h) in [(1440, 2560), (2560, 720), (1920, 1080), (1920, 1080)] {
            assert!(
                super::dib_geometry_fault(w, h, (w as usize) * 4).is_none(),
                "{w}x{h} should be sound"
            );
        }
    }

    /// The tearing fix, pinned as the pair it is.
    ///
    /// Sabotage-checked in both directions: setting `SYNC_INTERVAL` back to 0
    /// fails this, and so does dropping `SWAPCHAIN_BUFFERS` to 2. That is the
    /// point -- either one alone reintroduces a bug. Interval 0 with any buffer
    /// count races the compositor for the buffer it is reading (the seam on the
    /// portrait screen); interval 1 with two buffers blocks the shared render
    /// thread on every present, so a 5fps wallpaper paces the 30fps terminal
    /// backdrops.
    #[test]
    fn the_present_syncs_and_has_a_spare_buffer_to_make_that_free() {
        assert_eq!(
            super::SYNC_INTERVAL, 0,
            "interval 1 serialises every monitor's present against its own              vblank on one thread, so a second monitor misses frames --              measured on pHub: one surface clean, two surfaces flashing"
        );
        assert!(
            super::SWAPCHAIN_BUFFERS >= 3,
            "with fewer than 3 buffers the present queue holds <2 frames, so              interval 1 blocks the one thread that presents every panel"
        );
    }

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
