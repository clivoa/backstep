//! Loading and driving a libretro core as a rollback [`Simulation`].

use std::ffi::CString;
use std::os::raw::{c_uint, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use libloading::Library;
use rollback_core::{Button, Fnv1a, OutputMode, PlayerInput, Simulation, SimulationError};

use crate::ffi::*;
use crate::host;

/// Guards the single-instance rule: libretro cores keep their machine state in
/// process globals, so a second one in the same process would corrupt the first.
static CORE_LOADED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("failed to load core '{path}': {source}")]
    Load {
        path: PathBuf,
        #[source]
        source: libloading::Error,
    },
    #[error("core '{path}' is missing the symbol '{symbol}'")]
    MissingSymbol { path: PathBuf, symbol: &'static str },
    #[error("core speaks libretro API {got}, this host speaks {RETRO_API_VERSION}")]
    ApiVersion { got: c_uint },
    #[error("a libretro core is already loaded in this process")]
    AlreadyLoaded,
    #[error("core refused to load game '{path}'{}", crate::host::render_messages())]
    LoadGame { path: PathBuf },
    #[error("path '{0}' contains a NUL byte")]
    PathNotCString(PathBuf),
    #[error(
        "core reports a serialize size of zero, so no game is actually running.\n\
         \n\
         FBNeo returns success from retro_load_game even when the romset is \n\
         unusable, so a zero state size is how an incomplete romset surfaces \n\
         here. The core's own errors follow, if it logged any -- an \n\
         'is required' line names the file that is missing.\n\
         \n\
         Two cases that produce a complete-looking zip which still cannot run: \n\
         a CPS-2 set without its decryption key, and a Neo \n\
         Geo set without neogeo.zip, the BIOS, which must sit beside the game \n\
         or in the system directory.{}{}",
        crate::host::render_log_errors(),
        crate::host::render_messages()
    )]
    NoSerializeSupport,
    #[error("retro_serialize failed for a {0}-byte buffer")]
    SerializeFailed(usize),
    #[error("retro_unserialize rejected a {0}-byte state")]
    UnserializeFailed(usize),
    #[error("io error on '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

macro_rules! symbol {
    ($lib:expr, $path:expr, $name:literal, $ty:ty) => {{
        let name: &'static str = $name;
        // SAFETY: the caller has asserted that `$lib` is a libretro core, and
        // the signature matches libretro.h (see `ffi`).
        let sym: libloading::Symbol<'_, $ty> = unsafe {
            $lib.get(concat!($name, "\0").as_bytes())
                .map_err(|_| CoreError::MissingSymbol {
                    path: $path.clone(),
                    symbol: name,
                })?
        };
        *sym
    }};
}

/// The core's exported entry points, copied out as plain function pointers.
struct CoreApi {
    init: unsafe extern "C" fn(),
    deinit: unsafe extern "C" fn(),
    api_version: unsafe extern "C" fn() -> c_uint,
    get_system_info: unsafe extern "C" fn(*mut retro_system_info),
    get_system_av_info: unsafe extern "C" fn(*mut retro_system_av_info),
    set_environment: unsafe extern "C" fn(retro_environment_t),
    set_video_refresh: unsafe extern "C" fn(retro_video_refresh_t),
    set_audio_sample: unsafe extern "C" fn(retro_audio_sample_t),
    set_audio_sample_batch: unsafe extern "C" fn(retro_audio_sample_batch_t),
    set_input_poll: unsafe extern "C" fn(retro_input_poll_t),
    set_input_state: unsafe extern "C" fn(retro_input_state_t),
    set_controller_port_device: unsafe extern "C" fn(c_uint, c_uint),
    load_game: unsafe extern "C" fn(*const retro_game_info) -> bool,
    unload_game: unsafe extern "C" fn(),
    run: unsafe extern "C" fn(),
    reset: unsafe extern "C" fn(),
    serialize_size: unsafe extern "C" fn() -> usize,
    serialize: unsafe extern "C" fn(*mut c_void, usize) -> bool,
    unserialize: unsafe extern "C" fn(*const c_void, usize) -> bool,

    /// Declared last so it is dropped last: every pointer above points into it.
    _lib: Library,
}

/// A loaded libretro core with a game running in it.
pub struct LibretroCore {
    api: CoreApi,
    path: PathBuf,
    game_loaded: bool,
    state_size: usize,
    pub library_name: String,
    pub library_version: String,
}

impl LibretroCore {
    /// Load a core and wire it to the host callbacks.
    ///
    /// # Safety contract
    ///
    /// Loading a shared library and calling into it is inherently unsafe: this
    /// function trusts that `path` is a real libretro core built for this
    /// architecture. The mitigation is the handshake -- both peers compare the
    /// core's SHA-256 before a session starts, so a mismatched or tampered core
    /// fails as a refused connection rather than as undefined behaviour.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, CoreError> {
        let path = path.as_ref().to_path_buf();
        if CORE_LOADED.swap(true, Ordering::SeqCst) {
            return Err(CoreError::AlreadyLoaded);
        }

        let result = Self::load_inner(path);
        if result.is_err() {
            CORE_LOADED.store(false, Ordering::SeqCst);
        }
        result
    }

    fn load_inner(path: PathBuf) -> Result<Self, CoreError> {
        // SAFETY: dlopen runs the library's initialisers. See the safety
        // contract on `load`.
        let lib = unsafe { Library::new(&path) }.map_err(|source| CoreError::Load {
            path: path.clone(),
            source,
        })?;

        let api = CoreApi {
            init: symbol!(lib, path, "retro_init", unsafe extern "C" fn()),
            deinit: symbol!(lib, path, "retro_deinit", unsafe extern "C" fn()),
            api_version: symbol!(
                lib,
                path,
                "retro_api_version",
                unsafe extern "C" fn() -> c_uint
            ),
            get_system_info: symbol!(
                lib,
                path,
                "retro_get_system_info",
                unsafe extern "C" fn(*mut retro_system_info)
            ),
            get_system_av_info: symbol!(
                lib,
                path,
                "retro_get_system_av_info",
                unsafe extern "C" fn(*mut retro_system_av_info)
            ),
            set_environment: symbol!(
                lib,
                path,
                "retro_set_environment",
                unsafe extern "C" fn(retro_environment_t)
            ),
            set_video_refresh: symbol!(
                lib,
                path,
                "retro_set_video_refresh",
                unsafe extern "C" fn(retro_video_refresh_t)
            ),
            set_audio_sample: symbol!(
                lib,
                path,
                "retro_set_audio_sample",
                unsafe extern "C" fn(retro_audio_sample_t)
            ),
            set_audio_sample_batch: symbol!(
                lib,
                path,
                "retro_set_audio_sample_batch",
                unsafe extern "C" fn(retro_audio_sample_batch_t)
            ),
            set_input_poll: symbol!(
                lib,
                path,
                "retro_set_input_poll",
                unsafe extern "C" fn(retro_input_poll_t)
            ),
            set_input_state: symbol!(
                lib,
                path,
                "retro_set_input_state",
                unsafe extern "C" fn(retro_input_state_t)
            ),
            set_controller_port_device: symbol!(
                lib,
                path,
                "retro_set_controller_port_device",
                unsafe extern "C" fn(c_uint, c_uint)
            ),
            load_game: symbol!(
                lib,
                path,
                "retro_load_game",
                unsafe extern "C" fn(*const retro_game_info) -> bool
            ),
            unload_game: symbol!(lib, path, "retro_unload_game", unsafe extern "C" fn()),
            run: symbol!(lib, path, "retro_run", unsafe extern "C" fn()),
            reset: symbol!(lib, path, "retro_reset", unsafe extern "C" fn()),
            serialize_size: symbol!(
                lib,
                path,
                "retro_serialize_size",
                unsafe extern "C" fn() -> usize
            ),
            serialize: symbol!(
                lib,
                path,
                "retro_serialize",
                unsafe extern "C" fn(*mut c_void, usize) -> bool
            ),
            unserialize: symbol!(
                lib,
                path,
                "retro_unserialize",
                unsafe extern "C" fn(*const c_void, usize) -> bool
            ),
            _lib: lib,
        };

        // SAFETY: symbols resolved above; the API version check comes first so
        // an incompatible core is rejected before anything else is called.
        let got = unsafe { (api.api_version)() };
        if got != RETRO_API_VERSION {
            return Err(CoreError::ApiVersion { got });
        }

        host::reset_state();

        // The environment callback must be installed *before* retro_init: many
        // cores query options and directories from inside it.
        // SAFETY: all callbacks have the signatures from libretro.h.
        unsafe {
            (api.set_environment)(host::environment);
            (api.set_video_refresh)(host::video_refresh);
            (api.set_audio_sample)(host::audio_sample);
            (api.set_audio_sample_batch)(host::audio_sample_batch);
            (api.set_input_poll)(host::input_poll);
            (api.set_input_state)(host::input_state);
            (api.init)();
        }

        let mut info = retro_system_info {
            library_name: std::ptr::null(),
            library_version: std::ptr::null(),
            valid_extensions: std::ptr::null(),
            need_fullpath: false,
            block_extract: false,
        };
        // SAFETY: `info` is a valid, writable retro_system_info.
        unsafe { (api.get_system_info)(&mut info) };
        let library_name = cstr_to_string(info.library_name);
        let library_version = cstr_to_string(info.library_version);

        Ok(LibretroCore {
            api,
            path,
            game_loaded: false,
            state_size: 0,
            library_name,
            library_version,
        })
    }

    /// True when the core wants a filesystem path rather than a data buffer.
    pub fn needs_fullpath(&self) -> bool {
        let mut info = retro_system_info {
            library_name: std::ptr::null(),
            library_version: std::ptr::null(),
            valid_extensions: std::ptr::null(),
            need_fullpath: false,
            block_extract: false,
        };
        // SAFETY: `info` is a valid, writable retro_system_info.
        unsafe { (self.api.get_system_info)(&mut info) };
        info.need_fullpath
    }

    /// Load a ROM and put both ports on a RetroPad.
    pub fn load_game(&mut self, rom: &Path) -> Result<(), CoreError> {
        let path_c = CString::new(rom.as_os_str().as_encoded_bytes())
            .map_err(|_| CoreError::PathNotCString(rom.to_path_buf()))?;

        // Cores that parse the archive themselves want the path; the rest want
        // the bytes. Reading the file either way keeps `data` alive across the
        // call, which the API requires.
        let bytes = if self.needs_fullpath() {
            Vec::new()
        } else {
            std::fs::read(rom).map_err(|source| CoreError::Io {
                path: rom.to_path_buf(),
                source,
            })?
        };

        let info = retro_game_info {
            path: path_c.as_ptr(),
            data: if bytes.is_empty() {
                std::ptr::null()
            } else {
                bytes.as_ptr() as *const c_void
            },
            size: bytes.len(),
            meta: std::ptr::null(),
        };

        // SAFETY: `info` and everything it points at outlive the call.
        let ok = unsafe { (self.api.load_game)(&info) };
        if !ok {
            return Err(CoreError::LoadGame {
                path: rom.to_path_buf(),
            });
        }
        self.game_loaded = true;

        // SAFETY: the game is loaded, so these are legal to call.
        unsafe {
            (self.api.set_controller_port_device)(0, RETRO_DEVICE_JOYPAD);
            (self.api.set_controller_port_device)(1, RETRO_DEVICE_JOYPAD);
        }

        let mut av = retro_system_av_info::default();
        // SAFETY: `av` is a valid, writable retro_system_av_info.
        unsafe { (self.api.get_system_av_info)(&mut av) };
        host::with_host(|h| {
            h.geometry = av.geometry;
            h.timing = av.timing;
        });

        // SAFETY: serialize size is only meaningful once a game is loaded.
        self.state_size = unsafe { (self.api.serialize_size)() };
        if self.state_size == 0 {
            return Err(CoreError::NoSerializeSupport);
        }
        Ok(())
    }

    /// Bytes one snapshot occupies. Fixed for the lifetime of a loaded game.
    pub fn state_size(&self) -> usize {
        self.state_size
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Nominal frame rate and sample rate the core reported.
    pub fn av_timing(&self) -> retro_system_timing {
        host::with_host(|h| h.timing)
    }

    pub fn geometry(&self) -> retro_game_geometry {
        host::with_host(|h| h.geometry)
    }

    fn run_frame(&mut self) {
        // SAFETY: a game is loaded and the callbacks are installed.
        unsafe { (self.api.run)() };
    }

    /// Reset the machine, as if the arcade board were power-cycled.
    ///
    /// Used before the boot script so both peers start from an identical
    /// machine regardless of what ran before.
    pub fn reset(&mut self) {
        // SAFETY: legal at any point after `retro_init`.
        unsafe { (self.api.reset)() };
    }
}

impl std::fmt::Debug for LibretroCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibretroCore")
            .field("path", &self.path)
            .field("library_name", &self.library_name)
            .field("library_version", &self.library_version)
            .field("game_loaded", &self.game_loaded)
            .field("state_size", &self.state_size)
            .finish()
    }
}

impl Drop for LibretroCore {
    fn drop(&mut self) {
        // SAFETY: mirrors the load order exactly, once.
        unsafe {
            if self.game_loaded {
                (self.api.unload_game)();
            }
            (self.api.deinit)();
        }
        CORE_LOADED.store(false, Ordering::SeqCst);
    }
}

fn cstr_to_string(p: *const std::os::raw::c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: libretro guarantees a NUL-terminated static string.
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

/// Map the lab's logical buttons onto the RetroPad.
///
/// The layout follows FBNeo's default CPS-2 assignment, so the six attack
/// buttons land where a Fightcade player would expect them:
///
/// | logical  | RetroPad | Neo Geo         |
/// |----------|----------|-----------------|
/// | Attack   | Y        | light punch     |
/// | Special  | X        | medium punch    |
/// | Block    | B        | light kick      |
/// | Confirm  | A        | medium kick     |
/// | Start    | START    | start           |
/// | Coin     | SELECT   | insert coin     |
pub fn to_retropad(input: PlayerInput) -> u16 {
    let mut mask = 0u16;
    let mut set = |id: c_uint| mask |= 1 << id;
    if input.contains(Button::Up) {
        set(RETRO_DEVICE_ID_JOYPAD_UP);
    }
    if input.contains(Button::Down) {
        set(RETRO_DEVICE_ID_JOYPAD_DOWN);
    }
    if input.contains(Button::Left) {
        set(RETRO_DEVICE_ID_JOYPAD_LEFT);
    }
    if input.contains(Button::Right) {
        set(RETRO_DEVICE_ID_JOYPAD_RIGHT);
    }
    if input.contains(Button::Attack) {
        set(RETRO_DEVICE_ID_JOYPAD_Y);
    }
    if input.contains(Button::Special) {
        set(RETRO_DEVICE_ID_JOYPAD_X);
    }
    if input.contains(Button::Block) {
        set(RETRO_DEVICE_ID_JOYPAD_B);
    }
    if input.contains(Button::Confirm) {
        set(RETRO_DEVICE_ID_JOYPAD_A);
    }
    if input.contains(Button::Start) {
        set(RETRO_DEVICE_ID_JOYPAD_START);
    }
    if input.contains(Button::Coin) {
        set(RETRO_DEVICE_ID_JOYPAD_SELECT);
    }
    mask
}

/// A loaded core presented as a rollback [`Simulation`].
pub struct LibretroSimulation {
    core: LibretroCore,
    /// Checksum of the last snapshot taken, so the session's `save_state` +
    /// `checksum` pair costs one `retro_serialize` rather than two. For a
    /// multi-megabyte CPS-2 state at 60 Hz that difference is most of the frame
    /// budget. Invalidated by anything that changes the machine state.
    cached_checksum: std::cell::Cell<Option<u64>>,
    /// Bytes at the head of the state that [`Simulation::checksum`] ignores.
    ///
    /// Zero unless the caller asks for otherwise, because "ignore part of the
    /// state" is a claim about one specific core's savestate layout and must
    /// never be a silent default.
    checksum_skip: usize,
    /// Frames advanced in `Present` mode.
    pub presented_frames: u64,
    pub resimulated_frames: u64,
}

impl LibretroSimulation {
    pub fn new(core: LibretroCore) -> Self {
        LibretroSimulation {
            core,
            cached_checksum: std::cell::Cell::new(None),
            checksum_skip: 0,
            presented_frames: 0,
            resimulated_frames: 0,
        }
    }

    /// Ignore the first `bytes` of the savestate when checksumming.
    ///
    /// For FBNeo this is [`CHECKSUM_SKIP_BYTES`], and the reasoning is there.
    /// It is opt-in rather than the default because a skip is only correct for
    /// the core it was measured against.
    ///
    /// # Panics
    ///
    /// If the skip would swallow half the state or more. A skip that covers
    /// everything makes `checksum` a constant, and a constant checksum agrees
    /// with itself forever -- desync detection would silently become a
    /// no-op. This is not hypothetical: it is what happened the first time,
    /// against a test core with a 32-byte state, and the assertion is here so
    /// that it fails loudly instead.
    pub fn with_checksum_skip(mut self, bytes: usize) -> Self {
        let size = self.core.state_size;
        assert!(
            bytes * 2 < size,
            "a checksum skip of {bytes} bytes leaves nothing of a {size}-byte state to compare"
        );
        self.checksum_skip = bytes;
        self.cached_checksum.set(None);
        self
    }

    pub fn core(&self) -> &LibretroCore {
        &self.core
    }

    /// Power-cycle the emulated board.
    ///
    /// Used by the boot-calibration tools to put a fresh machine in front of
    /// each candidate timing, and by nothing on the session path -- a session
    /// resets once, before the first frame, so that both peers start from the
    /// same machine regardless of what ran before.
    pub fn reset_machine(&mut self) {
        self.core.reset();
        self.cached_checksum.set(None);
    }

    /// The most recent frame the core produced in `Present` mode.
    pub fn video(&self) -> host::VideoFrame {
        host::with_host(|h| h.video.clone())
    }

    /// Take the audio accumulated since the last call.
    pub fn take_audio(&self) -> Vec<i16> {
        host::with_host(|h| std::mem::take(&mut h.audio))
    }

    /// Serialise the machine state and hash it in one pass.
    fn snapshot(&self) -> Vec<u8> {
        let size = self.core.state_size;
        let mut buffer = vec![0u8; size];
        // SAFETY: `buffer` is exactly `size` bytes, which is what the core asked
        // for through `retro_serialize_size`.
        let ok = unsafe { (self.core.api.serialize)(buffer.as_mut_ptr() as *mut c_void, size) };
        if !ok {
            // A core that cannot serialise cannot roll back, and returning a
            // short buffer would let the session load garbage several frames
            // later, where it would look like a desync instead of a core bug.
            panic!("{}", CoreError::SerializeFailed(size));
        }
        let mut h = Fnv1a::new();
        h.write(&buffer[self.checksum_skip..]);
        self.cached_checksum.set(Some(h.finish()));
        buffer
    }
}

/// Bytes at the head of a libretro savestate that the desync checksum ignores.
///
/// # Why a checksum would otherwise lie
///
/// Rollback needs `retro_unserialize` to restore everything `retro_run` will go
/// on to read. FBNeo very nearly does: `examples/check-rollback-safety.rs` saves
/// a state, runs on, restores, replays the identical inputs and compares. Out of
/// **415 155 bytes**, save -> load -> save disagrees on **16 to 21**, always in
/// four four-byte fields at offsets 537, 829, 1413 and 1705. They are not
/// restored, they are recomputed.
///
/// The question that matters is whether that spreads. It does not:
///
/// ```text
/// probe at frame 2100, 300 replayed frames of a live match
///   -> 18 bytes differ, highest offset 1761
/// probe at frame 2500 -> 23 bytes differ, highest offset 1757
/// probe at frame 2900 -> 17 bytes differ, highest offset 1499
/// ```
///
/// Five seconds of re-simulated fighting and the difference is still a couple of
/// dozen bytes below offset 1800. It never reaches the 413 KB of work RAM,
/// video RAM and palette where the actual game lives -- this is sound-chip and
/// timer bookkeeping, which the 68000 does not read back.
///
/// So the machine *is* rollback-safe and the checksum was not. Hashing the
/// whole blob reported a desync on the first rollback of every session, which is
/// a false alarm that makes the desync detector worthless on this core.
///
/// # What this costs
///
/// A genuine divergence confined to these first 2 KB would go unnoticed. That is
/// the trade, stated plainly. It is worth taking because the alternative is a
/// detector that fires every time, and because everything the players can
/// observe lives past this boundary.
///
/// 2048 is the measured maximum (1761) rounded up with room to spare.
/// `check-rollback-safety` fails if instability ever reaches this offset, so the
/// claim is checkable rather than a hope.
pub const CHECKSUM_SKIP_BYTES: usize = 2048;

impl Simulation for LibretroSimulation {
    fn save_state(&self) -> Vec<u8> {
        self.snapshot()
    }

    fn load_state(&mut self, data: &[u8]) -> Result<(), SimulationError> {
        if data.len() != self.core.state_size {
            return Err(SimulationError::StateSize {
                expected: self.core.state_size,
                actual: data.len(),
            });
        }
        // SAFETY: `data` is exactly the size the core reported.
        let ok = unsafe { (self.core.api.unserialize)(data.as_ptr() as *const c_void, data.len()) };
        if !ok {
            return Err(SimulationError::Backend(
                CoreError::UnserializeFailed(data.len()).to_string(),
            ));
        }
        self.cached_checksum.set(None);
        Ok(())
    }

    fn advance_frame(&mut self, inputs: [PlayerInput; 2], output_mode: OutputMode) {
        let discard = !output_mode.emits_output();
        host::with_host(|h| {
            h.inputs = [to_retropad(inputs[0]), to_retropad(inputs[1])];
            h.discard_output = discard;
        });
        self.core.run_frame();
        host::with_host(|h| h.discard_output = false);
        self.cached_checksum.set(None);
        if discard {
            self.resimulated_frames += 1;
        } else {
            self.presented_frames += 1;
        }
    }

    fn checksum(&self) -> u64 {
        if let Some(checksum) = self.cached_checksum.get() {
            return checksum;
        }
        self.snapshot();
        self.cached_checksum
            .get()
            .expect("snapshot fills the cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retropad_mapping_is_one_bit_per_logical_button() {
        assert_eq!(to_retropad(PlayerInput::NEUTRAL), 0);
        assert_eq!(
            to_retropad(PlayerInput::NEUTRAL.with(Button::Attack)),
            1 << RETRO_DEVICE_ID_JOYPAD_Y
        );
        assert_eq!(
            to_retropad(PlayerInput::NEUTRAL.with(Button::Coin)),
            1 << RETRO_DEVICE_ID_JOYPAD_SELECT
        );

        // Every logical button must map somewhere distinct.
        let mut seen = 0u16;
        for b in Button::ALL {
            let mask = to_retropad(PlayerInput::NEUTRAL.with(b));
            assert_ne!(mask, 0, "{b:?} maps to nothing");
            assert_eq!(seen & mask, 0, "{b:?} collides with an earlier button");
            seen |= mask;
        }
    }

    #[test]
    fn combined_inputs_or_together() {
        let input = PlayerInput::NEUTRAL
            .with(Button::Down)
            .with(Button::Right)
            .with(Button::Attack);
        let mask = to_retropad(input);
        assert_eq!(
            mask,
            (1 << RETRO_DEVICE_ID_JOYPAD_DOWN)
                | (1 << RETRO_DEVICE_ID_JOYPAD_RIGHT)
                | (1 << RETRO_DEVICE_ID_JOYPAD_Y)
        );
    }

    #[test]
    fn loading_a_file_that_is_not_a_core_fails_cleanly() {
        let err = LibretroCore::load("/nonexistent/definitely-not-a-core.so").unwrap_err();
        assert!(matches!(err, CoreError::Load { .. }));
        // The single-instance latch must have been released again.
        assert!(!CORE_LOADED.load(Ordering::SeqCst));
    }
}
