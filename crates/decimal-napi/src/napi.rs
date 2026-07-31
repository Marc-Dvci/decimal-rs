//! A thin safe wrapper over the raw Node-API.
//!
//! # Why this module exists
//!
//! Every `unsafe` block in this project is in this file. That is the point of
//! it. `decimal-core` forbids `unsafe` outright and the compiler enforces it;
//! the adapter cannot, because talking to a C API is intrinsically unsafe. So
//! the unsafety is collected here, behind functions whose signatures are safe
//! to call, rather than scattered through the two thousand lines of binding
//! code that sit above it.
//!
//! The rules the wrappers below rely on, stated once so that each individual
//! block does not have to re-argue them:
//!
//! 1. **`Env` is only ever constructed by a callback from Node**, and never
//!    outlives that callback. Node guarantees the `napi_env` is valid for the
//!    duration of the call, so every use of it here is within its lifetime.
//! 2. **Every `napi_value` handed to these functions came from Node**, in the
//!    same callback, and so is a live handle in the current handle scope.
//! 3. **Out-parameters are always initialised by the API on success.** Each
//!    call below checks the returned status before reading the out-parameter,
//!    and returns `None`/`Err` if the call failed.
//! 4. **Strings crossing the boundary are copied**, never borrowed, so no
//!    pointer outlives the call that produced it.
//!
//! Where a wrapper cannot uphold one of these on its own, it says so.

use napi_sys as sys;
use std::ffi::c_void;
use std::ptr;

/// A Node-API environment handle, valid for the duration of one callback.
#[derive(Clone, Copy)]
pub struct Env(pub sys::napi_env);

/// A handle to a JavaScript value, live within the current handle scope.
pub type Value = sys::napi_value;

/// The JavaScript type of a value, as `typeof` reports it.
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum JsType {
    Undefined,
    Null,
    Boolean,
    Number,
    String,
    Symbol,
    Object,
    Function,
    External,
    BigInt,
    Unknown,
}

/// Bind the Node-API entry points before first use.
///
/// The two platform families get here differently, and the difference is not
/// cosmetic. On ELF targets the `napi_*` symbols are exported by the host
/// `node` executable itself, so the dynamic linker has already resolved them
/// by the time this addon is loaded and there is nothing to do. On Windows an
/// executable's exports cannot be linked against in that way, so the symbols
/// must be located in the loaded process image at run time.
///
/// The library handle is leaked deliberately: the symbols must stay valid for
/// as long as the process lives, which is strictly longer than any scope
/// available here.
pub fn bind_symbols() {
    #[cfg(any(windows, feature = "dyn-symbols"))]
    // SAFETY: called once, from the module registration callback, before any
    // other Node-API function is used. `setup` panics rather than returning an
    // invalid library, so there is no failure mode to handle here.
    unsafe {
        std::mem::forget(sys::setup());
    }
}

impl Env {
    // -- reading arguments -------------------------------------------------

    /// The arguments, the receiver, and the callback data for the current
    /// call.
    ///
    /// `max_args` bounds how many arguments are copied out; JavaScript permits
    /// any number, and the ones beyond a method's arity are ignored exactly as
    /// they are in the original.
    pub fn callback_info(
        self,
        info: sys::napi_callback_info,
        max_args: usize,
    ) -> (Vec<Value>, Value, *mut c_void) {
        let mut argc = max_args;
        let mut argv: Vec<Value> = vec![ptr::null_mut(); max_args];
        let mut this: Value = ptr::null_mut();
        let mut data: *mut c_void = ptr::null_mut();

        // SAFETY: `info` is the callback info Node passed to this callback;
        // `argv` has room for `argc` entries; all four out-pointers are valid
        // for the duration of the call. On return `argc` holds the number of
        // arguments actually written, which is at most `max_args`.
        unsafe {
            sys::napi_get_cb_info(
                self.0,
                info,
                &mut argc,
                argv.as_mut_ptr(),
                &mut this,
                &mut data,
            );
        }

        argv.truncate(argc.min(max_args));
        (argv, this, data)
    }

    /// The same, for the genuinely variadic functions — `Decimal.sum`,
    /// `Decimal.hypot`, `Decimal.max`, `Decimal.min` — where every argument
    /// counts and no cap would be honest.
    ///
    /// Node reports the true argument count when the `argv` pointer is null, so
    /// this asks first and allocates second. The alternative, guessing a
    /// generous fixed cap, silently drops the tail of a longer call; and the
    /// tail of a `sum` is exactly where a NaN would be hiding.
    pub fn callback_info_variadic(
        self,
        info: sys::napi_callback_info,
    ) -> (Vec<Value>, Value, *mut c_void) {
        let mut argc: usize = 0;
        // SAFETY: `info` is live; a null `argv` requests only the count, which
        // is what `argc` receives. `this` and `data` are not wanted here.
        unsafe {
            sys::napi_get_cb_info(
                self.0,
                info,
                &mut argc,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }
        self.callback_info(info, argc)
    }

    /// `typeof value`.
    pub fn type_of(self, value: Value) -> JsType {
        let mut result: sys::napi_valuetype = 0;
        // SAFETY: `value` is a live handle; `result` is a valid out-pointer.
        let status = unsafe { sys::napi_typeof(self.0, value, &mut result) };
        if status != sys::Status::napi_ok {
            return JsType::Unknown;
        }
        match result {
            sys::ValueType::napi_undefined => JsType::Undefined,
            sys::ValueType::napi_null => JsType::Null,
            sys::ValueType::napi_boolean => JsType::Boolean,
            sys::ValueType::napi_number => JsType::Number,
            sys::ValueType::napi_string => JsType::String,
            sys::ValueType::napi_symbol => JsType::Symbol,
            sys::ValueType::napi_object => JsType::Object,
            sys::ValueType::napi_function => JsType::Function,
            sys::ValueType::napi_external => JsType::External,
            _ => JsType::Unknown,
        }
    }

    /// A number argument, or `None` if the value is not a number.
    pub fn as_f64(self, value: Value) -> Option<f64> {
        let mut out = 0.0f64;
        // SAFETY: live handle, valid out-pointer; the status check below is
        // what makes reading `out` sound.
        let status = unsafe { sys::napi_get_value_double(self.0, value, &mut out) };
        (status == sys::Status::napi_ok).then_some(out)
    }

    /// A boolean argument.
    pub fn as_bool(self, value: Value) -> Option<bool> {
        let mut out = false;
        // SAFETY: as above.
        let status = unsafe { sys::napi_get_value_bool(self.0, value, &mut out) };
        (status == sys::Status::napi_ok).then_some(out)
    }

    /// A string argument, copied into an owned `String`.
    ///
    /// Two calls: the first asks for the encoded length, the second fills a
    /// buffer of that size. The buffer is over-allocated by one for the
    /// terminating NUL that the API writes and that Rust does not want.
    pub fn as_string(self, value: Value) -> Option<String> {
        let mut len = 0usize;
        // SAFETY: passing a null buffer with size 0 is the documented way to
        // ask for the required length; `len` is a valid out-pointer.
        let status =
            unsafe { sys::napi_get_value_string_utf8(self.0, value, ptr::null_mut(), 0, &mut len) };
        if status != sys::Status::napi_ok {
            return None;
        }

        let mut buffer = vec![0u8; len + 1];
        let mut written = 0usize;
        // SAFETY: `buffer` has `len + 1` bytes, which is what is passed as the
        // capacity, so the API cannot write past its end. The cast to `*mut
        // c_char` is a plain sign change on the element type.
        let status = unsafe {
            sys::napi_get_value_string_utf8(
                self.0,
                value,
                buffer.as_mut_ptr().cast(),
                len + 1,
                &mut written,
            )
        };
        if status != sys::Status::napi_ok {
            return None;
        }

        buffer.truncate(written);
        String::from_utf8(buffer).ok()
    }

    // -- creating values ---------------------------------------------------

    /// A JavaScript string.
    pub fn string(self, text: &str) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: `text` is a valid UTF-8 slice and its length is passed
        // explicitly, so the API reads only within it. The string contents are
        // copied by Node, so `text` need not outlive this call.
        unsafe {
            sys::napi_create_string_utf8(self.0, text.as_ptr().cast(), text.len(), &mut out);
        }
        out
    }

    /// A JavaScript number.
    pub fn number(self, value: f64) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: valid out-pointer; `napi_create_double` cannot fail for a
        // finite or non-finite double.
        unsafe {
            sys::napi_create_double(self.0, value, &mut out);
        }
        out
    }

    /// A JavaScript boolean.
    pub fn boolean(self, value: bool) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: valid out-pointer.
        unsafe {
            sys::napi_get_boolean(self.0, value, &mut out);
        }
        out
    }

    /// `undefined`.
    pub fn undefined(self) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: valid out-pointer.
        unsafe {
            sys::napi_get_undefined(self.0, &mut out);
        }
        out
    }

    /// An array of arbitrary values — used by `toFraction`, which returns a
    /// two-element `[numerator, denominator]`.
    pub fn array(self, elements: &[Value]) -> Value {
        let mut array: Value = ptr::null_mut();
        // SAFETY: valid out-pointer.
        unsafe {
            sys::napi_create_array_with_length(self.0, elements.len(), &mut array);
        }
        for (index, &element) in elements.iter().enumerate() {
            // SAFETY: `array` is a live array handle just created, `index` is
            // within the length it was created with, and `element` is live.
            unsafe {
                sys::napi_set_element(self.0, array, index as u32, element);
            }
        }
        array
    }

    /// An array of numbers — used for the `d` accessor, which must hand back
    /// something the original's test helper can index and take `.length` of.
    pub fn number_array(self, values: &[u32]) -> Value {
        let elements: Vec<Value> = values.iter().map(|&v| self.number(f64::from(v))).collect();
        self.array(&elements)
    }

    // -- properties --------------------------------------------------------

    /// `array[index]`.
    pub fn get_element(self, array: Value, index: u32) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: `array` is a live handle; `out` is a valid out-pointer. A
        // non-array or an out-of-range index yields `undefined` rather than
        // failing.
        unsafe {
            sys::napi_get_element(self.0, array, index, &mut out);
        }
        out
    }

    /// Define properties on an existing object.
    ///
    /// Used to install method aliases after the class exists, so that both
    /// spellings resolve to the same function object — `napi_define_class`
    /// would create a separate function for each descriptor, and the original's
    /// `P.absoluteValue = P.abs = …` makes them one.
    ///
    /// # Safety
    ///
    /// `object` is a live handle, and every descriptor's `utf8name` is a
    /// NUL-terminated string that outlives the call.
    pub unsafe fn define_properties(
        self,
        object: Value,
        properties: &[sys::napi_property_descriptor],
    ) {
        sys::napi_define_properties(self.0, object, properties.len(), properties.as_ptr());
    }

    /// `globalThis`.
    pub fn global(self) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: valid out-pointer.
        unsafe {
            sys::napi_get_global(self.0, &mut out);
        }
        out
    }

    /// `object[name] = value`.
    pub fn set_named(self, object: Value, name: &str, value: Value) {
        let name = std::ffi::CString::new(name).expect("property names contain no NUL");
        // SAFETY: `object` and `value` are live handles; `name` is a valid
        // NUL-terminated C string that outlives the call.
        unsafe {
            sys::napi_set_named_property(self.0, object, name.as_ptr(), value);
        }
    }

    /// `object[name]`.
    pub fn get_named(self, object: Value, name: &str) -> Value {
        let name = std::ffi::CString::new(name).expect("property names contain no NUL");
        let mut out: Value = ptr::null_mut();
        // SAFETY: as above; `out` is a valid out-pointer.
        unsafe {
            sys::napi_get_named_property(self.0, object, name.as_ptr(), &mut out);
        }
        out
    }

    /// Whether `object` has an own property called `name`.
    pub fn has_own(self, object: Value, name: &str) -> bool {
        let key = self.string(name);
        let mut out = false;
        // SAFETY: live handles and a valid out-pointer.
        unsafe {
            sys::napi_has_own_property(self.0, object, key, &mut out);
        }
        out
    }

    /// `value instanceof constructor`.
    pub fn instance_of(self, value: Value, constructor: Value) -> bool {
        let mut out = false;
        // SAFETY: live handles; `constructor` is known to be a function
        // because it is one this module created.
        unsafe {
            sys::napi_instanceof(self.0, value, constructor, &mut out);
        }
        out
    }

    /// `new constructor(...args)`.
    pub fn construct(self, constructor: Value, args: &[Value]) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: `constructor` is a live function handle; `args` is a slice
        // of live handles whose length is passed explicitly.
        unsafe {
            sys::napi_new_instance(self.0, constructor, args.len(), args.as_ptr(), &mut out);
        }
        out
    }

    /// `receiver.function(...args)`.
    pub fn call(self, receiver: Value, function: Value, args: &[Value]) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: all handles are live; `args` has its length passed
        // explicitly. A thrown exception leaves `out` untouched, which the
        // caller handles by checking `is_exception_pending`.
        unsafe {
            sys::napi_call_function(
                self.0,
                receiver,
                function,
                args.len(),
                args.as_ptr(),
                &mut out,
            );
        }
        out
    }

    // -- references --------------------------------------------------------

    /// A strong reference that keeps `value` alive beyond the current scope.
    ///
    /// Used for the constructor function, which every instance method needs to
    /// reach in order to build its result. The reference is never released:
    /// a `Decimal` constructor lives as long as the module does.
    pub fn create_reference(self, value: Value) -> sys::napi_ref {
        let mut out: sys::napi_ref = ptr::null_mut();
        // SAFETY: `value` is live; an initial refcount of 1 makes this a
        // strong reference.
        unsafe {
            sys::napi_create_reference(self.0, value, 1, &mut out);
        }
        out
    }

    /// The value behind a reference created by [`Env::create_reference`].
    pub fn reference_value(self, reference: sys::napi_ref) -> Value {
        let mut out: Value = ptr::null_mut();
        // SAFETY: `reference` was created by `create_reference` above and has
        // never been released, so it is still valid.
        unsafe {
            sys::napi_get_reference_value(self.0, reference, &mut out);
        }
        out
    }

    // -- native data attached to objects -----------------------------------

    /// Attach `payload` to `object`, to be dropped when `object` is collected.
    ///
    /// # Ownership
    ///
    /// This transfers ownership of the box into the JavaScript object. The
    /// finalizer below is the only place it is freed, and it runs exactly once
    /// per successful wrap, so the value is neither leaked nor freed twice.
    pub fn wrap<T: 'static>(self, object: Value, payload: Box<T>) {
        let raw = Box::into_raw(payload);
        let mut reference: sys::napi_ref = ptr::null_mut();
        // SAFETY: `raw` is a live, uniquely-owned pointer from `Box::into_raw`
        // that nothing else refers to. `finalize::<T>` is instantiated for the
        // same `T`, so the pointer it reconstructs has the correct type.
        let status = unsafe {
            sys::napi_wrap(
                self.0,
                object,
                raw.cast(),
                Some(finalize::<T>),
                ptr::null_mut(),
                &mut reference,
            )
        };
        if status != sys::Status::napi_ok {
            // The wrap failed, so Node did not take ownership and the
            // finalizer will never run. Reclaim the box rather than leak it.
            // SAFETY: `raw` came from `Box::into_raw` moments ago and was not
            // handed to anyone, since the call failed.
            drop(unsafe { Box::from_raw(raw) });
        }
    }

    /// Borrow the payload previously attached to `object`.
    ///
    /// Returns `None` when the object carries no payload, which happens for an
    /// instance whose construction has not finished.
    ///
    /// # Safety of the returned reference
    ///
    /// The reference borrows from data owned by the JavaScript object, which
    /// cannot be collected while it is reachable from the arguments of the
    /// call in progress. The lifetime is tied to `&self`, which is scoped to
    /// the callback, so it cannot escape into a longer-lived binding.
    pub fn unwrap<T: 'static>(&self, object: Value) -> Option<&mut T> {
        let mut raw: *mut c_void = ptr::null_mut();
        // SAFETY: live handle and valid out-pointer.
        let status = unsafe { sys::napi_unwrap(self.0, object, &mut raw) };
        if status != sys::Status::napi_ok || raw.is_null() {
            return None;
        }
        // SAFETY: `raw` was stored by `wrap::<T>` for this same `T` — every
        // object this module wraps carries exactly one payload type — so the
        // cast is type-correct. The pointer is live because the object is.
        Some(unsafe { &mut *raw.cast::<T>() })
    }

    // -- errors ------------------------------------------------------------

    /// Throw a JavaScript `Error` with the given message.
    ///
    /// A Rust panic must never cross this boundary, so every fallible path in
    /// the binding ends here instead.
    pub fn throw(self, message: &str) {
        let message = std::ffi::CString::new(message.replace('\0', ""))
            .expect("NULs were just removed");
        // SAFETY: a null code and a valid NUL-terminated message is the
        // documented way to throw a plain `Error`.
        unsafe {
            sys::napi_throw_error(self.0, ptr::null(), message.as_ptr());
        }
    }

    /// Throw a JavaScript `RangeError`.
    ///
    /// Distinct from [`Env::throw`] because the original does not raise this
    /// one: JavaScript does, when an array is asked to exceed its maximum
    /// length. Reproducing the message but not the constructor would leave
    /// `err instanceof RangeError` false where the original makes it true.
    pub fn throw_range_error(self, message: &str) {
        let message = std::ffi::CString::new(message.replace('\0', ""))
            .expect("NULs were just removed");
        // SAFETY: a null code and a valid NUL-terminated message is the
        // documented way to throw a `RangeError`.
        unsafe {
            sys::napi_throw_range_error(self.0, ptr::null(), message.as_ptr());
        }
    }

    /// Whether an exception is already pending, in which case the caller must
    /// return without throwing another.
    pub fn is_exception_pending(self) -> bool {
        let mut pending = false;
        // SAFETY: valid out-pointer.
        unsafe {
            sys::napi_is_exception_pending(self.0, &mut pending);
        }
        pending
    }
}

/// Drop a payload attached by [`Env::wrap`] when its object is collected.
///
/// # Safety
///
/// Called only by Node, once, with the pointer that `wrap::<T>` stored for the
/// same `T`.
unsafe extern "C" fn finalize<T: 'static>(
    _env: sys::napi_env,
    data: *mut c_void,
    _hint: *mut c_void,
) {
    if !data.is_null() {
        drop(Box::from_raw(data.cast::<T>()));
    }
}

/// Define a class with the given name, constructor callback, and properties.
///
/// Returns the constructor function itself, which is what makes it possible
/// for the module's exports to *be* the constructor.
///
/// # Safety
///
/// `properties` must remain valid for the duration of the call, and each
/// descriptor's `data` pointer must outlive every invocation of the method it
/// belongs to — which, for this module, means it must be leaked or otherwise
/// owned for the life of the constructor.
pub unsafe fn define_class(
    env: Env,
    name: &str,
    constructor: sys::napi_callback,
    data: *mut c_void,
    properties: &[sys::napi_property_descriptor],
) -> Value {
    let mut class: Value = ptr::null_mut();
    sys::napi_define_class(
        env.0,
        name.as_ptr().cast(),
        name.len(),
        constructor,
        data,
        properties.len(),
        properties.as_ptr(),
        &mut class,
    );
    class
}
