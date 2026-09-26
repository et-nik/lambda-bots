//! Static library linked into the Metamod module: exported `lb_core_*` functions.
//!
//! Every entry point runs inside [`guard`]: re-entrant calls are refused, panics are caught and
//! switch the runtime into safe mode instead of unwinding into the engine.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Once;
use std::time::Instant;

use lb_core::handles::MapEpoch;
use lb_ffi::*;
use lb_host::Host;
use lb_host::ffi_host::FfiHost;
use lb_runtime::{InitData, Runtime, commands, panic_message};
use parking_lot::Mutex;

struct Plugin {
    host: FfiHost,
    rt: Runtime,
}

static PLUGIN: Mutex<Option<Plugin>> = Mutex::new(None);
static PANIC_HOOK: Once = Once::new();

fn guard<R>(default: R, f: impl FnOnce(&mut Plugin) -> R) -> R {
    let Some(mut lock) = PLUGIN.try_lock() else {
        return default;
    };
    let Some(plugin) = lock.as_mut() else { return default };
    match catch_unwind(AssertUnwindSafe(|| f(plugin))) {
        Ok(r) => r,
        Err(payload) => {
            let msg = panic_message(&payload);
            plugin.rt.safe_mode = Some(format!("core panic: {msg}"));
            plugin
                .host
                .server_print(&format!("[lambdabots] core panic, entering safe mode: {msg}\n"));
            default
        }
    }
}

/// # Safety
/// `s` must be valid for `len` bytes during the call.
unsafe fn string(s: LbStr) -> String {
    // SAFETY: guaranteed by the caller.
    String::from_utf8_lossy(unsafe { s.as_bytes() }).into_owned()
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        std::panic::set_hook(Box::new(|info| {
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_default();
            tracing::error!("panic at {location}: {}", panic_payload(info));
            let backtrace = std::backtrace::Backtrace::force_capture();
            tracing::error!(target: lb_runtime::logging::FILE_ONLY, "backtrace:\n{backtrace}");
        }));
    });
}

fn panic_payload(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

static CORE_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

/// Initializes the core. Returns `LB_OK` or an error status; `result` is always filled.
///
/// # Safety
/// Called by the adapter on the engine main thread with valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lb_core_init(
    host: *const LbHostApi,
    info: *const LbInitInfo,
    result: *mut LbInitResult,
) -> i32 {
    let status = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the adapter passes a valid init struct.
        let Some(info) = (unsafe { info.as_ref() }) else {
            return LB_ERR_INVALID;
        };
        // SAFETY: the host table outlives the plugin.
        let Some(mut ffi_host) = (unsafe { FfiHost::new(host) }) else {
            return LB_ERR_ABI;
        };
        if info.abi_version != LB_ABI_VERSION {
            ffi_host.server_print(&format!(
                "[lambdabots] ABI mismatch: adapter {} core {LB_ABI_VERSION}\n",
                info.abi_version
            ));
            return LB_ERR_ABI;
        }
        let ours = abi_sizes();
        let mismatches: Vec<String> = ours
            .iter()
            .zip(info.sizes.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b && **a != 0)
            .map(|(i, (a, b))| format!("#{i}: core {a} adapter {b}"))
            .collect();
        if !mismatches.is_empty() {
            ffi_host.server_print(&format!(
                "[lambdabots] ABI struct size mismatch: {}\n",
                mismatches.join(", ")
            ));
            return LB_ERR_ABI;
        }
        install_panic_hook();
        // SAFETY: strings are valid during the call.
        let init = unsafe {
            InitData {
                adapter_version: string(info.adapter_version),
                plugin_path: PathBuf::from(string(info.plugin_path)),
                game_dir: PathBuf::from(string(info.game_dir)),
                install_dir: PathBuf::from(string(info.install_dir)),
                platform: info.platform,
                late_load: info.late_load != 0,
            }
        };
        let rt = Runtime::new(&mut ffi_host, init);
        *PLUGIN.lock() = Some(Plugin { host: ffi_host, rt });
        LB_OK
    }))
    .unwrap_or(LB_ERR_INVALID);
    // SAFETY: the adapter passes a writable result struct.
    if let Some(result) = unsafe { result.as_mut() } {
        result.struct_size = core::mem::size_of::<LbInitResult>() as u32;
        result.abi_version = LB_ABI_VERSION;
        result.status = status;
        result.pad = 0;
        result.core_version = LbStr {
            ptr: CORE_VERSION.as_ptr(),
            len: (CORE_VERSION.len() - 1) as u32,
        };
    }
    status
}

#[unsafe(no_mangle)]
pub extern "C" fn lb_core_shutdown(reason: u32) {
    let _ = catch_unwind(|| {
        if let Some(mut lock) = PLUGIN.try_lock()
            && let Some(mut p) = lock.take()
        {
            tracing::info!("shutdown (reason {reason})");
            let (lines, _) = lb_runtime::logging::drain_console(64);
            for line in lines {
                p.host.server_print(&format!("{line}\n"));
            }
        }
    });
}

/// # Safety
/// `info` must be valid during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lb_core_map_start(info: *const LbMapInfo) {
    // SAFETY: the adapter passes a valid map info struct.
    let Some(info) = (unsafe { info.as_ref() }) else { return };
    // SAFETY: strings are valid during the call.
    let name = unsafe { string(info.map_name) };
    guard((), |p| {
        p.rt.map_start(
            &mut p.host,
            &name,
            info.max_clients,
            MapEpoch(info.map_epoch),
            info.late_load != 0,
        )
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lb_core_map_end(_epoch: u32) {
    guard((), |p| p.rt.map_end(&mut p.host));
}

/// # Safety
/// `input` and everything it points to must be valid during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lb_core_frame_pre(input: *const LbFrameInput) {
    // SAFETY: the adapter passes a valid frame input.
    let Some(input) = (unsafe { input.as_ref() }) else {
        return;
    };
    guard((), |p| {
        let start = Instant::now();
        // SAFETY: arrays and the arena are valid during the call.
        let (frame, malformed) = unsafe { lb_host::arena::decode_frame(input, &mut p.rt.strings) };
        p.rt.frame_pre(&mut p.host, frame, malformed);
        p.rt.core_times.set_pre(start.elapsed().as_nanos() as u64);
    });
}

/// # Safety
/// `input` must be valid during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lb_core_frame_post(input: *const LbFrameInput) {
    // SAFETY: the adapter passes a valid frame input.
    let mono_ns = unsafe { input.as_ref() }.map(|i| i.header.mono_ns).unwrap_or(0);
    guard((), |p| {
        let start = Instant::now();
        p.rt.frame_post(&mut p.host, mono_ns);
        p.rt.core_times.finish_frame(start.elapsed().as_nanos() as u64);
    });
}

/// # Safety
/// `args` must be valid during the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lb_core_server_command(args: *const LbArgs) {
    // SAFETY: the adapter passes valid arguments.
    let Some(args) = (unsafe { args.as_ref() }) else { return };
    let argv: Vec<String> = if args.argv.is_null() {
        Vec::new()
    } else {
        // SAFETY: `argc` strings are valid during the call.
        unsafe { core::slice::from_raw_parts(args.argv, args.argc as usize) }
            .iter()
            .map(|s| unsafe { string(*s) })
            .collect()
    };
    guard((), |p| {
        let refs: Vec<&str> = argv.iter().skip(1).map(String::as_str).collect();
        let out = commands::execute(&mut p.rt, &mut p.host, &refs);
        for line in out {
            p.host.server_print(&format!("{line}\n"));
        }
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn lb_core_fatal(message: LbStr) {
    let _ = catch_unwind(|| {
        // SAFETY: the adapter passes a string valid during the call.
        let text = unsafe { string(message) };
        tracing::error!("fatal engine error: {text}");
    });
}
