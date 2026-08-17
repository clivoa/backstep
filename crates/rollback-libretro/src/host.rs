//! The libretro *frontend* side: environment, video, audio and input callbacks.
//!
//! # Why the state is global
//!
//! libretro callbacks are bare C function pointers with no user-data argument,
//! so the frontend state has to be reachable from a `static`. That is not a
//! shortcut: libretro cores are single-instance by design (FBNeo keeps its
//! machine state in globals), so a per-instance host would be a lie anyway.
//! The host therefore refuses to load a second core in the same process.
//!
//! The state sits behind a `Mutex` and no lock is ever held across a call into
//! the core, so a core that calls back from its own thread is safe rather than
//! deadlocked.

use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_uint, c_void};
use std::sync::{LazyLock, Mutex, OnceLock};

use crate::ffi::*;

/// One decoded video frame, always converted to XRGB8888 for the renderer.
#[derive(Clone, Default)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl VideoFrame {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0 || self.pixels.is_empty()
    }
}

#[derive(Default)]
pub struct HostState {
    /// RetroPad bitmask per port, written by the simulation before `retro_run`.
    pub inputs: [u16; 2],
    /// When true, video and audio produced by the core are thrown away.
    pub discard_output: bool,
    pub video: VideoFrame,
    pub audio: Vec<i16>,
    pub geometry: retro_game_geometry,
    pub timing: retro_system_timing,
    pub pixel_format: c_uint,
    pub shutdown_requested: bool,
    pub frames_presented: u64,
    pub frames_discarded: u64,
    pub input_polls: u64,
    /// Environment commands the core asked for that this host does not answer.
    pub unhandled_environment: Vec<c_uint>,
    /// Messages the core asked the frontend to display.
    ///
    /// This is the *only* diagnostic channel a stable-Rust frontend has. The
    /// libretro log interface needs a C-variadic function, which stable Rust
    /// cannot define, so it is refused -- and without capturing these, a core
    /// that fails to load a ROM fails silently with no reason given. FBNeo in
    /// particular returns success from `retro_load_game` even when the romset
    /// is unusable, and explains itself only here.
    pub messages: Vec<String>,
}

static HOST: LazyLock<Mutex<HostState>> = LazyLock::new(|| {
    Mutex::new(HostState {
        pixel_format: RETRO_PIXEL_FORMAT_0RGB1555,
        ..Default::default()
    })
});

/// Core option overrides, fixed for the process lifetime.
///
/// Pointers handed to the core through `GET_VARIABLE` must stay valid until the
/// core is unloaded, so these live in a `OnceLock` rather than inside the
/// mutex-protected state, whose `Vec` could reallocate under them.
static OPTIONS: OnceLock<Vec<(CString, CString)>> = OnceLock::new();
/// System and save directories handed to the core, same lifetime argument.
static DIRS: OnceLock<(CString, CString)> = OnceLock::new();

/// Run `f` against the host state.
pub fn with_host<R>(f: impl FnOnce(&mut HostState) -> R) -> R {
    let mut guard = HOST.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// Install the core option overrides. Only the first call takes effect.
pub fn set_options(options: &[(&str, &str)]) {
    let _ = OPTIONS.set(
        options
            .iter()
            .map(|(k, v)| {
                (
                    CString::new(*k).expect("option key has no interior NUL"),
                    CString::new(*v).expect("option value has no interior NUL"),
                )
            })
            .collect(),
    );
}

/// Install the system and save directories. Only the first call takes effect.
///
/// Both peers must point these at a directory with identical contents. FBNeo
/// reads NVRAM and per-game settings from here, and a stale NVRAM file on one
/// side is a desync on frame one that no amount of input agreement can fix.
pub fn set_directories(system_dir: &str, save_dir: &str) {
    let _ = DIRS.set((
        CString::new(system_dir).expect("system dir has no interior NUL"),
        CString::new(save_dir).expect("save dir has no interior NUL"),
    ));
}

fn options() -> &'static [(CString, CString)] {
    OPTIONS.get_or_init(Vec::new)
}

fn dirs() -> &'static (CString, CString) {
    DIRS.get_or_init(|| (CString::new(".").unwrap(), CString::new(".").unwrap()))
}

/// Reset the mutable parts of the host between sessions.
pub fn reset_state() {
    with_host(|h| {
        h.inputs = [0; 2];
        h.discard_output = false;
        h.video = VideoFrame::default();
        h.audio.clear();
        h.shutdown_requested = false;
        h.frames_presented = 0;
        h.frames_discarded = 0;
        h.input_polls = 0;
        h.unhandled_environment.clear();
        h.messages.clear();
    });
}

/// Copy a NUL-terminated message out of the core and keep it.
fn record_message(msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    // SAFETY: libretro guarantees a NUL-terminated string valid for the call.
    let text = unsafe { CStr::from_ptr(msg) }
        .to_string_lossy()
        .into_owned();
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    with_host(|h| {
        // Cores repeat the same notification for several frames; keep one copy.
        if h.messages.last().map(String::as_str) != Some(text.as_str()) {
            h.messages.push(text);
        }
    });
}

/// Messages the core has asked the frontend to display, oldest first.
pub fn messages() -> Vec<String> {
    with_host(|h| h.messages.clone())
}

/// The messages formatted for an error message, or an empty string.
pub fn render_messages() -> String {
    let messages = messages();
    if messages.is_empty() {
        return String::new();
    }
    format!("\n  core said: {}", messages.join("\n  core said: "))
}

// --- callbacks -------------------------------------------------------------

/// # Safety
/// Called by the core with a `data` pointer whose type is determined by `cmd`.
pub unsafe extern "C" fn environment(cmd: c_uint, data: *mut c_void) -> bool {
    match cmd {
        RETRO_ENVIRONMENT_SET_PIXEL_FORMAT => {
            if data.is_null() {
                return false;
            }
            let format = unsafe { *(data as *const c_uint) };
            let supported = matches!(
                format,
                RETRO_PIXEL_FORMAT_0RGB1555
                    | RETRO_PIXEL_FORMAT_XRGB8888
                    | RETRO_PIXEL_FORMAT_RGB565
            );
            if supported {
                with_host(|h| h.pixel_format = format);
            }
            supported
        }
        RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY | RETRO_ENVIRONMENT_GET_CORE_ASSETS_DIRECTORY => {
            if data.is_null() {
                return false;
            }
            unsafe { *(data as *mut *const c_char) = dirs().0.as_ptr() };
            true
        }
        RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY => {
            if data.is_null() {
                return false;
            }
            unsafe { *(data as *mut *const c_char) = dirs().1.as_ptr() };
            true
        }
        RETRO_ENVIRONMENT_GET_VARIABLE => {
            if data.is_null() {
                return false;
            }
            let var = unsafe { &mut *(data as *mut retro_variable) };
            if var.key.is_null() {
                return false;
            }
            let key = unsafe { CStr::from_ptr(var.key) };
            match options().iter().find(|(k, _)| k.as_c_str() == key) {
                Some((_, value)) => {
                    var.value = value.as_ptr();
                    true
                }
                // Returning false leaves the core on its own default, which is
                // what we want for every option we do not deliberately pin.
                None => false,
            }
        }
        // We never change options mid-session; a core that believed otherwise
        // could re-read them at a different frame on each peer.
        RETRO_ENVIRONMENT_GET_VARIABLE_UPDATE => {
            if !data.is_null() {
                unsafe { *(data as *mut bool) = false };
            }
            true
        }
        RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO => {
            if data.is_null() {
                return false;
            }
            let av = unsafe { &*(data as *const retro_system_av_info) };
            with_host(|h| {
                h.geometry = av.geometry;
                h.timing = av.timing;
            });
            true
        }
        RETRO_ENVIRONMENT_SET_GEOMETRY => {
            if data.is_null() {
                return false;
            }
            let geometry = unsafe { *(data as *const retro_game_geometry) };
            with_host(|h| h.geometry = geometry);
            true
        }
        RETRO_ENVIRONMENT_GET_CAN_DUPE => {
            if !data.is_null() {
                unsafe { *(data as *mut bool) = true };
            }
            true
        }
        RETRO_ENVIRONMENT_GET_OVERSCAN => {
            if !data.is_null() {
                unsafe { *(data as *mut bool) = false };
            }
            true
        }
        RETRO_ENVIRONMENT_SET_MESSAGE => {
            if data.is_null() {
                return false;
            }
            // SAFETY: the command contract says `data` is a `retro_message`.
            let msg = unsafe { (*(data as *const retro_message)).msg };
            record_message(msg);
            true
        }
        RETRO_ENVIRONMENT_SET_MESSAGE_EXT => {
            if data.is_null() {
                return false;
            }
            // SAFETY: the command contract says `data` is a `retro_message_ext`.
            let msg = unsafe { (*(data as *const retro_message_ext)).msg };
            record_message(msg);
            true
        }
        RETRO_ENVIRONMENT_SHUTDOWN => {
            with_host(|h| h.shutdown_requested = true);
            true
        }
        RETRO_ENVIRONMENT_GET_CORE_OPTIONS_VERSION => {
            // Version 0: the core falls back to plain SET_VARIABLES, which is
            // all this host parses.
            if !data.is_null() {
                unsafe { *(data as *mut c_uint) = 0 };
            }
            true
        }
        // Accepted and ignored: descriptive metadata with no effect on the
        // simulation.
        RETRO_ENVIRONMENT_SET_VARIABLES
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_INTL
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2
        | RETRO_ENVIRONMENT_SET_CORE_OPTIONS_V2_INTL
        | RETRO_ENVIRONMENT_SET_INPUT_DESCRIPTORS
        | RETRO_ENVIRONMENT_SET_CONTROLLER_INFO
        | RETRO_ENVIRONMENT_SET_SUBSYSTEM_INFO
        | RETRO_ENVIRONMENT_SET_PERFORMANCE_LEVEL
        | RETRO_ENVIRONMENT_SET_SUPPORT_NO_GAME
        | RETRO_ENVIRONMENT_SET_ROTATION => true,
        // Deliberately unanswered. The log interface in particular needs a
        // C-variadic function, which stable Rust cannot define; cores fall back
        // to their own logging when it is refused.
        RETRO_ENVIRONMENT_GET_LOG_INTERFACE
        | RETRO_ENVIRONMENT_GET_PERF_INTERFACE
        | RETRO_ENVIRONMENT_GET_INPUT_BITMASKS
        | RETRO_ENVIRONMENT_GET_LANGUAGE
        | RETRO_ENVIRONMENT_GET_LIBRETRO_PATH => false,
        other => {
            with_host(|h| {
                if !h.unhandled_environment.contains(&other) {
                    h.unhandled_environment.push(other);
                }
            });
            false
        }
    }
}

/// # Safety
/// `data` points at `height * pitch` bytes in the format last agreed through
/// `SET_PIXEL_FORMAT`, or is null for a duplicate frame.
pub unsafe extern "C" fn video_refresh(
    data: *const c_void,
    width: c_uint,
    height: c_uint,
    pitch: usize,
) {
    let discard = with_host(|h| h.discard_output);
    if discard {
        // This is the whole point of `OutputMode::Resimulate`: a rollback
        // replays up to eight frames inside one display frame, and showing them
        // would be a visible stutter.
        with_host(|h| h.frames_discarded += 1);
        return;
    }
    if data.is_null() || width == 0 || height == 0 {
        with_host(|h| h.frames_presented += 1);
        return;
    }

    let format = with_host(|h| h.pixel_format);
    let (w, h_) = (width as usize, height as usize);
    let mut pixels = vec![0u32; w * h_];

    // SAFETY: the core guarantees `height * pitch` readable bytes at `data`.
    unsafe {
        match format {
            RETRO_PIXEL_FORMAT_XRGB8888 => {
                for row in 0..h_ {
                    let src = (data as *const u8).add(row * pitch) as *const u32;
                    std::ptr::copy_nonoverlapping(src, pixels.as_mut_ptr().add(row * w), w);
                }
            }
            RETRO_PIXEL_FORMAT_RGB565 => {
                for row in 0..h_ {
                    let src = (data as *const u8).add(row * pitch) as *const u16;
                    for col in 0..w {
                        pixels[row * w + col] = rgb565_to_xrgb8888(*src.add(col));
                    }
                }
            }
            _ => {
                for row in 0..h_ {
                    let src = (data as *const u8).add(row * pitch) as *const u16;
                    for col in 0..w {
                        pixels[row * w + col] = rgb1555_to_xrgb8888(*src.add(col));
                    }
                }
            }
        }
    }

    with_host(|h| {
        h.video = VideoFrame {
            width,
            height,
            pixels,
        };
        h.frames_presented += 1;
    });
}

pub const fn rgb565_to_xrgb8888(p: u16) -> u32 {
    let r = ((p >> 11) & 0x1F) as u32;
    let g = ((p >> 5) & 0x3F) as u32;
    let b = (p & 0x1F) as u32;
    // Replicate the high bits into the low ones so full-scale stays full-scale.
    ((r << 3 | r >> 2) << 16) | ((g << 2 | g >> 4) << 8) | (b << 3 | b >> 2)
}

pub const fn rgb1555_to_xrgb8888(p: u16) -> u32 {
    let r = ((p >> 10) & 0x1F) as u32;
    let g = ((p >> 5) & 0x1F) as u32;
    let b = (p & 0x1F) as u32;
    ((r << 3 | r >> 2) << 16) | ((g << 3 | g >> 2) << 8) | (b << 3 | b >> 2)
}

/// # Safety
/// Called by the core; both arguments are plain integers.
pub unsafe extern "C" fn audio_sample(left: i16, right: i16) {
    with_host(|h| {
        if h.discard_output {
            return;
        }
        h.audio.push(left);
        h.audio.push(right);
    });
}

/// # Safety
/// `data` points at `frames * 2` interleaved samples.
pub unsafe extern "C" fn audio_sample_batch(data: *const i16, frames: usize) -> usize {
    let discard = with_host(|h| h.discard_output);
    if discard || data.is_null() {
        return frames;
    }
    // SAFETY: the core guarantees `frames * 2` readable samples at `data`.
    let slice = unsafe { std::slice::from_raw_parts(data, frames * 2) };
    with_host(|h| h.audio.extend_from_slice(slice));
    frames
}

/// # Safety
/// Called by the core; takes no arguments.
pub unsafe extern "C" fn input_poll() {
    with_host(|h| h.input_polls += 1);
}

/// # Safety
/// Called by the core; all arguments are plain integers.
pub unsafe extern "C" fn input_state(
    port: c_uint,
    device: c_uint,
    _index: c_uint,
    id: c_uint,
) -> i16 {
    if device != RETRO_DEVICE_JOYPAD || port > 1 {
        return 0;
    }
    let mask = with_host(|h| h.inputs[port as usize]);
    if id >= 16 {
        return 0;
    }
    i16::from(mask & (1 << id) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_conversion_preserves_the_extremes() {
        assert_eq!(rgb565_to_xrgb8888(0x0000), 0x0000_0000);
        assert_eq!(rgb565_to_xrgb8888(0xFFFF), 0x00FF_FFFF);
        assert_eq!(rgb1555_to_xrgb8888(0x0000), 0x0000_0000);
        assert_eq!(rgb1555_to_xrgb8888(0x7FFF), 0x00FF_FFFF);
    }

    #[test]
    fn pixel_conversion_keeps_channels_separate() {
        // Pure red in RGB565 is the top five bits.
        assert_eq!(rgb565_to_xrgb8888(0xF800), 0x00FF_0000);
        assert_eq!(rgb565_to_xrgb8888(0x07E0), 0x0000_FF00);
        assert_eq!(rgb565_to_xrgb8888(0x001F), 0x0000_00FF);
    }

    #[test]
    fn input_state_reads_the_requested_port() {
        with_host(|h| h.inputs = [0b0000_0001, 0b0000_1000]);
        unsafe {
            assert_eq!(input_state(0, RETRO_DEVICE_JOYPAD, 0, 0), 1);
            assert_eq!(input_state(0, RETRO_DEVICE_JOYPAD, 0, 3), 0);
            assert_eq!(input_state(1, RETRO_DEVICE_JOYPAD, 0, 3), 1);
            // Unknown devices and ports read as neutral rather than garbage.
            assert_eq!(input_state(0, 99, 0, 0), 0);
            assert_eq!(input_state(7, RETRO_DEVICE_JOYPAD, 0, 0), 0);
            assert_eq!(input_state(0, RETRO_DEVICE_JOYPAD, 0, 99), 0);
        }
        reset_state();
    }

    #[test]
    fn the_environment_rejects_an_unsupported_pixel_format() {
        let mut format: c_uint = 99;
        let ok = unsafe {
            environment(
                RETRO_ENVIRONMENT_SET_PIXEL_FORMAT,
                &mut format as *mut _ as *mut c_void,
            )
        };
        assert!(!ok);
    }

    #[test]
    fn the_environment_survives_null_pointers() {
        for cmd in [
            RETRO_ENVIRONMENT_SET_PIXEL_FORMAT,
            RETRO_ENVIRONMENT_GET_SYSTEM_DIRECTORY,
            RETRO_ENVIRONMENT_GET_SAVE_DIRECTORY,
            RETRO_ENVIRONMENT_GET_VARIABLE,
            RETRO_ENVIRONMENT_SET_GEOMETRY,
            RETRO_ENVIRONMENT_SET_SYSTEM_AV_INFO,
        ] {
            assert!(
                !unsafe { environment(cmd, std::ptr::null_mut()) },
                "cmd {cmd} must refuse a null pointer"
            );
        }
    }

    #[test]
    fn an_unknown_environment_command_is_refused_and_recorded() {
        reset_state();
        assert!(!unsafe { environment(9_999, std::ptr::null_mut()) });
        with_host(|h| assert!(h.unhandled_environment.contains(&9_999)));
        reset_state();
    }

    #[test]
    fn discarded_output_is_counted_and_not_stored() {
        reset_state();
        with_host(|h| h.discard_output = true);
        unsafe {
            audio_sample(1, 2);
            video_refresh(std::ptr::null(), 320, 240, 640);
        }
        with_host(|h| {
            assert!(h.audio.is_empty());
            assert!(h.video.is_empty());
            assert_eq!(h.frames_discarded, 1);
            assert_eq!(h.frames_presented, 0);
        });
        reset_state();
    }
}
