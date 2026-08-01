//! The Node binding: what makes `require('../decimal')` return a Rust
//! implementation.
//!
//! # The one structural idea
//!
//! `test/setup.js` — which is hash-pinned and must not change — reaches the
//! implementation through a single line:
//!
//! ```js
//! Decimal = require('../decimal');
//! ```
//!
//! Node resolves that by trying `../decimal.js`, then `../decimal.json`, then
//! `../decimal.node`. The first is the file being replaced and is absent, so
//! the compiled artifact of this crate is what the tests receive. For that to
//! work, the *module itself* has to be the `Decimal` constructor: the tests
//! call `new Decimal(x)`, read statics off it, and check `x.constructor ===
//! Decimal`.
//!
//! Node uses the return value of `napi_register_module_v1` as `module.exports`
//! whenever it differs from the `exports` object passed in. So returning the
//! class from that function makes the module the constructor. The usual derive
//! macros cannot express this, which is why the entry point below is written
//! against the raw API.
//!
//! # Per-constructor state
//!
//! `Decimal.clone()` produces an *independent* constructor with its own
//! configuration, and the original's `clone` and `config` modules check that
//! two clones do not interfere. Each constructor therefore owns a
//! [`ConstructorState`], owned and finalized by that JavaScript function.
//! Every instance has its constructor as an own property, so methods on the
//! one shared prototype recover the correct clone and then borrow its state
//! only for a non-reentrant Rust operation. This mirrors the original's object
//! model and avoids both persistent-reference leaks and forged Rust lifetimes.
//!
//! # Errors, and the one thing that must not happen
//!
//! Every fallible path ends in a thrown JavaScript `Error` carrying the
//! original's exact message text. Behind that, a Rust panic must never cross
//! this boundary: an `extern "C"` function that lets one escape does not
//! return an error, it aborts the process, and a library that can take Node
//! down is not a drop-in replacement for one that cannot.
//!
//! So there is exactly one `extern "C"` function per callback and it is not
//! the callback — it is the wrapper [`guarded!`] builds, whose whole body is a
//! [`catching`] around a plain Rust `fn`. The callbacks themselves are
//! ordinary `unsafe fn`s, which is what lets an unwind reach the
//! `catch_unwind` at all: a panic escaping an inner `extern "C"` frame would
//! abort there, before any handler further out could see it.
//!
//! That is not a theoretical distinction. The first version of this guard
//! wrapped the registration table while the callbacks were still
//! `extern "C"`, read exactly as it does now, and aborted the process on a
//! deliberately injected panic — the negative control is in DECISIONS.md D-22,
//! along with what it printed before and after.

mod napi;

use decimal_core::arith::{self, compare};
use decimal_core::random::Xoshiro256StarStar;
use decimal_core::{
    format, fraction, inverse, ops, parse, roots, Config, Ctx, Decimal, Error, Sign,
};
use napi::{bind_symbols, define_class, Env, JsType, Value, WeakReferenceOwner};
use napi_sys as sys;
use std::cell::RefCell;
use std::ffi::c_void;
use std::ptr;

// ---------------------------------------------------------------------------
// Per-constructor state
// ---------------------------------------------------------------------------

/// The mutable state of one `Decimal` constructor.
struct ConstructorState {
    ctx: Ctx,
    /// The non-cryptographic generator behind `random`, standing where the
    /// original has `Math.random()`.
    ///
    /// Per constructor rather than per module, and seeded independently, so
    /// that a clone draws its own stream. Sharing one would make two
    /// constructors' draws interleave — harmless, but it would mean `clone`
    /// did not produce quite the independent object it advertises.
    entropy: Xoshiro256StarStar,
}

/// Native data owned by a constructor function.
///
/// `state` is runtime-borrowed because JavaScript is re-entrant. More
/// importantly, every callback releases the borrow before it touches a
/// property, coerces a value, constructs an object, or calls a function.
/// `constructor` is weak: it supports `Decimal(x)` without preventing a cloned
/// constructor from being collected.
struct ConstructorData {
    state: RefCell<ConstructorState>,
    constructor: sys::napi_ref,
}

impl WeakReferenceOwner for ConstructorData {
    fn set_weak_reference(&mut self, reference: sys::napi_ref) {
        self.constructor = reference;
    }

    fn weak_reference(&self) -> sys::napi_ref {
        self.constructor
    }
}

/// Use constructor callback data without manufacturing a reference lifetime.
///
/// # Safety
///
/// `data` must be the `ConstructorData` pointer installed on this constructor
/// by `build_class`. The closure cannot return a borrowed reference.
unsafe fn with_callback_data<R>(
    data: *mut c_void,
    body: impl for<'a> FnOnce(&'a ConstructorData) -> R,
) -> R {
    body(&*data.cast::<ConstructorData>())
}

fn with_state<R>(
    env: Env,
    class: Value,
    body: impl for<'a> FnOnce(&'a ConstructorState) -> R,
) -> Option<R> {
    env.with_wrapped::<ConstructorData, _>(class, |data| {
        let state = data.state.borrow();
        body(&state)
    })
}

fn with_state_mut<R>(
    env: Env,
    class: Value,
    body: impl for<'a> FnOnce(&'a mut ConstructorState) -> R,
) -> Option<R> {
    env.with_wrapped::<ConstructorData, _>(class, |data| {
        let mut state = data.state.borrow_mut();
        body(&mut state)
    })
}

/// Copy the calculation context, run pure Rust, and commit the scratch state.
/// No Node-API call occurs while the constructor state is borrowed.
fn calculate<R>(env: Env, class: Value, body: impl FnOnce(&mut Ctx) -> R) -> Option<(R, bool)> {
    let mut ctx = with_state(env, class, |state| state.ctx)?;
    let result = body(&mut ctx);
    let exceeded = ctx.take_array_limit_exceeded();
    with_state_mut(env, class, |state| state.ctx = ctx)?;
    Some((result, exceeded))
}

fn class_from_instance(env: Env, instance: Value) -> Option<Value> {
    let class = env.get_named(instance, "constructor");
    env.with_wrapped::<ConstructorData, _>(class, |_| ())
        .map(|()| class)
}

// ---------------------------------------------------------------------------
// Converting JavaScript values into decimals
// ---------------------------------------------------------------------------

/// The constructor's dispatch on the type of its argument.
///
/// JavaScript conversion is completed before the constructor state is
/// borrowed. A user-defined coercion hook may therefore re-enter this addon
/// without aliasing the outer calculation's state.
fn coerce(env: Env, class: Value, value: Value) -> Result<Decimal, Error> {
    // An existing Decimal — of this constructor or any other clone — is
    // re-judged against the current exponent limits rather than copied. This
    // is the `new Ctor(y)` that opens every binary method upstream, and it is
    // why a wide-built operand passed to a narrowly configured one arrives
    // already clamped; `ops::clamped_copy` is the same rule the core applies
    // to receivers, and it lives in one place so the two cannot drift.
    if let Some(existing) = decimal_of(env, value) {
        return with_state(env, class, |state| ops::clamped_copy(&state.ctx, &existing))
            .ok_or_else(|| Error::InvalidArgument(describe(env, value)));
    }

    match env.type_of(value) {
        JsType::Number => {
            let n = env.as_f64(value).unwrap_or(f64::NAN);
            with_state(env, class, |state| from_f64(&state.ctx, n))
                .ok_or_else(|| Error::InvalidArgument(describe(env, value)))
        }
        JsType::String => {
            let text = env.as_string(value).unwrap_or_default();
            calculate(env, class, |ctx| parse::from_str(ctx, &text))
                .map(|(result, _)| result)
                .unwrap_or_else(|| Err(Error::InvalidArgument(text)))
        }
        // The fourth accepted type, and the one easiest to forget: the
        // original's constructor has a `t === 'bigint'` branch, and its
        // documentation comments say `{number|string|bigint|Decimal}` on
        // nineteen methods. It takes the sign off and then hands the digits to
        // `parseDecimal` — not to `parseOther` — so a BigInt goes down the
        // plain decimal path and never sees the radix or separator rules.
        //
        // Nothing in the original suite constructs one, and the differential
        // harness generates strings and numbers, so this was missing until the
        // dead-code warning on the unused `JsType::BigInt` variant asked why
        // the variant existed.
        JsType::BigInt => match env.to_string_value(value) {
            Some(text) => {
                let (sign, digits) = match text.strip_prefix('-') {
                    Some(rest) => (Sign::Neg, rest),
                    None => (Sign::Pos, text.as_str()),
                };
                with_state(env, class, |state| {
                    parse::parse_decimal(&state.ctx, sign, digits)
                })
                .ok_or(Error::InvalidArgument(text))
            }
            None => Err(Error::InvalidArgument(describe(env, value))),
        },
        _ => Err(Error::InvalidArgument(describe(env, value))),
    }
}

/// A value from an IEEE double, following the original's constructor.
fn from_f64(ctx: &Ctx, v: f64) -> Decimal {
    if v == 0.0 {
        // `1 / v < 0` distinguishes -0 from +0, which `v == 0.0` does not.
        return Decimal::zero(if v.is_sign_negative() {
            Sign::Neg
        } else {
            Sign::Pos
        });
    }
    if v.is_nan() {
        return Decimal::nan();
    }
    let sign = if v < 0.0 { Sign::Neg } else { Sign::Pos };
    if v.is_infinite() {
        return Decimal::infinity(sign);
    }
    // The original goes through `v.toString()` for everything that is not a
    // small integer, so the ECMAScript number-to-string rules apply here too.
    parse::parse_decimal(ctx, sign, &format::number_to_string(v.abs()))
}

/// How JavaScript would render `value` when it is interpolated into an error
/// message.
fn describe(env: Env, value: Value) -> String {
    match env.type_of(value) {
        JsType::Undefined => "undefined".to_string(),
        JsType::Null => "null".to_string(),
        JsType::Boolean => env.as_bool(value).unwrap_or(false).to_string(),
        JsType::Number => format::number_to_string(env.as_f64(value).unwrap_or(f64::NAN)),
        JsType::String => env.as_string(value).unwrap_or_default(),
        // Everything else goes through JavaScript's own conversion, because
        // that is what the original does: `Error(invalidArgument + v)` is a
        // string concatenation, and the result is whatever `v` stringifies to.
        //
        // Guessing at it produced two wrong messages. An empty array gives the
        // empty string, not `[object Object]` — `'' + []` is `''`, because
        // `Array.prototype.toString` joins. And a Symbol does not stringify at
        // all: the concatenation raises `TypeError: Cannot convert a Symbol
        // value to a string`, so `new Decimal(Symbol())` fails with a TypeError
        // and never reaches the library's own error. Here that falls out
        // correctly — the coercion leaves V8's exception pending, and `fail`
        // declines to throw over one that is already there.
        _ => env.to_string_value(value).unwrap_or_default(),
    }
}

/// The decimal wrapped inside a JavaScript object, if it is one.
///
/// Returns an owned copy rather than a borrow. The borrow would be sound —
/// the object cannot be collected while it is an argument to the call in
/// progress — but expressing that to the borrow checker means attaching a
/// lifetime to the `Env`, which would infect every signature in this file for
/// the sake of avoiding a copy of a short digit vector.
fn decimal_of(env: Env, value: Value) -> Option<Decimal> {
    if env.type_of(value) != JsType::Object {
        return None;
    }
    env.clone_wrapped::<Decimal>(value)
}

/// Build a new JavaScript `Decimal` belonging to `class`.
///
/// The instance is created by actually invoking the constructor, so that it
/// gets the right prototype, the right `constructor` property, and satisfies
/// `instanceof` — all of which the original's tests check. It is invoked with
/// `0` rather than with no argument, because `new Decimal()` is required to
/// throw; the placeholder payload is then overwritten with the real value.
///
/// Constructing with a placeholder is cheap — `from_f64` short-circuits zero
/// without parsing — and it avoids the alternative, which was a flag on the
/// constructor state saying "this call is internal". That flag was wrong in a
/// way worth recording: it made correctness depend on two callbacks sharing
/// one mutable cell, and when they did not, every method failed with the
/// constructor's own "Invalid argument: undefined".
fn make(env: Env, class: Value, value: Decimal) -> Value {
    let placeholder = env.number(0.0);
    let object = env.construct(class, &[placeholder]);
    env.replace_wrapped::<Decimal>(object, value);
    object
}

/// Turn an abandoned calculation into the error the original raises, if one
/// happened; `None` if the result is a real one.
///
/// A routine that asked for a digit array larger than the original's host will
/// build sets a flag and returns a placeholder (`decimal_core::arith`'s
/// `abandoned`). The placeholder means nothing, so **every** path that returns a
/// result to JavaScript has to consume the flag before it hands anything back —
/// not only the one that builds a Decimal.
///
/// That distinction is not hypothetical. This check lived inside [`make`] alone,
/// and `toBinary` returns a *string*: at precision 939,524,081 the port answered
/// `0b1` — the placeholder, rendered — where the original raises `RangeError:
/// Invalid array length`. Found by the sweep in `scripts/host-limits.js`, and
/// only after that script stopped fingerprinting results in a way that could not
/// tell a string from a Decimal.
///
/// The message is JavaScript's own, thrown with `napi_throw_range_error` so that
/// `err instanceof RangeError` is true on both implementations. See
/// `Ctx::array_limit_exceeded` and DECISIONS.md D-10, D-19.
fn abandoned(env: Env, exceeded: bool) -> Option<Value> {
    if exceeded {
        env.throw_range_error("Invalid array length");
        return Some(env.undefined());
    }
    None
}

/// Throw `error` and return `undefined`, the shape every failing callback
/// takes.
fn fail(env: Env, error: Error) -> Value {
    if !env.is_exception_pending() {
        env.throw(&error.to_string());
    }
    env.undefined()
}

// ---------------------------------------------------------------------------
// The panic backstop
// ---------------------------------------------------------------------------

/// Run a callback, converting a panic into a thrown JavaScript error.
///
/// An `extern "C"` function that lets a Rust panic escape does not return an
/// error — it **aborts the process**. In a Node addon that means the panicking
/// expression takes the host down with it: no stack, no `catch`, no exit code
/// anyone can act on. `Cargo.toml` sets `panic = "unwind"` for exactly this
/// reason, and this is the other half of that decision.
///
/// Nothing here is expected to fire. Every fallible path in `decimal-core`
/// returns a `Result`, and the handful of `expect`s it does contain assert
/// invariants that `finalise` restores on every exit. A panic reaching this
/// point is therefore a *bug in the port*, and the message says so rather than
/// wearing the library's own `[DecimalError]` prefix — a caller must not be
/// able to mistake one for the other, or a `try`/`catch` written for the
/// library's errors would swallow the evidence.
///
/// `AssertUnwindSafe` is the honest annotation and not a workaround: the
/// closure borrows the constructor's state mutably, so a panic part-way
/// through a configuration change could leave that state inconsistent. The
/// trade is deliberate — an inconsistent `precision` is recoverable and
/// observable, an aborted process is neither.
fn catching(env: Env, body: impl FnOnce() -> sys::napi_value) -> sys::napi_value {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            let what = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("a panic with no message");
            if !env.is_exception_pending() {
                env.throw(&format!("decimal-rs internal error: {what}"));
            }
            env.undefined()
        }
    }
}

/// Wrap a callback in [`catching`], producing a function pointer of the same
/// shape.
///
/// Applied at the registration tables rather than inside each callback, so
/// that the guarantee is visible in one place and cannot be forgotten by a
/// method added later: an entry that is not wrapped does not read like the
/// sixty that are.
macro_rules! guarded {
    ($callback:path) => {{
        unsafe extern "C" fn wrapper(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            // SAFETY: `env` and `info` are the arguments Node just passed to
            // this callback, forwarded unchanged to the callback it wraps.
            catching(Env(env), || unsafe { $callback(env, info) })
        }
        wrapper as unsafe extern "C" fn(sys::napi_env, sys::napi_callback_info) -> sys::napi_value
    }};
}

// ---------------------------------------------------------------------------
// The constructor
// ---------------------------------------------------------------------------

unsafe fn construct_decimal(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, this, data) = env.callback_info(info, 1);
    let argument = args.first().copied().unwrap_or_else(|| env.undefined());

    // The callback data is used only to resolve its weak JavaScript owner. It
    // is never exposed as mutable state, and the reference is live because the
    // function is executing now.
    let class = unsafe { with_callback_data(data, |owner| env.reference_value(owner.constructor)) };
    let Some(class) = class else {
        return env.undefined();
    };

    if env.new_target(info).is_none() {
        return env.construct(class, &[argument]);
    }

    match coerce(env, class, argument) {
        Ok(value) => {
            env.wrap(this, Box::new(value));
            // Upstream assigns this as an own property. It is what lets one
            // shared prototype dispatch to the state of the actual clone.
            env.set_named(this, "constructor", class);
            this
        }
        Err(error) => fail(env, error),
    }
}

// ---------------------------------------------------------------------------
// Callback plumbing
// ---------------------------------------------------------------------------

/// The receiver of an instance method, plus its constructor state.
fn receiver(
    env: Env,
    info: sys::napi_callback_info,
    max_args: usize,
) -> Option<(Vec<Value>, Decimal, Value)> {
    let (args, this, _) = env.callback_info(info, max_args);
    let value = decimal_of(env, this)?;
    let class = class_from_instance(env, this)?;
    Some((args, value, class))
}

/// An argument coerced to a decimal, or a thrown error.
fn argument(env: Env, class: Value, args: &[Value], index: usize) -> Result<Decimal, Error> {
    let value = args.get(index).copied().unwrap_or_else(|| env.undefined());
    coerce(env, class, value)
}

/// An optional numeric argument: `None` when absent or `undefined`.
fn optional_number(env: Env, args: &[Value], index: usize) -> Option<f64> {
    match args.get(index).copied() {
        None => None,
        Some(v) if env.type_of(v) == JsType::Undefined => None,
        Some(v) => Some(env.as_f64(v).unwrap_or(f64::NAN)),
    }
}

/// Declares an instance method of shape `(&mut Ctx, &Decimal) -> Decimal`.
macro_rules! unary {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let Some((_, x, class)) = receiver(env, info, 0) else {
                return env.undefined();
            };
            let f: fn(&mut Ctx, &Decimal) -> Decimal = $body;
            let Some((result, exceeded)) = calculate(env, class, |ctx| f(ctx, &x)) else {
                return env.undefined();
            };
            if let Some(thrown) = abandoned(env, exceeded) {
                return thrown;
            }
            make(env, class, result)
        }
    };
}

/// Declares an instance method of shape `(&mut Ctx, &Decimal, &Decimal) -> Decimal`.
macro_rules! binary {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, class)) = receiver(env, info, 1) else {
                return env.undefined();
            };
            let y = match argument(env, class, &args, 0) {
                Ok(y) => y,
                Err(e) => return fail(env, e),
            };
            let f: fn(&mut Ctx, &Decimal, &Decimal) -> Decimal = $body;
            let Some((result, exceeded)) = calculate(env, class, |ctx| f(ctx, &x, &y)) else {
                return env.undefined();
            };
            if let Some(thrown) = abandoned(env, exceeded) {
                return thrown;
            }
            make(env, class, result)
        }
    };
}

/// Declares an instance method returning a boolean from the value alone.
macro_rules! predicate {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let Some((_, x, _)) = receiver(env, info, 0) else {
                return env.undefined();
            };
            let f: fn(&Decimal) -> bool = $body;
            env.boolean(f(&x))
        }
    };
}

/// Declares an instance method comparing against one argument.
macro_rules! comparison {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, class)) = receiver(env, info, 1) else {
                return env.undefined();
            };
            let y = match argument(env, class, &args, 0) {
                Ok(y) => y,
                Err(e) => return fail(env, e),
            };
            let f: fn(Option<core::cmp::Ordering>) -> bool = $body;
            env.boolean(f(compare(&x, &y)))
        }
    };
}

/// Declares an instance method returning a string built from the value and up
/// to two optional numeric arguments.
macro_rules! stringify {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, class)) = receiver(env, info, 2) else {
                return env.undefined();
            };
            let a = optional_number(env, &args, 0);
            let b = optional_number(env, &args, 1);
            let f: fn(&mut Ctx, &Decimal, Option<f64>, Option<f64>) -> Result<String, Error> =
                $body;
            let Some((outcome, exceeded)) = calculate(env, class, |ctx| f(ctx, &x, a, b)) else {
                return env.undefined();
            };
            // Before the string, not after: a rendering that reached the host's
            // array ceiling is holding a placeholder, and `toBinary` at
            // precision 939,524,081 rendered it as `0b1`. See `abandoned`.
            if let Some(thrown) = abandoned(env, exceeded) {
                return thrown;
            }
            match outcome {
                Ok(text) => env.string(&text),
                Err(e) => fail(env, e),
            }
        }
    };
}

/// Declares an instance method returning a rounded decimal from up to two
/// optional numeric arguments.
macro_rules! rounder {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, class)) = receiver(env, info, 2) else {
                return env.undefined();
            };
            let a = optional_number(env, &args, 0);
            let b = optional_number(env, &args, 1);
            let f: fn(&mut Ctx, &Decimal, Option<f64>, Option<f64>) -> Result<Decimal, Error> =
                $body;
            let Some((outcome, exceeded)) = calculate(env, class, |ctx| f(ctx, &x, a, b)) else {
                return env.undefined();
            };
            if let Some(thrown) = abandoned(env, exceeded) {
                return thrown;
            }
            match outcome {
                Ok(value) => make(env, class, value),
                Err(e) => fail(env, e),
            }
        }
    };
}

// -- the methods themselves -------------------------------------------------

unary!(m_abs, |ctx, x| ops::abs(ctx, x));
unary!(m_neg, |ctx, x| ops::neg(ctx, x));
unary!(m_ceil, |ctx, x| ops::ceil(ctx, x));
unary!(m_floor, |ctx, x| ops::floor(ctx, x));
unary!(m_round, |ctx, x| ops::round(ctx, x));
unary!(m_trunc, |ctx, x| ops::trunc(ctx, x));
unary!(m_sqrt, |ctx, x| decimal_core::roots::sqrt(ctx, x));
unary!(m_cbrt, |ctx, x| decimal_core::roots::cbrt(ctx, x));
unary!(m_exp, |ctx, x| decimal_core::elementary::exp(ctx, x));
unary!(m_sinh, |ctx, x| decimal_core::trig::sinh(ctx, x));
unary!(m_cosh, |ctx, x| decimal_core::trig::cosh(ctx, x));
unary!(m_tanh, |ctx, x| decimal_core::trig::tanh(ctx, x));

/// Declares a unary instance method that can raise `[DecimalError] Precision
/// limit exceeded`, which the circular functions do when the configured
/// precision outruns the 1025-digit `PI` constant.
macro_rules! fallible_unary {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let Some((_, x, class)) = receiver(env, info, 0) else {
                return env.undefined();
            };
            let f: fn(&mut Ctx, &Decimal) -> Result<Decimal, Error> = $body;
            let Some((outcome, exceeded)) = calculate(env, class, |ctx| f(ctx, &x)) else {
                return env.undefined();
            };
            if let Some(thrown) = abandoned(env, exceeded) {
                return thrown;
            }
            match outcome {
                Ok(value) => make(env, class, value),
                Err(e) => fail(env, e),
            }
        }
    };
}

fallible_unary!(m_sin, |ctx, x| decimal_core::trig::sin(ctx, x));
fallible_unary!(m_cos, |ctx, x| decimal_core::trig::cos(ctx, x));
fallible_unary!(m_tan, |ctx, x| decimal_core::trig::tan(ctx, x));
fallible_unary!(m_asin, |ctx, x| decimal_core::inverse::asin(ctx, x));
fallible_unary!(m_acos, |ctx, x| decimal_core::inverse::acos(ctx, x));
fallible_unary!(m_atan, |ctx, x| decimal_core::inverse::atan(ctx, x));
fallible_unary!(m_asinh, |ctx, x| decimal_core::inverse::asinh(ctx, x));
fallible_unary!(m_acosh, |ctx, x| decimal_core::inverse::acosh(ctx, x));
fallible_unary!(m_atanh, |ctx, x| decimal_core::inverse::atanh(ctx, x));

/// `naturalLogarithm`, which can raise `[DecimalError] Precision limit
/// exceeded` when the configured precision outruns the 1025-digit `LN10`
/// constant.
unsafe fn m_ln(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, class)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    let Some((outcome, exceeded)) =
        calculate(env, class, |ctx| decimal_core::elementary::ln(ctx, &x))
    else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    match outcome {
        Ok(value) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

binary!(m_plus, |ctx, x, y| arith::add(ctx, x, y));
binary!(m_minus, |ctx, x, y| arith::sub(ctx, x, y));
binary!(m_times, |ctx, x, y| arith::mul(ctx, x, y));
binary!(m_div, |ctx, x, y| {
    let rm = ctx.cfg.rounding;
    arith::divide(ctx, x, y, None, rm, false, None)
});
binary!(m_mod, |ctx, x, y| ops::modulo(ctx, x, y));
binary!(m_div_to_int, |ctx, x, y| decimal_core::trig::div_to_int(
    ctx, x, y
));

predicate!(m_is_nan, |x| x.is_nan());
predicate!(m_is_finite, |x| x.is_finite());
predicate!(m_is_integer, |x| x.is_integer());
predicate!(m_is_zero, |x| x.is_zero());
predicate!(m_is_negative, |x| x.is_negative());
predicate!(m_is_positive, |x| !x.is_negative() && !x.is_nan());

comparison!(m_equals, |o| o == Some(core::cmp::Ordering::Equal));
comparison!(m_lt, |o| o == Some(core::cmp::Ordering::Less));
comparison!(m_lte, |o| matches!(
    o,
    Some(core::cmp::Ordering::Less) | Some(core::cmp::Ordering::Equal)
));
comparison!(m_gt, |o| o == Some(core::cmp::Ordering::Greater));
comparison!(m_gte, |o| matches!(
    o,
    Some(core::cmp::Ordering::Greater) | Some(core::cmp::Ordering::Equal)
));

stringify!(m_to_fixed, |ctx, x, a, b| ops::to_fixed(ctx, x, a, b));
stringify!(m_to_exponential, |ctx, x, a, b| ops::to_exponential(
    ctx, x, a, b
));
stringify!(m_to_precision, |ctx, x, a, b| ops::to_precision(
    ctx, x, a, b
));
stringify!(m_to_binary, |ctx, x, a, b| {
    decimal_core::radix::to_string_binary(ctx, x, 2, a, b)
});
stringify!(m_to_octal, |ctx, x, a, b| {
    decimal_core::radix::to_string_binary(ctx, x, 8, a, b)
});
stringify!(m_to_hex, |ctx, x, a, b| {
    decimal_core::radix::to_string_binary(ctx, x, 16, a, b)
});

rounder!(m_to_dp, |ctx, x, a, b| ops::to_decimal_places(ctx, x, a, b));
rounder!(m_to_sd, |ctx, x, a, b| ops::to_significant_digits(
    ctx, x, a, b
));

/// `toFraction([maxDenominator])`, which returns a two-element array rather
/// than a Decimal — except for a non-finite receiver, where it returns a
/// Decimal after all.
///
/// The optional argument accepts `null` as well as `undefined` to mean
/// "absent", because the original's test is `maxD == null`, which is the loose
/// equality that holds for both. `argument` would coerce a `null` into a thrown
/// `[DecimalError] Invalid argument: null`, so the check has to come first.
unsafe fn m_to_fraction(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, class)) = receiver(env, info, 1) else {
        return env.undefined();
    };

    let given = match args.first().copied() {
        None => None,
        Some(v) if matches!(env.type_of(v), JsType::Undefined | JsType::Null) => None,
        Some(v) => match coerce(env, class, v) {
            Ok(bound) => Some(bound),
            Err(e) => return fail(env, e),
        },
    };

    let Some((outcome, exceeded)) = calculate(env, class, |ctx| {
        fraction::to_fraction(ctx, &x, given.as_ref())
    }) else {
        return env.undefined();
    };
    // Checked once, here, rather than being left to the two `make` calls below:
    // the first would throw and the second would then build a Decimal with an
    // exception already pending. See `abandoned`.
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }

    match outcome {
        Ok(fraction::Fractional::Ratio(f)) => {
            let numerator = make(env, class, f.numerator);
            let denominator = make(env, class, f.denominator);
            env.array(&[numerator, denominator])
        }
        Ok(fraction::Fractional::NonFinite(value)) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

/// `logarithm`, whose base argument is optional and defaults to 10.
unsafe fn m_log(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, class)) = receiver(env, info, 1) else {
        return env.undefined();
    };
    let base = match args.first().copied() {
        None => None,
        Some(v) if matches!(env.type_of(v), JsType::Undefined | JsType::Null) => None,
        Some(v) => match coerce(env, class, v) {
            Ok(b) => Some(b),
            Err(e) => return fail(env, e),
        },
    };
    let Some((outcome, exceeded)) = calculate(env, class, |ctx| {
        decimal_core::power::logarithm(ctx, &x, base.as_ref())
    }) else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    match outcome {
        Ok(value) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

/// `toPower`.
unsafe fn m_pow(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, class)) = receiver(env, info, 1) else {
        return env.undefined();
    };
    let y = match argument(env, class, &args, 0) {
        Ok(y) => y,
        Err(e) => return fail(env, e),
    };
    let Some((outcome, exceeded)) =
        calculate(env, class, |ctx| decimal_core::power::to_power(ctx, &x, &y))
    else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    match outcome {
        Ok(value) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

/// `Decimal.log2` and `Decimal.log10`, which are the logarithm with the base
/// supplied rather than taken from the caller.
macro_rules! static_log_base {
    ($name:ident, $base:literal) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let (args, class, _) = env.callback_info(info, 1);
            let first = args.first().copied().unwrap_or_else(|| env.undefined());
            let x = match coerce(env, class, first) {
                Ok(v) => v,
                Err(e) => return fail(env, e),
            };
            let base = Decimal::from_i32($base);
            let Some((outcome, exceeded)) = calculate(env, class, |ctx| {
                decimal_core::power::logarithm(ctx, &x, Some(&base))
            }) else {
                return env.undefined();
            };
            if let Some(thrown) = abandoned(env, exceeded) {
                return thrown;
            }
            match outcome {
                Ok(value) => make(env, class, value),
                Err(e) => fail(env, e),
            }
        }
    };
}

static_log_base!(s_log2, 2);
static_log_base!(s_log10, 10);

/// `clampedTo`, which takes two bounds and can reject an inverted range.
unsafe fn m_clamp(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, class)) = receiver(env, info, 2) else {
        return env.undefined();
    };
    let min = match argument(env, class, &args, 0) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };
    let max = match argument(env, class, &args, 1) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };
    let Some((outcome, exceeded)) = calculate(env, class, |ctx| ops::clamp(ctx, &x, &min, &max))
    else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    match outcome {
        Ok(value) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

/// `toNearest`, whose modulus argument is optional.
unsafe fn m_to_nearest(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, class)) = receiver(env, info, 2) else {
        return env.undefined();
    };
    let modulus = match args.first().copied() {
        None => None,
        Some(v) if matches!(env.type_of(v), JsType::Undefined | JsType::Null) => None,
        Some(v) => match coerce(env, class, v) {
            Ok(y) => Some(y),
            Err(e) => return fail(env, e),
        },
    };
    let rm = optional_number(env, &args, 1);
    let Some((outcome, exceeded)) = calculate(env, class, |ctx| {
        ops::to_nearest(ctx, &x, modulus.as_ref(), rm)
    }) else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    match outcome {
        Ok(value) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

/// `comparedTo`, which returns a number rather than a boolean and reports NaN
/// for an unordered pair.
unsafe fn m_compared_to(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, class)) = receiver(env, info, 1) else {
        return env.undefined();
    };
    let y = match argument(env, class, &args, 0) {
        Ok(y) => y,
        Err(e) => return fail(env, e),
    };
    env.number(match compare(&x, &y) {
        None => f64::NAN,
        Some(core::cmp::Ordering::Less) => -1.0,
        Some(core::cmp::Ordering::Equal) => 0.0,
        Some(core::cmp::Ordering::Greater) => 1.0,
    })
}

unsafe fn m_to_string(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, class)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    let Some(cfg) = with_state(env, class, |state| state.ctx.cfg) else {
        return env.undefined();
    };
    env.string(&format::to_string(&x, &cfg))
}

unsafe fn m_value_of(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, class)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    let Some(cfg) = with_state(env, class, |state| state.ctx.cfg) else {
        return env.undefined();
    };
    env.string(&format::value_of(&x, &cfg))
}

unsafe fn m_to_number(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, class)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    let Some(cfg) = with_state(env, class, |state| state.ctx.cfg) else {
        return env.undefined();
    };
    let text = format::value_of(&x, &cfg);
    env.number(text.parse::<f64>().unwrap_or(f64::NAN))
}

unsafe fn m_decimal_places(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, _)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    env.number(x.decimal_places().map_or(f64::NAN, |dp| dp as f64))
}

unsafe fn m_precision(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, _)) = receiver(env, info, 1) else {
        return env.undefined();
    };

    // `z !== void 0 && z !== !!z && z !== 1 && z !== 0` — so the argument may
    // be absent, a boolean, or the *numbers* 1 and 0, and nothing else. The
    // string `'1'` is refused, which is why this cannot be a truthiness test.
    let include_zeros = match args.first().copied() {
        None => false,
        Some(v) => match env.type_of(v) {
            JsType::Undefined => false,
            JsType::Boolean => env.as_bool(v).unwrap_or(false),
            JsType::Number if env.as_f64(v) == Some(1.0) => true,
            JsType::Number if env.as_f64(v) == Some(0.0) => false,
            _ => {
                return fail(env, Error::InvalidArgument(describe(env, v)));
            }
        },
    };

    if !x.is_finite() {
        return env.number(f64::NAN);
    }

    // With the flag set, an integer's trailing zeros count: `1e+123` has one
    // significant digit but 124 places before the point.
    let mut k = x.significant_digits();
    if include_zeros && x.e + 1 > k {
        k = x.e + 1;
    }
    env.number(k as f64)
}

// -- the accessors the test helper reads ------------------------------------

macro_rules! accessor {
    ($name:ident, $body:expr) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let (_, this, _) = env.callback_info(info, 0);
            let Some(x) = decimal_of(env, this) else {
                return env.undefined();
            };
            let f: fn(Env, &Decimal) -> Value = $body;
            f(env, &x)
        }
    };
}

// `assertEqualProps` in the original's test/setup.js reads `n.d[i]`,
// `n.d.length`, `n.e` and `n.s` directly, so these three have to be present on
// every instance and have to look like the original's plain properties.
accessor!(get_s, |env, x| env.number(match x.s {
    Sign::Pos => 1.0,
    Sign::Neg => -1.0,
    Sign::Nan => f64::NAN,
}));
accessor!(get_e, |env, x| env.number(if x.is_finite() {
    x.e as f64
} else {
    f64::NAN
}));
accessor!(get_d, |env, x| match &x.d {
    Some(limbs) => env.number_array(limbs),
    None => {
        let mut out: Value = ptr::null_mut();
        // SAFETY: valid out-pointer; `d` is null for a non-finite value in the
        // original, and the tests distinguish null from undefined.
        unsafe { sys::napi_get_null(env.0, &mut out) };
        out
    }
});

// ---------------------------------------------------------------------------
// Statics: configuration
// ---------------------------------------------------------------------------

/// One configurable setting: its name, a reader, and a writer.
///
/// Every setting is carried as an `f64` on the JavaScript side, including the
/// three that are conceptually integers — because that is what a JavaScript
/// number is, and because `config` validates the integrality itself rather
/// than letting a conversion do it silently.
type Setting = (&'static str, fn(&Config) -> f64, fn(&mut Config, f64));

/// The eight configurable settings, with the ranges `config` validates them
/// against and the accessors that read and write them.
const SETTINGS: &[Setting] = &[
    (
        "precision",
        |c| c.precision as f64,
        |c, v| c.precision = v as i64,
    ),
    (
        "rounding",
        |c| c.rounding as f64,
        |c, v| c.rounding = v as u8,
    ),
    ("modulo", |c| c.modulo as f64, |c, v| c.modulo = v as u8),
    (
        "toExpNeg",
        |c| c.to_exp_neg as f64,
        |c, v| c.to_exp_neg = v as i64,
    ),
    (
        "toExpPos",
        |c| c.to_exp_pos as f64,
        |c, v| c.to_exp_pos = v as i64,
    ),
    ("minE", |c| c.min_e as f64, |c, v| c.min_e = v as i64),
    ("maxE", |c| c.max_e as f64, |c, v| c.max_e = v as i64),
];

fn range_of(name: &str) -> (i64, i64) {
    match name {
        "precision" => Config::PRECISION_RANGE,
        "rounding" => Config::ROUNDING_RANGE,
        "modulo" => Config::MODULO_RANGE,
        "toExpNeg" => Config::TO_EXP_NEG_RANGE,
        "toExpPos" => Config::TO_EXP_POS_RANGE,
        "minE" => Config::MIN_E_RANGE,
        "maxE" => Config::MAX_E_RANGE,
        _ => (0, 0),
    }
}

unsafe fn s_config(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info(info, 1);

    let object = args.first().copied().unwrap_or_else(|| env.undefined());
    if env.type_of(object) != JsType::Object {
        // A distinct message from the range errors, and the original's exact
        // wording.
        env.throw("[DecimalError] Object expected");
        return env.undefined();
    }

    // `{ defaults: true }` resets every setting first; explicit settings in
    // the same object then apply on top.
    let defaults = env.get_named(object, "defaults");
    let use_defaults =
        env.type_of(defaults) == JsType::Boolean && env.as_bool(defaults) == Some(true);
    let default_cfg = Config::default();

    // Upstream applies settings in source order. Each property read may run a
    // getter and re-enter us, so every borrow ends before the next read.
    for (name, get, set) in SETTINGS {
        if use_defaults {
            let value = get(&default_cfg);
            if with_state_mut(env, class, |state| set(&mut state.ctx.cfg, value)).is_none() {
                return env.undefined();
            }
        }
        let raw = env.get_named(object, name);
        if env.type_of(raw) == JsType::Undefined {
            continue;
        }
        let value = env.as_f64(raw).unwrap_or(f64::NAN);
        let (min, max) = range_of(name);
        // The original's test is `mathfloor(v) === v && v >= min && v <= max`
        // — an integrality check, *not* the 32-bit truncation that
        // `checkInt32` performs elsewhere. The difference matters: minE is
        // -9e15, which is a perfectly good integer and not a valid `i32`.
        if value.floor() == value && value >= min as f64 && value <= max as f64 {
            if with_state_mut(env, class, |state| set(&mut state.ctx.cfg, value)).is_none() {
                return env.undefined();
            }
        } else {
            return fail(
                env,
                Error::InvalidArgument(format!("{name}: {}", describe(env, raw))),
            );
        }
    }

    if use_defaults
        && with_state_mut(env, class, |state| {
            state.ctx.cfg.crypto = default_cfg.crypto
        })
        .is_none()
    {
        return env.undefined();
    }

    let raw = env.get_named(object, "crypto");
    if env.type_of(raw) != JsType::Undefined {
        // `true`, `false`, `0` and `1` are all accepted.
        let requested = match env.type_of(raw) {
            JsType::Boolean => env.as_bool(raw),
            JsType::Number => env.as_f64(raw).and_then(|v| {
                if v == 0.0 {
                    Some(false)
                } else if v == 1.0 {
                    Some(true)
                } else {
                    None
                }
            }),
            _ => None,
        };
        match requested {
            Some(true) => {
                // The original's test, transcribed: a `crypto` global that
                // offers either entropy function. Node has supplied one since
                // v19, so this normally succeeds; on a host without it, the
                // configuration is refused rather than silently downgraded to
                // `Math.random`, which is the point of asking for it.
                if crypto_source(env).is_none() {
                    return fail(env, Error::CryptoUnavailable);
                }
                if with_state_mut(env, class, |state| state.ctx.cfg.crypto = true).is_none() {
                    return env.undefined();
                }
            }
            Some(false) => {
                if with_state_mut(env, class, |state| state.ctx.cfg.crypto = false).is_none() {
                    return env.undefined();
                }
            }
            None => {
                return fail(
                    env,
                    Error::InvalidArgument(format!("crypto: {}", describe(env, raw))),
                )
            }
        }
    }

    class
}

macro_rules! setting_accessor {
    ($getter:ident, $setter:ident, $index:expr) => {
        unsafe fn $getter(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let (_, class, _) = env.callback_info(info, 0);
            let Some(value) = with_state(env, class, |state| SETTINGS[$index].1(&state.ctx.cfg))
            else {
                return env.undefined();
            };
            env.number(value)
        }

        unsafe fn $setter(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let (args, class, _) = env.callback_info(info, 1);
            let value = args
                .first()
                .and_then(|&v| env.as_f64(v))
                .unwrap_or(f64::NAN);
            // Assigning directly — `Decimal.precision = 5` — is a plain
            // property write in the original, with no validation at all. Only
            // `config` validates. Reproduced rather than improved: a test that
            // sets an out-of-range value directly and observes the consequence
            // would otherwise see an exception the original never raises.
            let (_, _, set) = SETTINGS[$index];
            with_state_mut(env, class, |state| set(&mut state.ctx.cfg, value));
            env.undefined()
        }
    };
}

/// `Decimal.crypto`, which is a boolean and so cannot join the numeric
/// settings table.
///
/// Reading it is how the config tests check that `config` took the setting;
/// writing it directly is, as with the numeric settings, an unvalidated plain
/// property write in the original — `config` is the only validating door.
unsafe fn get_crypto(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (_, class, _) = env.callback_info(info, 0);
    let Some(value) = with_state(env, class, |state| state.ctx.cfg.crypto) else {
        return env.undefined();
    };
    env.boolean(value)
}

unsafe fn set_crypto(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info(info, 1);
    let value = args.first().and_then(|&v| env.as_bool(v)).unwrap_or(false);
    with_state_mut(env, class, |state| state.ctx.cfg.crypto = value);
    env.undefined()
}

setting_accessor!(get_precision, set_precision, 0);
setting_accessor!(get_rounding, set_rounding, 1);
setting_accessor!(get_modulo, set_modulo, 2);
setting_accessor!(get_to_exp_neg, set_to_exp_neg, 3);
setting_accessor!(get_to_exp_pos, set_to_exp_pos, 4);
setting_accessor!(get_min_e, set_min_e, 5);
setting_accessor!(get_max_e, set_max_e, 6);

/// Declares a static that is the instance method of the same meaning, applied
/// to a freshly constructed receiver.
///
/// This is exactly how the original defines them — `function abs(x) { return
/// new this(x).abs(); }` — and going through the real prototype method rather
/// than reimplementing keeps the two spellings from ever drifting apart.
macro_rules! static_via_instance {
    ($name:ident, $method:literal) => {
        unsafe fn $name(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
            let env = Env(env);
            let (args, class, _) = env.callback_info(info, 3);

            let first = args.first().copied().unwrap_or_else(|| env.undefined());
            let receiver = match coerce(env, class, first) {
                Ok(value) => make(env, class, value),
                Err(e) => return fail(env, e),
            };
            let function = env.get_named(receiver, $method);
            env.call(receiver, function, &args[1.min(args.len())..])
        }
    };
}

static_via_instance!(s_abs, "abs");
static_via_instance!(s_acos, "acos");
static_via_instance!(s_acosh, "acosh");
static_via_instance!(s_add, "plus");
static_via_instance!(s_asin, "asin");
static_via_instance!(s_asinh, "asinh");
static_via_instance!(s_atan, "atan");
static_via_instance!(s_atanh, "atanh");
static_via_instance!(s_cbrt, "cbrt");
static_via_instance!(s_ceil, "ceil");
static_via_instance!(s_clamp, "clamp");
static_via_instance!(s_cos, "cos");
static_via_instance!(s_cosh, "cosh");
static_via_instance!(s_div, "div");
static_via_instance!(s_exp, "exp");
static_via_instance!(s_floor, "floor");
static_via_instance!(s_ln, "ln");
static_via_instance!(s_log, "log");
static_via_instance!(s_mod, "mod");
static_via_instance!(s_mul, "times");
static_via_instance!(s_pow, "pow");
static_via_instance!(s_round, "round");
static_via_instance!(s_sin, "sin");
static_via_instance!(s_sinh, "sinh");
static_via_instance!(s_sqrt, "sqrt");
static_via_instance!(s_sub, "minus");
static_via_instance!(s_tan, "tan");
static_via_instance!(s_tanh, "tanh");
static_via_instance!(s_trunc, "trunc");

/// The `crypto` global, if it exists and offers either entropy function.
///
/// This is the original's
/// `typeof crypto != 'undefined' && crypto && (crypto.getRandomValues || crypto.randomBytes)`,
/// with the same two names checked in the same order.
fn crypto_source(env: Env) -> Option<Value> {
    let crypto = env.get_named(env.global(), "crypto");
    if env.type_of(crypto) != JsType::Object {
        return None;
    }
    let has = |name: &str| env.type_of(env.get_named(crypto, name)) == JsType::Function;
    (has("getRandomValues") || has("randomBytes")).then_some(crypto)
}

/// Limbs drawn from `crypto.getRandomValues`.
///
/// # The rejection rule
///
/// A `u32` is uniform on `[0, 2³²)`, and `2³² mod 10⁷` is not zero, so taking
/// it modulo `10⁷` would favour the low end of the range. The original fixes
/// this by discarding any draw of `4.29e9` or more and redrawing: `4 290 000 000`
/// is exactly `429 × 10⁷`, so what remains is a whole number of complete cycles
/// and the modulo is uniform. The waste is one draw in 865.
///
/// The original fetches all `k` words in one call and redraws singly into the
/// gaps; this fetches singly throughout. The stream of values consumed and the
/// rule applied to each are identical, and nothing observable counts the calls.
struct CryptoEntropy {
    env: Env,
    crypto: Value,
    array: Value,
}

impl CryptoEntropy {
    /// A source holding a one-element `Uint32Array` to be filled repeatedly.
    ///
    /// `getRandomValues` writes into the array it is given and returns it, so
    /// one allocation serves every draw.
    fn new(env: Env, crypto: Value) -> Option<Self> {
        let constructor = env.get_named(env.global(), "Uint32Array");
        if env.type_of(constructor) != JsType::Function {
            return None;
        }
        let array = env.construct(constructor, &[env.number(1.0)]);
        Some(CryptoEntropy { env, crypto, array })
    }

    /// One uniform `u32`.
    fn draw(&mut self) -> u32 {
        let function = self.env.get_named(self.crypto, "getRandomValues");
        let filled = self.env.call(self.crypto, function, &[self.array]);
        let value = self.env.get_element(filled, 0);
        self.env.as_f64(value).unwrap_or(0.0) as u32
    }
}

impl decimal_core::random::Entropy for CryptoEntropy {
    fn next_limb(&mut self) -> u32 {
        loop {
            let n = self.draw();
            if n < 4_290_000_000 {
                return n % 10_000_000;
            }
        }
    }
}

/// `Decimal.random([sd])`.
///
/// The generator behind the default path is this crate's own — see
/// `decimal_core::random`. It stands where the original has `Math.random()`,
/// and shares that function's standing exactly: adequate for anything that is
/// not a secret, and used only when `crypto` is off.
unsafe fn s_random(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info(info, 1);

    let sd = optional_number(env, &args, 0);
    let Some((ctx, crypto, mut entropy)) = with_state(env, class, |state| {
        (state.ctx, state.ctx.cfg.crypto, state.entropy.clone())
    }) else {
        return env.undefined();
    };

    let drawn = if crypto {
        // Configured for crypto, so it was reachable when `config` accepted the
        // setting. If it has since been removed from the global object, the
        // original would throw a TypeError from the call itself; refusing here
        // with its own message is the closer answer.
        match crypto_source(env).and_then(|crypto| CryptoEntropy::new(env, crypto)) {
            Some(mut source) => decimal_core::random::random(&ctx, sd, &mut source),
            None => return fail(env, Error::CryptoUnavailable),
        }
    } else {
        let result = decimal_core::random::random(&ctx, sd, &mut entropy);
        with_state_mut(env, class, |state| state.entropy = entropy);
        result
    };

    match drawn {
        Ok(value) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

/// `Decimal.atan2(y, x)`.
///
/// Both arguments are coerced before either is used, so a bad `x` raises even
/// when `y` is already NaN — the original writes `y = new this(y); x = new
/// this(x);` on two consecutive lines, ahead of every test.
unsafe fn s_atan2(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info(info, 2);

    let y = match argument(env, class, &args, 0) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };
    let x = match argument(env, class, &args, 1) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };

    let Some((outcome, exceeded)) = calculate(env, class, |ctx| inverse::atan2(ctx, &y, &x)) else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    match outcome {
        Ok(value) => make(env, class, value),
        Err(e) => fail(env, e),
    }
}

/// `Decimal.hypot(…)`.
///
/// Arguments are coerced one at a time, inside the loop, because the original
/// does: an infinite argument returns before the rest are ever looked at, so a
/// later value that would fail to convert never gets the chance.
unsafe fn s_hypot(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info_variadic(info);

    // `external = false` comes *before* the first `new this(arguments[i++])` in
    // the original, so no operand is measured against `minE`/`maxE` on the way
    // in. Coercing first and suppressing the clamps afterwards would turn an
    // operand above `maxE` into Infinity and short-circuit the whole call.
    // `roots::hypot` sets the flag back on, as the original does.
    if with_state_mut(env, class, |state| state.ctx.external = false).is_none() {
        return env.undefined();
    }

    let mut values = Vec::with_capacity(args.len());
    for index in 0..args.len() {
        match argument(env, class, &args, index) {
            Ok(v) => {
                let infinite = v.is_infinite();
                values.push(v);
                if infinite {
                    break;
                }
            }
            Err(e) => {
                // The original throws from here with the flag still cleared and
                // nothing to restore it, so the library stops clamping for the
                // rest of the process. Declined, as in D-11 and D-16.
                with_state_mut(env, class, |state| state.ctx.external = true);
                return fail(env, e);
            }
        }
    }

    let Some((value, exceeded)) = calculate(env, class, |ctx| roots::hypot(ctx, &values)) else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    make(env, class, value)
}

/// `Decimal.sum(…)`.
///
/// Coerced lazily for the same reason as `hypot`, and stopping at the first
/// NaN: `Decimal.sum(NaN, {})` is NaN, because the `{}` is never constructed.
unsafe fn s_sum(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info_variadic(info);

    // With no arguments at all the original still evaluates `new this(args[0])`
    // — that is, `new Decimal(undefined)` — and raises. Deferring to `argument`
    // with an out-of-range index reproduces both the error and its text.
    let count = args.len().max(1);
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        // The first argument is coerced under the clamps and the rest are not.
        // The original's `x = new this(args[i])` sits one line *above* its
        // `external = false`, and every later argument is coerced inside
        // `x.plus(args[i])`, which is below it. So `Decimal.sum(a, b)` measures
        // `a` against `maxE` and does not measure `b`. Asymmetric, and easy to
        // read as an oversight — but it is what the library does, and `hypot`
        // three functions above puts the same line on the other side.
        if index == 1 {
            with_state_mut(env, class, |state| state.ctx.external = false);
        }
        match argument(env, class, &args, index) {
            Ok(v) => {
                let is_nan = v.is_nan();
                values.push(v);
                if is_nan {
                    break;
                }
            }
            Err(e) => {
                with_state_mut(env, class, |state| state.ctx.external = true);
                return fail(env, e);
            }
        }
    }
    with_state_mut(env, class, |state| state.ctx.external = true);

    let Some((value, exceeded)) = calculate(env, class, |ctx| arith::sum(ctx, &values)) else {
        return env.undefined();
    };
    if let Some(thrown) = abandoned(env, exceeded) {
        return thrown;
    }
    make(env, class, value)
}

/// `Decimal.max` and `Decimal.min`, which take any number of arguments.
///
/// # The tie-break, which is the whole of the difficulty
///
/// The original's test is `k === n || k === 0 && x.s === n`, where `n` is −1
/// for `max` and 1 for `min`. The first disjunct is the obvious comparison. The
/// second exists because `cmp` reports `-0` and `0` as **equal**, so a plain
/// comparison could not choose between them — and the tests demand a choice:
/// `Decimal.max(-2, -1, -0, 0)` is `0`, while `Decimal.min(0, -0)` is `-0`.
///
/// Reading it as a rule: on a tie, replace the incumbent when its sign is the
/// wrong one. `max` discards a negative incumbent for an equal challenger, so
/// the last non-negative equal value wins; `min` does the mirror image.
///
/// # The early exit
///
/// A NaN *argument* ends the scan immediately, so later arguments are never
/// converted and a later value that would fail to convert never gets the
/// chance. A NaN produced by the first argument does not exit — nothing can
/// displace it, since every comparison against it is unordered.
unsafe fn s_max_or_min<const WANT_GREATER: bool>(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info_variadic(info);

    // With no arguments the original evaluates `new Ctor(args[0])`, i.e.
    // `new Decimal(undefined)`, and raises. Asking for index 0 regardless
    // reproduces the error and its wording.
    let mut best = match argument(env, class, &args, 0) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };

    // `n` in the original: the comparison outcome that means "replace".
    let replacing = if WANT_GREATER {
        core::cmp::Ordering::Less
    } else {
        core::cmp::Ordering::Greater
    };
    let wrong_sign = |s: Sign| s.is_negative() == WANT_GREATER;

    for index in 1..args.len() {
        let challenger = match argument(env, class, &args, index) {
            Ok(v) => v,
            Err(e) => return fail(env, e),
        };
        if challenger.is_nan() {
            best = challenger;
            break;
        }
        let replace = match compare(&best, &challenger) {
            Some(order) if order == replacing => true,
            Some(core::cmp::Ordering::Equal) => wrong_sign(best.s),
            _ => false,
        };
        if replace {
            best = challenger;
        }
    }

    make(env, class, best)
}

unsafe fn s_max(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    s_max_or_min::<true>(env, info)
}

unsafe fn s_min(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    s_max_or_min::<false>(env, info)
}

/// `Decimal.sign`, which returns a plain number and distinguishes the two
/// zeros.
unsafe fn s_sign(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, class, _) = env.callback_info(info, 1);

    let first = args.first().copied().unwrap_or_else(|| env.undefined());
    let value = match coerce(env, class, first) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };

    env.number(if value.is_nan() {
        f64::NAN
    } else if value.is_zero() {
        // Signed zero survives: sign(-0) is -0, not 0.
        if value.is_negative() {
            -0.0
        } else {
            0.0
        }
    } else if value.is_negative() {
        -1.0
    } else {
        1.0
    })
}

unsafe fn s_is_decimal(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, _, _) = env.callback_info(info, 1);
    let is = args
        .first()
        .map(|&v| decimal_of(env, v).is_some())
        .unwrap_or(false);
    env.boolean(is)
}

unsafe fn s_clone(env: sys::napi_env, info: sys::napi_callback_info) -> sys::napi_value {
    let env = Env(env);
    let (args, parent, _) = env.callback_info(info, 1);

    // A clone starts from the parent's settings unless `{ defaults: true }`
    // asks for a fresh constructor.
    let Some(mut cfg) = with_state(env, parent, |state| state.ctx.cfg) else {
        return env.undefined();
    };
    if let Some(&object) = args.first() {
        if env.type_of(object) == JsType::Object
            && env.has_own(object, "defaults")
            && env.as_bool(env.get_named(object, "defaults")) == Some(true)
        {
            cfg = Config::default();
        }
    }

    let prototype = env.get_named(parent, "prototype");
    let class = build_class(env, cfg, Some(prototype));

    // The original ends with an *unconditional* `Decimal.config(obj)`, having
    // replaced an absent argument with `{}` — and only an absent one. So
    // `clone(null)` reaches `config(null)`, which refuses it: "Object expected".
    // Routing every non-absent argument through the real `config` is therefore
    // both the validation and the application, and it keeps the two spellings
    // of every settings check in one place.
    //
    // The constructor is built first here as it is there, so a rejected
    // argument throws *after* the class exists — which nothing can observe,
    // since the throw discards the return value.
    let given = args
        .first()
        .copied()
        .filter(|&v| env.type_of(v) != JsType::Undefined);

    if let Some(object) = given {
        let config_fn = env.get_named(class, "config");
        let mut result: Value = ptr::null_mut();
        let argv = [object];
        // SAFETY: `class` and `config_fn` are live handles this function just
        // created; `argv` holds one live handle.
        sys::napi_call_function(env.0, class, config_fn, 1, argv.as_ptr(), &mut result);
        if env.is_exception_pending() {
            return env.undefined();
        }
    }

    class
}

// ---------------------------------------------------------------------------
// Building a class
// ---------------------------------------------------------------------------

/// A property descriptor for a method.
fn method(
    name: &'static str,
    cb: sys::napi_callback,
    data: *mut c_void,
) -> sys::napi_property_descriptor {
    sys::napi_property_descriptor {
        utf8name: name.as_ptr().cast(),
        name: ptr::null_mut(),
        method: cb,
        getter: None,
        setter: None,
        value: ptr::null_mut(),
        attributes: sys::PropertyAttributes::default,
        data,
    }
}

/// A property descriptor for an instance accessor.
fn getter(
    name: &'static str,
    cb: sys::napi_callback,
    data: *mut c_void,
) -> sys::napi_property_descriptor {
    sys::napi_property_descriptor {
        utf8name: name.as_ptr().cast(),
        name: ptr::null_mut(),
        method: None,
        getter: cb,
        setter: None,
        value: ptr::null_mut(),
        attributes: sys::PropertyAttributes::default,
        data,
    }
}

/// A property descriptor for a static method or accessor on the constructor.
/// A property holding an existing value, with the attributes
/// `napi_define_class` gives a method: not writable, not enumerable, not
/// configurable.
///
/// Matching those attributes is the reason this exists rather than a plain
/// `set_named`, which would make the alias enumerable and so put it in
/// `Object.keys(Decimal.prototype)` where the original has neither name.
fn same_function(name: &'static str, value: Value) -> sys::napi_property_descriptor {
    sys::napi_property_descriptor {
        utf8name: name.as_ptr().cast(),
        name: ptr::null_mut(),
        method: None,
        getter: None,
        setter: None,
        value,
        attributes: sys::PropertyAttributes::default,
        data: ptr::null_mut(),
    }
}

fn static_entry(
    name: &'static str,
    method_cb: sys::napi_callback,
    getter_cb: sys::napi_callback,
    setter_cb: sys::napi_callback,
    data: *mut c_void,
) -> sys::napi_property_descriptor {
    sys::napi_property_descriptor {
        utf8name: name.as_ptr().cast(),
        name: ptr::null_mut(),
        method: method_cb,
        getter: getter_cb,
        setter: setter_cb,
        value: ptr::null_mut(),
        attributes: sys::PropertyAttributes::static_,
        data,
    }
}

/// Define one `Decimal` constructor with the given configuration.
///
/// Every name below carries a NUL terminator in its literal, because
/// `napi_property_descriptor` takes a C string and the descriptor table is
/// built from `&'static str` rather than from allocated `CString`s.
fn build_class(env: Env, cfg: Config, shared_prototype: Option<Value>) -> Value {
    let boxed = Box::new(ConstructorData {
        state: RefCell::new(ConstructorState {
            ctx: Ctx::new(cfg),
            entropy: Xoshiro256StarStar::from_environment(),
        }),
        constructor: ptr::null_mut(),
    });
    let data: *mut c_void = Box::into_raw(boxed).cast();

    let mut properties: Vec<sys::napi_property_descriptor> = Vec::new();

    // Instance methods. Each row is one function and every name it answers
    // to; the first is defined on the class and the rest are installed
    // afterwards as *the same function object*, because the original writes
    // `P.absoluteValue = P.abs = function …` and its tests check the identity
    // — `Decimal.prototype.toDP === Decimal.prototype.toDecimalPlaces` is an
    // assertion, not an assumption.
    let instance: &[(&'static [&'static str], sys::napi_callback)] = &[
        (&["absoluteValue\0", "abs\0"], Some(guarded!(m_abs))),
        (&["negated\0", "neg\0"], Some(guarded!(m_neg))),
        (&["ceil\0"], Some(guarded!(m_ceil))),
        (&["floor\0"], Some(guarded!(m_floor))),
        (&["round\0"], Some(guarded!(m_round))),
        (&["truncated\0", "trunc\0"], Some(guarded!(m_trunc))),
        (&["plus\0", "add\0"], Some(guarded!(m_plus))),
        (&["minus\0", "sub\0"], Some(guarded!(m_minus))),
        (&["times\0", "mul\0"], Some(guarded!(m_times))),
        (&["dividedBy\0", "div\0"], Some(guarded!(m_div))),
        (
            &["dividedToIntegerBy\0", "divToInt\0"],
            Some(guarded!(m_div_to_int)),
        ),
        (&["modulo\0", "mod\0"], Some(guarded!(m_mod))),
        (&["comparedTo\0", "cmp\0"], Some(guarded!(m_compared_to))),
        (&["equals\0", "eq\0"], Some(guarded!(m_equals))),
        (&["lessThan\0", "lt\0"], Some(guarded!(m_lt))),
        (&["lessThanOrEqualTo\0", "lte\0"], Some(guarded!(m_lte))),
        (&["greaterThan\0", "gt\0"], Some(guarded!(m_gt))),
        (&["greaterThanOrEqualTo\0", "gte\0"], Some(guarded!(m_gte))),
        (&["isNaN\0"], Some(guarded!(m_is_nan))),
        (&["isFinite\0"], Some(guarded!(m_is_finite))),
        (&["isInteger\0", "isInt\0"], Some(guarded!(m_is_integer))),
        (&["isZero\0"], Some(guarded!(m_is_zero))),
        (&["isNegative\0", "isNeg\0"], Some(guarded!(m_is_negative))),
        (&["isPositive\0", "isPos\0"], Some(guarded!(m_is_positive))),
        (
            &["decimalPlaces\0", "dp\0"],
            Some(guarded!(m_decimal_places)),
        ),
        (&["precision\0", "sd\0"], Some(guarded!(m_precision))),
        (&["toDecimalPlaces\0", "toDP\0"], Some(guarded!(m_to_dp))),
        (
            &["toSignificantDigits\0", "toSD\0"],
            Some(guarded!(m_to_sd)),
        ),
        (&["toFixed\0"], Some(guarded!(m_to_fixed))),
        (&["toExponential\0"], Some(guarded!(m_to_exponential))),
        (&["toPrecision\0"], Some(guarded!(m_to_precision))),
        (&["toNumber\0"], Some(guarded!(m_to_number))),
        (&["toString\0"], Some(guarded!(m_to_string))),
        (&["valueOf\0", "toJSON\0"], Some(guarded!(m_value_of))),
        (&["clampedTo\0", "clamp\0"], Some(guarded!(m_clamp))),
        (&["toNearest\0"], Some(guarded!(m_to_nearest))),
        (&["inverseCosine\0", "acos\0"], Some(guarded!(m_acos))),
        (
            &["inverseHyperbolicCosine\0", "acosh\0"],
            Some(guarded!(m_acosh)),
        ),
        (&["inverseSine\0", "asin\0"], Some(guarded!(m_asin))),
        (
            &["inverseHyperbolicSine\0", "asinh\0"],
            Some(guarded!(m_asinh)),
        ),
        (&["inverseTangent\0", "atan\0"], Some(guarded!(m_atan))),
        (
            &["inverseHyperbolicTangent\0", "atanh\0"],
            Some(guarded!(m_atanh)),
        ),
        (&["cubeRoot\0", "cbrt\0"], Some(guarded!(m_cbrt))),
        (&["cosine\0", "cos\0"], Some(guarded!(m_cos))),
        (&["hyperbolicCosine\0", "cosh\0"], Some(guarded!(m_cosh))),
        (&["naturalExponential\0", "exp\0"], Some(guarded!(m_exp))),
        (&["naturalLogarithm\0", "ln\0"], Some(guarded!(m_ln))),
        (&["logarithm\0", "log\0"], Some(guarded!(m_log))),
        (&["toPower\0", "pow\0"], Some(guarded!(m_pow))),
        (&["sine\0", "sin\0"], Some(guarded!(m_sin))),
        (&["hyperbolicSine\0", "sinh\0"], Some(guarded!(m_sinh))),
        (&["squareRoot\0", "sqrt\0"], Some(guarded!(m_sqrt))),
        (&["tangent\0", "tan\0"], Some(guarded!(m_tan))),
        (&["hyperbolicTangent\0", "tanh\0"], Some(guarded!(m_tanh))),
        (&["toBinary\0"], Some(guarded!(m_to_binary))),
        (&["toHexadecimal\0", "toHex\0"], Some(guarded!(m_to_hex))),
        (&["toOctal\0"], Some(guarded!(m_to_octal))),
        (&["toFraction\0"], Some(guarded!(m_to_fraction))),
    ];
    let accessors: &[(&'static str, sys::napi_callback)] = &[
        ("s\0", Some(guarded!(get_s))),
        ("e\0", Some(guarded!(get_e))),
        ("d\0", Some(guarded!(get_d))),
    ];
    // Statics.
    // `set` is not defined here: it is installed below as the very function
    // object `config` becomes, because `Decimal.set === Decimal.config` is one
    // of the original's assertions.
    properties.push(static_entry(
        "config\0",
        Some(guarded!(s_config)),
        None,
        None,
        data,
    ));
    properties.push(static_entry(
        "clone\0",
        Some(guarded!(s_clone)),
        None,
        None,
        data,
    ));
    properties.push(static_entry(
        "isDecimal\0",
        Some(guarded!(s_is_decimal)),
        None,
        None,
        data,
    ));

    let statics: &[(&'static str, sys::napi_callback)] = &[
        ("abs\0", Some(guarded!(s_abs))),
        ("acos\0", Some(guarded!(s_acos))),
        ("acosh\0", Some(guarded!(s_acosh))),
        ("add\0", Some(guarded!(s_add))),
        ("asin\0", Some(guarded!(s_asin))),
        ("asinh\0", Some(guarded!(s_asinh))),
        ("atan\0", Some(guarded!(s_atan))),
        ("atanh\0", Some(guarded!(s_atanh))),
        ("atan2\0", Some(guarded!(s_atan2))),
        ("cbrt\0", Some(guarded!(s_cbrt))),
        ("ceil\0", Some(guarded!(s_ceil))),
        ("clamp\0", Some(guarded!(s_clamp))),
        ("cos\0", Some(guarded!(s_cos))),
        ("cosh\0", Some(guarded!(s_cosh))),
        ("div\0", Some(guarded!(s_div))),
        ("exp\0", Some(guarded!(s_exp))),
        ("floor\0", Some(guarded!(s_floor))),
        ("hypot\0", Some(guarded!(s_hypot))),
        ("ln\0", Some(guarded!(s_ln))),
        ("log\0", Some(guarded!(s_log))),
        ("log2\0", Some(guarded!(s_log2))),
        ("log10\0", Some(guarded!(s_log10))),
        ("max\0", Some(guarded!(s_max))),
        ("min\0", Some(guarded!(s_min))),
        ("mod\0", Some(guarded!(s_mod))),
        ("mul\0", Some(guarded!(s_mul))),
        ("pow\0", Some(guarded!(s_pow))),
        ("random\0", Some(guarded!(s_random))),
        ("round\0", Some(guarded!(s_round))),
        ("sign\0", Some(guarded!(s_sign))),
        ("sin\0", Some(guarded!(s_sin))),
        ("sinh\0", Some(guarded!(s_sinh))),
        ("sqrt\0", Some(guarded!(s_sqrt))),
        ("sub\0", Some(guarded!(s_sub))),
        ("sum\0", Some(guarded!(s_sum))),
        ("tan\0", Some(guarded!(s_tan))),
        ("tanh\0", Some(guarded!(s_tanh))),
        ("trunc\0", Some(guarded!(s_trunc))),
    ];
    for &(name, cb) in statics {
        properties.push(static_entry(name, cb, None, None, data));
    }

    properties.push(static_entry(
        "precision\0",
        None,
        Some(guarded!(get_precision)),
        Some(guarded!(set_precision)),
        data,
    ));
    properties.push(static_entry(
        "rounding\0",
        None,
        Some(guarded!(get_rounding)),
        Some(guarded!(set_rounding)),
        data,
    ));
    properties.push(static_entry(
        "modulo\0",
        None,
        Some(guarded!(get_modulo)),
        Some(guarded!(set_modulo)),
        data,
    ));
    properties.push(static_entry(
        "toExpNeg\0",
        None,
        Some(guarded!(get_to_exp_neg)),
        Some(guarded!(set_to_exp_neg)),
        data,
    ));
    properties.push(static_entry(
        "toExpPos\0",
        None,
        Some(guarded!(get_to_exp_pos)),
        Some(guarded!(set_to_exp_pos)),
        data,
    ));
    properties.push(static_entry(
        "minE\0",
        None,
        Some(guarded!(get_min_e)),
        Some(guarded!(set_min_e)),
        data,
    ));
    properties.push(static_entry(
        "maxE\0",
        None,
        Some(guarded!(get_max_e)),
        Some(guarded!(set_max_e)),
        data,
    ));
    properties.push(static_entry(
        "crypto\0",
        None,
        Some(guarded!(get_crypto)),
        Some(guarded!(set_crypto)),
        data,
    ));

    // SAFETY: `properties` lives until after the call returns. Constructor
    // callback data is owned by `class` below and therefore has precisely the
    // same lifetime as the callback that uses it.
    let class = unsafe {
        define_class(
            env,
            "Decimal",
            Some(guarded!(construct_decimal)),
            data,
            &properties,
        )
    };

    // Upstream creates one plain prototype `P` and assigns it to every clone.
    // Defining its methods with `napi_define_properties` avoids the V8 class
    // signature that rejects a method called on an instance of another clone.
    let owns_prototype = shared_prototype.is_none();
    let prototype = shared_prototype.unwrap_or_else(|| env.object());
    env.set_named(class, "prototype", prototype);

    if owns_prototype {
        let mut primary: Vec<sys::napi_property_descriptor> = instance
            .iter()
            .map(|&(names, cb)| method(names[0], cb, ptr::null_mut()))
            .collect();
        primary.extend(
            accessors
                .iter()
                .map(|&(name, cb)| getter(name, cb, ptr::null_mut())),
        );
        // SAFETY: descriptors and their NUL-terminated names live through the
        // call. Their callbacks recover state from each instance instead of
        // holding constructor-specific callback data.
        unsafe { env.define_properties(prototype, &primary) };
    }

    // Install every alias as the function object already defined, rather than
    // as a second function over the same callback. `Decimal.set` must *be*
    // `Decimal.config`, and `toDP` must *be* `toDecimalPlaces`; the original
    // gets that for free from `P.toDP = P.toDecimalPlaces = …`, and a
    // descriptor table does not — it would mint one function per row.
    if owns_prototype {
        let mut aliases: Vec<sys::napi_property_descriptor> = Vec::new();
        for &(names, _) in instance {
            let function = env.get_named(prototype, names[0].trim_end_matches('\0'));
            for &alias in &names[1..] {
                aliases.push(same_function(alias, function));
            }
        }
        // SAFETY: `aliases` outlives the call and every name is NUL-terminated.
        unsafe { env.define_properties(prototype, &aliases) };
    }

    let config_fn = env.get_named(class, "config");
    // SAFETY: as above.
    unsafe { env.define_properties(class, &[same_function("set\0", config_fn)]) };

    // The rounding-mode constants.
    for (index, name) in [
        "ROUND_UP",
        "ROUND_DOWN",
        "ROUND_CEIL",
        "ROUND_FLOOR",
        "ROUND_HALF_UP",
        "ROUND_HALF_DOWN",
        "ROUND_HALF_EVEN",
        "ROUND_HALF_CEIL",
        "ROUND_HALF_FLOOR",
    ]
    .iter()
    .enumerate()
    {
        let value = env.number(index as f64);
        env.set_named(class, name, value);
    }
    let euclid = env.number(9.0);
    env.set_named(class, "EUCLID", euclid);

    // SAFETY: `data` came from `Box::into_raw` above and has not been
    // reconstructed. `wrap_with_weak_reference` transfers it to `class` and
    // deletes its weak self-reference when the class is collected.
    let owner = unsafe { Box::from_raw(data.cast::<ConstructorData>()) };
    env.wrap_with_weak_reference(class, owner);

    class
}

// ---------------------------------------------------------------------------
// Module entry point
// ---------------------------------------------------------------------------

/// Node calls this on `require()`.
///
/// Its **return value** becomes `module.exports` when it differs from the
/// `exports` object passed in, which is what lets the module itself be the
/// `Decimal` constructor.
///
/// # Safety
///
/// Called once by Node, on the module's loading thread, with a valid `env`.
#[no_mangle]
pub unsafe extern "C" fn napi_register_module_v1(
    env: sys::napi_env,
    _exports: sys::napi_value,
) -> sys::napi_value {
    bind_symbols();
    build_class(Env(env), Config::default(), None)
}
