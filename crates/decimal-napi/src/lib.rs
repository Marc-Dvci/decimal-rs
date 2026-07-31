//! Scaffold probe: prove that a hand-written module registration entry point
//! can return a *constructor function* as `module.exports`, so that the
//! original `test/setup.js` line `Decimal = require('../decimal')` loads this
//! binary with no JavaScript shim in between.
//!
//! Everything here is placeholder except the registration mechanism, which is
//! the one piece of genuine unknown risk in the plan and is therefore proven
//! before any porting work starts.

use napi_sys as sys;
use std::ptr;

unsafe extern "C" fn decimal_ctor(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let mut this: sys::napi_value = ptr::null_mut();
    let mut argc: usize = 0;
    sys::napi_get_cb_info(
        env,
        info,
        &mut argc,
        ptr::null_mut(),
        &mut this,
        ptr::null_mut(),
    );
    this
}

/// Node calls this on `require()`. Its **return value** becomes
/// `module.exports` when it differs from the `exports` object passed in, which
/// is what lets the module itself be a constructor.
#[no_mangle]
pub unsafe extern "C" fn napi_register_module_v1(
    env: sys::napi_env,
    _exports: sys::napi_value,
) -> sys::napi_value {
    // napi-sys resolves the Node-API symbols out of the host process at load
    // time rather than link time, so they must be bound before first use.
    // Leaked deliberately: the symbols stay valid for the life of the process.
    std::mem::forget(sys::setup());

    let mut ctor: sys::napi_value = ptr::null_mut();
    sys::napi_define_class(
        env,
        c"Decimal".as_ptr(),
        7,
        Some(decimal_ctor),
        ptr::null_mut(),
        0,
        ptr::null(),
        &mut ctor,
    );

    let mut round_up: sys::napi_value = ptr::null_mut();
    sys::napi_create_int32(env, 0, &mut round_up);
    sys::napi_set_named_property(env, ctor, c"ROUND_UP".as_ptr(), round_up);

    ctor
}
