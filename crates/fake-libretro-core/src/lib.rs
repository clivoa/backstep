//! A minimal but genuine libretro core, built as a `cdylib` for CI.
//!
//! The point is to exercise the *real* FFI path -- `dlopen`, the environment
//! callback, `retro_run`, `retro_serialize`/`retro_unserialize`, the video and
//! audio callbacks -- with no ROM involved. CI can therefore test everything
//! about the libretro host except the emulator itself, on a machine that has
//! never seen a protected ROM.
//!
//! The "game" is a deterministic 1D chase: two dots on a 64-pixel line move on
//! input and the state is 32 bytes. It has no gameplay value; what it has is a
//! state that changes every frame, so a missed or duplicated `retro_run` is
//! visible in the checksum.

#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_uint, c_void};
use std::sync::Mutex;

pub const WIDTH: c_uint = 64;
pub const HEIGHT: c_uint = 32;
/// Bytes in a serialised state.
pub const STATE_SIZE: usize = 32;

const RETRO_ENVIRONMENT_SET_PIXEL_FORMAT: c_uint = 10;
const RETRO_PIXEL_FORMAT_XRGB8888: c_uint = 1;
const RETRO_DEVICE_JOYPAD: c_uint = 1;
const ID_UP: c_uint = 4;
const ID_DOWN: c_uint = 5;
const ID_LEFT: c_uint = 6;
const ID_RIGHT: c_uint = 7;
const ID_Y: c_uint = 1;

#[repr(C)]
pub struct retro_system_info {
    pub library_name: *const c_char,
    pub library_version: *const c_char,
    pub valid_extensions: *const c_char,
    pub need_fullpath: bool,
    pub block_extract: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct retro_game_geometry {
    pub base_width: c_uint,
    pub base_height: c_uint,
    pub max_width: c_uint,
    pub max_height: c_uint,
    pub aspect_ratio: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct retro_system_timing {
    pub fps: f64,
    pub sample_rate: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct retro_system_av_info {
    pub geometry: retro_game_geometry,
    pub timing: retro_system_timing,
}

#[repr(C)]
pub struct retro_game_info {
    pub path: *const c_char,
    pub data: *const c_void,
    pub size: usize,
    pub meta: *const c_char,
}

type retro_environment_t = unsafe extern "C" fn(c_uint, *mut c_void) -> bool;
type retro_video_refresh_t = unsafe extern "C" fn(*const c_void, c_uint, c_uint, usize);
type retro_audio_sample_t = unsafe extern "C" fn(i16, i16);
type retro_audio_sample_batch_t = unsafe extern "C" fn(*const i16, usize) -> usize;
type retro_input_poll_t = unsafe extern "C" fn();
type retro_input_state_t = unsafe extern "C" fn(c_uint, c_uint, c_uint, c_uint) -> i16;

#[derive(Default)]
struct Callbacks {
    environment: Option<retro_environment_t>,
    video: Option<retro_video_refresh_t>,
    audio: Option<retro_audio_sample_t>,
    audio_batch: Option<retro_audio_sample_batch_t>,
    poll: Option<retro_input_poll_t>,
    input: Option<retro_input_state_t>,
}

/// The whole "machine": two positions, a frame counter and an accumulator.
#[derive(Clone, Copy, Default)]
pub struct Machine {
    pub frame: u64,
    pub x: [i32; 2],
    pub y: [i32; 2],
    pub score: [u32; 2],
}

impl Machine {
    fn step(&mut self, inputs: [u16; 2]) {
        for p in 0..2 {
            let held = |id: c_uint| inputs[p] & (1 << id) != 0;
            if held(ID_LEFT) {
                self.x[p] -= 1;
            }
            if held(ID_RIGHT) {
                self.x[p] += 1;
            }
            if held(ID_UP) {
                self.y[p] -= 1;
            }
            if held(ID_DOWN) {
                self.y[p] += 1;
            }
            self.x[p] = self.x[p].rem_euclid(WIDTH as i32);
            self.y[p] = self.y[p].rem_euclid(HEIGHT as i32);
            // Scoring mixes in the frame number, so replaying the same inputs
            // out of order gives a different state.
            if held(ID_Y) {
                self.score[p] = self.score[p]
                    .wrapping_mul(31)
                    .wrapping_add(self.frame as u32)
                    .wrapping_add(self.x[p] as u32);
            }
        }
        self.frame += 1;
    }

    fn serialize(&self) -> [u8; STATE_SIZE] {
        let mut out = [0u8; STATE_SIZE];
        out[0..8].copy_from_slice(&self.frame.to_le_bytes());
        out[8..12].copy_from_slice(&self.x[0].to_le_bytes());
        out[12..16].copy_from_slice(&self.x[1].to_le_bytes());
        out[16..20].copy_from_slice(&self.y[0].to_le_bytes());
        out[20..24].copy_from_slice(&self.y[1].to_le_bytes());
        out[24..28].copy_from_slice(&self.score[0].to_le_bytes());
        out[28..32].copy_from_slice(&self.score[1].to_le_bytes());
        out
    }

    fn deserialize(bytes: &[u8]) -> Option<Machine> {
        if bytes.len() != STATE_SIZE {
            return None;
        }
        let w32 = |at: usize| i32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let u32_ = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        Some(Machine {
            frame: u64::from_le_bytes(bytes[0..8].try_into().unwrap()),
            x: [w32(8), w32(12)],
            y: [w32(16), w32(20)],
            score: [u32_(24), u32_(28)],
        })
    }
}

struct CoreState {
    callbacks: Callbacks,
    machine: Machine,
    game_loaded: bool,
    framebuffer: Vec<u32>,
}

impl Default for CoreState {
    fn default() -> Self {
        CoreState {
            callbacks: Callbacks::default(),
            machine: Machine {
                x: [16, 48],
                y: [16, 16],
                ..Default::default()
            },
            game_loaded: false,
            framebuffer: vec![0; (WIDTH * HEIGHT) as usize],
        }
    }
}

static STATE: Mutex<Option<CoreState>> = Mutex::new(None);

fn with_state<R>(f: impl FnOnce(&mut CoreState) -> R) -> R {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(CoreState::default))
}

// --- libretro exports ------------------------------------------------------

#[no_mangle]
pub extern "C" fn retro_api_version() -> c_uint {
    1
}

#[no_mangle]
pub extern "C" fn retro_init() {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let callbacks = guard.take().map(|s| s.callbacks).unwrap_or_default();
    *guard = Some(CoreState {
        callbacks,
        ..Default::default()
    });
}

#[no_mangle]
pub extern "C" fn retro_deinit() {
    let mut guard = STATE.lock().unwrap_or_else(|e| e.into_inner());
    *guard = None;
}

/// # Safety
/// `info` must point at a writable `retro_system_info`.
#[no_mangle]
pub unsafe extern "C" fn retro_get_system_info(info: *mut retro_system_info) {
    if info.is_null() {
        return;
    }
    unsafe {
        (*info).library_name = c"fake-libretro-core".as_ptr();
        (*info).library_version = c"0.1.0".as_ptr();
        (*info).valid_extensions = c"fake|zip".as_ptr();
        (*info).need_fullpath = false;
        (*info).block_extract = true;
    }
}

/// # Safety
/// `info` must point at a writable `retro_system_av_info`.
#[no_mangle]
pub unsafe extern "C" fn retro_get_system_av_info(info: *mut retro_system_av_info) {
    if info.is_null() {
        return;
    }
    unsafe {
        (*info).geometry = retro_game_geometry {
            base_width: WIDTH,
            base_height: HEIGHT,
            max_width: WIDTH,
            max_height: HEIGHT,
            aspect_ratio: WIDTH as f32 / HEIGHT as f32,
        };
        (*info).timing = retro_system_timing {
            fps: 60.0,
            sample_rate: 48_000.0,
        };
    }
}

#[no_mangle]
pub extern "C" fn retro_set_environment(cb: retro_environment_t) {
    with_state(|s| s.callbacks.environment = Some(cb));
    // Announce the pixel format the way a real core does, so the host's
    // environment handler is exercised.
    let mut format: c_uint = RETRO_PIXEL_FORMAT_XRGB8888;
    // SAFETY: `cb` came from the frontend and `format` is a valid c_uint.
    unsafe { cb(RETRO_ENVIRONMENT_SET_PIXEL_FORMAT, &mut format as *mut _ as *mut c_void) };
}

#[no_mangle]
pub extern "C" fn retro_set_video_refresh(cb: retro_video_refresh_t) {
    with_state(|s| s.callbacks.video = Some(cb));
}

#[no_mangle]
pub extern "C" fn retro_set_audio_sample(cb: retro_audio_sample_t) {
    with_state(|s| s.callbacks.audio = Some(cb));
}

#[no_mangle]
pub extern "C" fn retro_set_audio_sample_batch(cb: retro_audio_sample_batch_t) {
    with_state(|s| s.callbacks.audio_batch = Some(cb));
}

#[no_mangle]
pub extern "C" fn retro_set_input_poll(cb: retro_input_poll_t) {
    with_state(|s| s.callbacks.poll = Some(cb));
}

#[no_mangle]
pub extern "C" fn retro_set_input_state(cb: retro_input_state_t) {
    with_state(|s| s.callbacks.input = Some(cb));
}

#[no_mangle]
pub extern "C" fn retro_set_controller_port_device(_port: c_uint, _device: c_uint) {}

/// # Safety
/// `_info` is either null or a valid `retro_game_info`.
#[no_mangle]
pub unsafe extern "C" fn retro_load_game(_info: *const retro_game_info) -> bool {
    with_state(|s| {
        s.game_loaded = true;
        s.machine = CoreState::default().machine;
    });
    true
}

#[no_mangle]
pub extern "C" fn retro_unload_game() {
    with_state(|s| s.game_loaded = false);
}

#[no_mangle]
pub extern "C" fn retro_reset() {
    with_state(|s| s.machine = CoreState::default().machine);
}

#[no_mangle]
pub extern "C" fn retro_run() {
    let (poll, input, video, audio_batch, loaded) = with_state(|s| {
        (
            s.callbacks.poll,
            s.callbacks.input,
            s.callbacks.video,
            s.callbacks.audio_batch,
            s.game_loaded,
        )
    });
    if !loaded {
        return;
    }

    if let Some(poll) = poll {
        // SAFETY: installed by the frontend.
        unsafe { poll() };
    }

    let mut inputs = [0u16; 2];
    if let Some(input) = input {
        for (port, slot) in inputs.iter_mut().enumerate() {
            for id in 0..16u32 {
                // SAFETY: installed by the frontend; all arguments are integers.
                if unsafe { input(port as c_uint, RETRO_DEVICE_JOYPAD, 0, id) } != 0 {
                    *slot |= 1 << id;
                }
            }
        }
    }

    with_state(|s| {
        s.machine.step(inputs);
        // Paint the two dots so the frontend has something real to convert.
        s.framebuffer.fill(0);
        for p in 0..2 {
            let idx = (s.machine.y[p] as usize) * WIDTH as usize + s.machine.x[p] as usize;
            s.framebuffer[idx] = if p == 0 { 0x00FF_0000 } else { 0x0000_00FF };
        }
        if let Some(video) = video {
            // SAFETY: the buffer is WIDTH*HEIGHT u32s with a WIDTH*4 pitch.
            unsafe {
                video(
                    s.framebuffer.as_ptr() as *const c_void,
                    WIDTH,
                    HEIGHT,
                    WIDTH as usize * 4,
                )
            };
        }
    });

    if let Some(batch) = audio_batch {
        // 800 stereo frames is one frame's worth at 48 kHz / 60 Hz.
        let samples = [0i16; 1600];
        // SAFETY: 800 frames of interleaved stereo, as declared.
        unsafe { batch(samples.as_ptr(), 800) };
    }
}

#[no_mangle]
pub extern "C" fn retro_serialize_size() -> usize {
    STATE_SIZE
}

/// # Safety
/// `data` must point at `size` writable bytes.
#[no_mangle]
pub unsafe extern "C" fn retro_serialize(data: *mut c_void, size: usize) -> bool {
    if data.is_null() || size < STATE_SIZE {
        return false;
    }
    let bytes = with_state(|s| s.machine.serialize());
    // SAFETY: `data` has at least STATE_SIZE writable bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, STATE_SIZE) };
    true
}

/// # Safety
/// `data` must point at `size` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn retro_unserialize(data: *const c_void, size: usize) -> bool {
    if data.is_null() || size != STATE_SIZE {
        return false;
    }
    // SAFETY: `data` has `size` readable bytes.
    let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, size) };
    match Machine::deserialize(bytes) {
        Some(machine) => {
            with_state(|s| s.machine = machine);
            true
        }
        None => false,
    }
}

#[no_mangle]
pub extern "C" fn retro_get_region() -> c_uint {
    0
}

/// # Safety
/// Part of the libretro ABI; this core exposes no memory regions.
#[no_mangle]
pub unsafe extern "C" fn retro_get_memory_data(_id: c_uint) -> *mut c_void {
    std::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn retro_get_memory_size(_id: c_uint) -> usize {
    0
}

/// # Safety
/// Part of the libretro ABI; subsystems are unsupported.
#[no_mangle]
pub unsafe extern "C" fn retro_load_game_special(
    _game_type: c_uint,
    _info: *const retro_game_info,
    _num: usize,
) -> bool {
    false
}

/// # Safety
/// Part of the libretro ABI; cheats are unsupported.
#[no_mangle]
pub unsafe extern "C" fn retro_cheat_reset() {}

/// # Safety
/// Part of the libretro ABI; cheats are unsupported.
#[no_mangle]
pub unsafe extern "C" fn retro_cheat_set(_index: c_uint, _enabled: bool, _code: *const c_char) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_round_trips_through_the_byte_form() {
        let m = Machine {
            frame: 4242,
            x: [7, -3],
            y: [1, 31],
            score: [900, 12],
        };
        let bytes = m.serialize();
        let back = Machine::deserialize(&bytes).unwrap();
        assert_eq!(back.frame, m.frame);
        assert_eq!(back.x, m.x);
        assert_eq!(back.y, m.y);
        assert_eq!(back.score, m.score);
    }

    #[test]
    fn a_wrong_sized_state_is_rejected() {
        assert!(Machine::deserialize(&[0u8; 8]).is_none());
    }

    #[test]
    fn positions_wrap_rather_than_going_negative() {
        let mut m = Machine::default();
        for _ in 0..10 {
            m.step([1 << ID_LEFT, 0]);
        }
        assert!(m.x[0] >= 0 && m.x[0] < WIDTH as i32);
    }

    #[test]
    fn the_same_inputs_give_the_same_state() {
        let script: Vec<[u16; 2]> = (0..500).map(|f: u16| [f & 0xFF, (f * 3) & 0xFF]).collect();
        let mut a = Machine::default();
        let mut b = Machine::default();
        for inputs in &script {
            a.step(*inputs);
            b.step(*inputs);
        }
        assert_eq!(a.serialize(), b.serialize());
    }

    #[test]
    fn input_order_changes_the_state() {
        let mut a = Machine::default();
        let mut b = Machine::default();
        a.step([1 << ID_Y, 0]);
        a.step([1 << ID_RIGHT, 0]);
        b.step([1 << ID_RIGHT, 0]);
        b.step([1 << ID_Y, 0]);
        assert_ne!(a.serialize(), b.serialize());
    }
}
