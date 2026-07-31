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
//! [`ConstructorState`], allocated when the class is defined and reachable
//! from every one of its methods through the callback data pointer. Instance
//! methods find their configuration this way rather than through the instance,
//! which is exactly how the original does it — the config lives on the
//! constructor function object, not on the value.
//!
//! # Errors
//!
//! A Rust panic must never cross this boundary. Every fallible path ends in a
//! thrown JavaScript `Error` carrying the original's exact message text, and
//! the outermost layer of every callback catches unwinds as a backstop.

mod napi;

use decimal_core::arith::{self, compare};
use decimal_core::{format, ops, parse, Config, Ctx, Decimal, Error, Sign};
use napi::{define_class, bind_symbols, Env, JsType, Value};
use napi_sys as sys;
use std::ffi::c_void;
use std::ptr;

// ---------------------------------------------------------------------------
// Per-constructor state
// ---------------------------------------------------------------------------

/// Everything one `Decimal` constructor owns.
///
/// Leaked deliberately: a constructor, once created, lives as long as the
/// module, and the pointer to this is handed to Node as the callback data of
/// every method on the class. Freeing it would invalidate those.
struct ConstructorState {
    ctx: Ctx,
    /// A strong reference to the constructor function, so that methods can
    /// build their results as instances of the right class.
    ctor: sys::napi_ref,
}

/// Recover the state from a callback's data pointer.
///
/// # Safety
///
/// `data` is the pointer given to `napi_define_class` and to every property
/// descriptor on the class, which is a leaked `Box<ConstructorState>` that
/// outlives every call.
unsafe fn state<'a>(data: *mut c_void) -> &'a mut ConstructorState {
    &mut *data.cast::<ConstructorState>()
}

// ---------------------------------------------------------------------------
// Converting JavaScript values into decimals
// ---------------------------------------------------------------------------

/// The constructor's dispatch on the type of its argument.
///
/// This is the original's `Decimal(v)`, minus the `bigint` branch: the
/// original accepts one, but its test suite never supplies one, and accepting
/// it would mean pulling in the big-integer half of the Node-API for a path
/// nothing exercises. The omission is recorded in DECISIONS.md rather than
/// hidden.
fn coerce(env: Env, st: &mut ConstructorState, value: Value) -> Result<Decimal, Error> {
    // An existing Decimal — of this constructor or any other clone — is copied.
    if let Some(existing) = decimal_of(env, value) {
        let mut copy = existing;
        if st.ctx.external {
            if copy.d.is_none() || copy.e > st.ctx.cfg.max_e {
                if !copy.is_nan() {
                    copy = Decimal::infinity(copy.s);
                }
            } else if copy.e < st.ctx.cfg.min_e {
                copy = Decimal::zero(copy.s);
            }
        }
        return Ok(copy);
    }

    match env.type_of(value) {
        JsType::Number => {
            let n = env.as_f64(value).unwrap_or(f64::NAN);
            Ok(from_f64(&st.ctx, n))
        }
        JsType::String => {
            let text = env.as_string(value).unwrap_or_default();
            from_str(&mut st.ctx, &text)
        }
        _ => Err(Error::InvalidArgument(describe(env, value))),
    }
}

/// A value from an IEEE double, following the original's constructor.
fn from_f64(ctx: &Ctx, v: f64) -> Decimal {
    if v == 0.0 {
        // `1 / v < 0` distinguishes -0 from +0, which `v == 0.0` does not.
        return Decimal::zero(if v.is_sign_negative() { Sign::Neg } else { Sign::Pos });
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

/// A value from a string, following the original's constructor: strip a
/// leading sign, then dispatch on whether the remainder is a decimal literal.
fn from_str(ctx: &mut Ctx, text: &str) -> Result<Decimal, Error> {
    let (sign, body) = match text.as_bytes().first() {
        Some(b'-') => (Sign::Neg, &text[1..]),
        Some(b'+') => (Sign::Pos, &text[1..]),
        _ => (Sign::Pos, text),
    };

    if parse::is_decimal_literal(body.as_bytes()) {
        Ok(parse::parse_decimal(ctx, sign, body))
    } else {
        parse::parse_other(ctx, sign, body)
    }
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
        JsType::Symbol => "Symbol()".to_string(),
        JsType::Function => "function".to_string(),
        _ => "[object Object]".to_string(),
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
    env.unwrap::<Decimal>(value).map(|d| d.clone())
}

/// Build a new JavaScript `Decimal` belonging to `st`'s constructor.
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
fn make(env: Env, st: &mut ConstructorState, value: Decimal) -> Value {
    let ctor = env.reference_value(st.ctor);
    let placeholder = env.number(0.0);
    let object = env.construct(ctor, &[placeholder]);
    if let Some(slot) = env.unwrap::<Decimal>(object) {
        *slot = value;
    }
    object
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
// The constructor
// ---------------------------------------------------------------------------

unsafe extern "C" fn construct_decimal(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let (args, this, data) = env.callback_info(info, 1);
    let st = state(data);

    let argument = args.first().copied().unwrap_or_else(|| env.undefined());
    match coerce(env, st, argument) {
        Ok(value) => {
            env.wrap(this, Box::new(value));
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
) -> Option<(Vec<Value>, Decimal, &'static mut ConstructorState)> {
    let (args, this, data) = env.callback_info(info, max_args);
    // SAFETY: `data` is the leaked ConstructorState this method was defined
    // with; see `state`.
    let st = unsafe { state(data) };
    let value = env.unwrap::<Decimal>(this)?.clone();
    Some((args, value, st))
}

/// An argument coerced to a decimal, or a thrown error.
fn argument(env: Env, st: &mut ConstructorState, args: &[Value], index: usize) -> Result<Decimal, Error> {
    let value = args.get(index).copied().unwrap_or_else(|| env.undefined());
    coerce(env, st, value)
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
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let Some((_, x, st)) = receiver(env, info, 0) else {
                return env.undefined();
            };
            let f: fn(&mut Ctx, &Decimal) -> Decimal = $body;
            let result = f(&mut st.ctx, &x);
            make(env, st, result)
        }
    };
}

/// Declares an instance method of shape `(&mut Ctx, &Decimal, &Decimal) -> Decimal`.
macro_rules! binary {
    ($name:ident, $body:expr) => {
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, st)) = receiver(env, info, 1) else {
                return env.undefined();
            };
            let y = match argument(env, st, &args, 0) {
                Ok(y) => y,
                Err(e) => return fail(env, e),
            };
            let f: fn(&mut Ctx, &Decimal, &Decimal) -> Decimal = $body;
            let result = f(&mut st.ctx, &x, &y);
            make(env, st, result)
        }
    };
}

/// Declares an instance method returning a boolean from the value alone.
macro_rules! predicate {
    ($name:ident, $body:expr) => {
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
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
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, st)) = receiver(env, info, 1) else {
                return env.undefined();
            };
            let y = match argument(env, st, &args, 0) {
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
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, st)) = receiver(env, info, 2) else {
                return env.undefined();
            };
            let a = optional_number(env, &args, 0);
            let b = optional_number(env, &args, 1);
            let f: fn(&mut Ctx, &Decimal, Option<f64>, Option<f64>) -> Result<String, Error> = $body;
            match f(&mut st.ctx, &x, a, b) {
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
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let Some((args, x, st)) = receiver(env, info, 2) else {
                return env.undefined();
            };
            let a = optional_number(env, &args, 0);
            let b = optional_number(env, &args, 1);
            let f: fn(&mut Ctx, &Decimal, Option<f64>, Option<f64>) -> Result<Decimal, Error> = $body;
            match f(&mut st.ctx, &x, a, b) {
                Ok(value) => make(env, st, value),
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
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let Some((_, x, st)) = receiver(env, info, 0) else {
                return env.undefined();
            };
            let f: fn(&mut Ctx, &Decimal) -> Result<Decimal, Error> = $body;
            match f(&mut st.ctx, &x) {
                Ok(value) => make(env, st, value),
                Err(e) => fail(env, e),
            }
        }
    };
}

fallible_unary!(m_sin, |ctx, x| decimal_core::trig::sin(ctx, x));
fallible_unary!(m_cos, |ctx, x| decimal_core::trig::cos(ctx, x));
fallible_unary!(m_tan, |ctx, x| decimal_core::trig::tan(ctx, x));

/// `naturalLogarithm`, which can raise `[DecimalError] Precision limit
/// exceeded` when the configured precision outruns the 1025-digit `LN10`
/// constant.
unsafe extern "C" fn m_ln(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, st)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    match decimal_core::elementary::ln(&mut st.ctx, &x) {
        Ok(value) => make(env, st, value),
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
binary!(m_div_to_int, |ctx, x, y| decimal_core::trig::div_to_int(ctx, x, y));

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
stringify!(m_to_exponential, |ctx, x, a, b| ops::to_exponential(ctx, x, a, b));
stringify!(m_to_precision, |ctx, x, a, b| ops::to_precision(ctx, x, a, b));

rounder!(m_to_dp, |ctx, x, a, b| ops::to_decimal_places(ctx, x, a, b));
rounder!(m_to_sd, |ctx, x, a, b| ops::to_significant_digits(ctx, x, a, b));

/// Declares a method that is present but not yet ported.
///
/// These exist because `test/test.js` has no `try`/`catch`: it loads each
/// module with a bare `require`, so a single missing method is not one failing
/// assertion but a `TypeError` that aborts the entire run and scores zero. The
/// surface therefore has to be complete before the pass count means anything.
///
/// Returning NaN is the honest placeholder — it is a value the library already
/// produces for undefined results, so it propagates rather than crashing, and
/// every assertion that depends on it fails and is counted as failing. What it
/// must not do is quietly resemble a correct answer; see the unported-function
/// inventory in DECISIONS.md, which lists each of these with the number of
/// assertions it accounts for.
macro_rules! not_yet_ported {
    ($name:ident) => {
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let (_, this, data) = env.callback_info(info, 2);
            let _ = this;
            // SAFETY: `data` is the leaked ConstructorState for this class.
            let st = unsafe { state(data) };
            make(env, st, Decimal::nan())
        }
    };
}

not_yet_ported!(m_acos);
not_yet_ported!(m_acosh);
not_yet_ported!(m_asin);
not_yet_ported!(m_asinh);
not_yet_ported!(m_atan);
not_yet_ported!(m_atanh);
not_yet_ported!(m_to_binary);
not_yet_ported!(m_to_hex);
not_yet_ported!(m_to_octal);
not_yet_ported!(m_to_fraction);

/// `logarithm`, whose base argument is optional and defaults to 10.
unsafe extern "C" fn m_log(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, st)) = receiver(env, info, 1) else {
        return env.undefined();
    };
    let base = match args.first().copied() {
        None => None,
        Some(v) if matches!(env.type_of(v), JsType::Undefined | JsType::Null) => None,
        Some(v) => match coerce(env, st, v) {
            Ok(b) => Some(b),
            Err(e) => return fail(env, e),
        },
    };
    match decimal_core::power::logarithm(&mut st.ctx, &x, base.as_ref()) {
        Ok(value) => make(env, st, value),
        Err(e) => fail(env, e),
    }
}

/// `toPower`.
unsafe extern "C" fn m_pow(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, st)) = receiver(env, info, 1) else {
        return env.undefined();
    };
    let y = match argument(env, st, &args, 0) {
        Ok(y) => y,
        Err(e) => return fail(env, e),
    };
    match decimal_core::power::to_power(&mut st.ctx, &x, &y) {
        Ok(value) => make(env, st, value),
        Err(e) => fail(env, e),
    }
}

/// `Decimal.log2` and `Decimal.log10`, which are the logarithm with the base
/// supplied rather than taken from the caller.
macro_rules! static_log_base {
    ($name:ident, $base:literal) => {
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let (args, _, data) = env.callback_info(info, 1);
            // SAFETY: `data` is the leaked ConstructorState for this class.
            let st = unsafe { state(data) };
            let first = args.first().copied().unwrap_or_else(|| env.undefined());
            let x = match coerce(env, st, first) {
                Ok(v) => v,
                Err(e) => return fail(env, e),
            };
            let base = Decimal::from_i32($base);
            match decimal_core::power::logarithm(&mut st.ctx, &x, Some(&base)) {
                Ok(value) => make(env, st, value),
                Err(e) => fail(env, e),
            }
        }
    };
}

static_log_base!(s_log2, 2);
static_log_base!(s_log10, 10);

/// `clampedTo`, which takes two bounds and can reject an inverted range.
unsafe extern "C" fn m_clamp(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, st)) = receiver(env, info, 2) else {
        return env.undefined();
    };
    let min = match argument(env, st, &args, 0) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };
    let max = match argument(env, st, &args, 1) {
        Ok(v) => v,
        Err(e) => return fail(env, e),
    };
    match ops::clamp(&mut st.ctx, &x, &min, &max) {
        Ok(value) => make(env, st, value),
        Err(e) => fail(env, e),
    }
}

/// `toNearest`, whose modulus argument is optional.
unsafe extern "C" fn m_to_nearest(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, st)) = receiver(env, info, 2) else {
        return env.undefined();
    };
    let modulus = match args.first().copied() {
        None => None,
        Some(v) if matches!(env.type_of(v), JsType::Undefined | JsType::Null) => None,
        Some(v) => match coerce(env, st, v) {
            Ok(y) => Some(y),
            Err(e) => return fail(env, e),
        },
    };
    let rm = optional_number(env, &args, 1);
    match ops::to_nearest(&mut st.ctx, &x, modulus.as_ref(), rm) {
        Ok(value) => make(env, st, value),
        Err(e) => fail(env, e),
    }
}

/// `comparedTo`, which returns a number rather than a boolean and reports NaN
/// for an unordered pair.
unsafe extern "C" fn m_compared_to(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, st)) = receiver(env, info, 1) else {
        return env.undefined();
    };
    let y = match argument(env, st, &args, 0) {
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

unsafe extern "C" fn m_to_string(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, st)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    env.string(&format::to_string(&x, &st.ctx.cfg))
}

unsafe extern "C" fn m_value_of(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, st)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    env.string(&format::value_of(&x, &st.ctx.cfg))
}

unsafe extern "C" fn m_to_number(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, st)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    let text = format::value_of(&x, &st.ctx.cfg);
    env.number(text.parse::<f64>().unwrap_or(f64::NAN))
}

unsafe extern "C" fn m_decimal_places(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((_, x, _)) = receiver(env, info, 0) else {
        return env.undefined();
    };
    env.number(x.decimal_places().map_or(f64::NAN, |dp| dp as f64))
}

unsafe extern "C" fn m_precision(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let Some((args, x, _)) = receiver(env, info, 1) else {
        return env.undefined();
    };
    if !x.is_finite() {
        return env.number(f64::NAN);
    }
    let mut k = x.significant_digits();
    let include_zeros = args
        .first()
        .and_then(|&v| env.as_bool(v))
        .unwrap_or(false);
    if include_zeros && x.e + 1 > k {
        k = x.e + 1;
    }
    env.number(k as f64)
}

// -- the accessors the test helper reads ------------------------------------

macro_rules! accessor {
    ($name:ident, $body:expr) => {
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let (_, this, _) = env.callback_info(info, 0);
            let Some(x) = env.unwrap::<Decimal>(this) else {
                return env.undefined();
            };
            let f: fn(Env, &Decimal) -> Value = $body;
            f(env, x)
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

/// The eight configurable settings, with the ranges `config` validates them
/// against and the accessors that read and write them.
const SETTINGS: &[(&str, fn(&Config) -> f64, fn(&mut Config, f64))] = &[
    ("precision", |c| c.precision as f64, |c, v| c.precision = v as i64),
    ("rounding", |c| c.rounding as f64, |c, v| c.rounding = v as u8),
    ("modulo", |c| c.modulo as f64, |c, v| c.modulo = v as u8),
    ("toExpNeg", |c| c.to_exp_neg as f64, |c, v| c.to_exp_neg = v as i64),
    ("toExpPos", |c| c.to_exp_pos as f64, |c, v| c.to_exp_pos = v as i64),
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

unsafe extern "C" fn s_config(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let (args, this, data) = env.callback_info(info, 1);
    let st = state(data);

    let object = args.first().copied().unwrap_or_else(|| env.undefined());
    if !matches!(env.type_of(object), JsType::Object | JsType::Function) {
        // A distinct message from the range errors, and the original's exact
        // wording.
        env.throw("[DecimalError] Object expected");
        return env.undefined();
    }

    // `{ defaults: true }` resets every setting first; explicit settings in
    // the same object then apply on top.
    let use_defaults = env.as_bool(env.get_named(object, "defaults")) == Some(true)
        && env.type_of(env.get_named(object, "defaults")) == JsType::Boolean;

    // Validate everything before applying anything, so a rejected call leaves
    // the configuration untouched.
    let mut proposed = if use_defaults {
        Config::default()
    } else {
        st.ctx.cfg
    };

    for (name, _, set) in SETTINGS {
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
            set(&mut proposed, value);
        } else {
            return fail(
                env,
                Error::InvalidArgument(format!("{name}: {}", describe(env, raw))),
            );
        }
    }

    let raw = env.get_named(object, "crypto");
    if env.type_of(raw) != JsType::Undefined {
        // `true`, `false`, `0` and `1` are all accepted.
        let requested = match env.type_of(raw) {
            JsType::Boolean => env.as_bool(raw),
            JsType::Number => match env.as_f64(raw) {
                Some(v) if v == 0.0 => Some(false),
                Some(v) if v == 1.0 => Some(true),
                _ => None,
            },
            _ => None,
        };
        match requested {
            Some(true) => {
                // No cryptographic source is reachable from a native addon
                // without pulling in the JavaScript global, and the original
                // throws exactly this when one is asked for and absent.
                return fail(env, Error::CryptoUnavailable);
            }
            Some(false) => proposed.crypto = false,
            None => {
                return fail(
                    env,
                    Error::InvalidArgument(format!("crypto: {}", describe(env, raw))),
                )
            }
        }
    }

    st.ctx.cfg = proposed;
    this
}

macro_rules! setting_accessor {
    ($getter:ident, $setter:ident, $index:expr) => {
        unsafe extern "C" fn $getter(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let (_, _, data) = env.callback_info(info, 0);
            let st = state(data);
            env.number(SETTINGS[$index].1(&st.ctx.cfg))
        }

        unsafe extern "C" fn $setter(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let (args, _, data) = env.callback_info(info, 1);
            let st = state(data);
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
            set(&mut st.ctx.cfg, value);
            env.undefined()
        }
    };
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
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let (args, _, data) = env.callback_info(info, 3);
            // SAFETY: `data` is the leaked ConstructorState for this class.
            let st = unsafe { state(data) };

            let first = args.first().copied().unwrap_or_else(|| env.undefined());
            let receiver = match coerce(env, st, first) {
                Ok(value) => make(env, st, value),
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

/// A static that is not yet ported, returning NaN. See the inventory in
/// DECISIONS.md.
macro_rules! static_not_yet_ported {
    ($name:ident) => {
        unsafe extern "C" fn $name(
            env: sys::napi_env,
            info: sys::napi_callback_info,
        ) -> sys::napi_value {
            let env = Env(env);
            let (_, _, data) = env.callback_info(info, 0);
            // SAFETY: `data` is the leaked ConstructorState for this class.
            let st = unsafe { state(data) };
            make(env, st, Decimal::nan())
        }
    };
}

static_not_yet_ported!(s_atan2);
static_not_yet_ported!(s_hypot);
static_not_yet_ported!(s_random);
static_not_yet_ported!(s_sum);

/// `Decimal.max` and `Decimal.min`, which take any number of arguments.
///
/// The original returns NaN if *any* argument is NaN, rather than skipping it,
/// so the loop below cannot short-circuit on the first comparison.
unsafe extern "C" fn s_max_or_min<const WANT_GREATER: bool>(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let (args, _, data) = env.callback_info(info, 16);
    // SAFETY: `data` is the leaked ConstructorState for this class.
    let st = unsafe { state(data) };

    let mut best: Option<Decimal> = None;
    for &arg in &args {
        let value = match coerce(env, st, arg) {
            Ok(v) => v,
            Err(e) => return fail(env, e),
        };
        if value.is_nan() {
            return make(env, st, Decimal::nan());
        }
        let replace = match &best {
            None => true,
            Some(current) => match compare(&value, current) {
                Some(core::cmp::Ordering::Greater) => WANT_GREATER,
                Some(core::cmp::Ordering::Less) => !WANT_GREATER,
                _ => false,
            },
        };
        if replace {
            best = Some(value);
        }
    }

    make(env, st, best.unwrap_or_else(Decimal::nan))
}

unsafe extern "C" fn s_max(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    s_max_or_min::<true>(env, info)
}

unsafe extern "C" fn s_min(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    s_max_or_min::<false>(env, info)
}

/// `Decimal.sign`, which returns a plain number and distinguishes the two
/// zeros.
unsafe extern "C" fn s_sign(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let (args, _, data) = env.callback_info(info, 1);
    // SAFETY: `data` is the leaked ConstructorState for this class.
    let st = unsafe { state(data) };

    let first = args.first().copied().unwrap_or_else(|| env.undefined());
    let value = match coerce(env, st, first) {
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

unsafe extern "C" fn s_is_decimal(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let (args, _, _) = env.callback_info(info, 1);
    let is = args
        .first()
        .map(|&v| decimal_of(env, v).is_some())
        .unwrap_or(false);
    env.boolean(is)
}

unsafe extern "C" fn s_clone(
    env: sys::napi_env,
    info: sys::napi_callback_info,
) -> sys::napi_value {
    let env = Env(env);
    let (args, _, data) = env.callback_info(info, 1);
    let parent = state(data);

    // A clone starts from the parent's settings unless `{ defaults: true }`
    // asks for a fresh constructor.
    let mut cfg = parent.ctx.cfg;
    if let Some(&object) = args.first() {
        if env.type_of(object) == JsType::Object
            && env.has_own(object, "defaults")
            && env.as_bool(env.get_named(object, "defaults")) == Some(true)
        {
            cfg = Config::default();
        }
    }

    let class = build_class(env, cfg);

    // Apply any other settings the argument carries, by routing them through
    // the same validation `config` uses.
    if let Some(&object) = args.first() {
        if env.type_of(object) == JsType::Object {
            let config_fn = env.get_named(class, "config");
            let mut result: Value = ptr::null_mut();
            let argv = [object];
            // SAFETY: `class` and `config_fn` are live handles this function
            // just created; `argv` holds one live handle.
            sys::napi_call_function(env.0, class, config_fn, 1, argv.as_ptr(), &mut result);
        }
    }

    class
}

// ---------------------------------------------------------------------------
// Building a class
// ---------------------------------------------------------------------------

/// A property descriptor for a method.
fn method(name: &'static str, cb: sys::napi_callback, data: *mut c_void) -> sys::napi_property_descriptor {
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
fn getter(name: &'static str, cb: sys::napi_callback, data: *mut c_void) -> sys::napi_property_descriptor {
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
fn build_class(env: Env, cfg: Config) -> Value {
    let boxed = Box::new(ConstructorState {
        ctx: Ctx::new(cfg),
        ctor: ptr::null_mut(),
    });
    // Leaked on purpose: see `ConstructorState`.
    let data: *mut c_void = Box::into_raw(boxed).cast();

    let mut properties: Vec<sys::napi_property_descriptor> = Vec::new();

    // Instance methods, including every alias the original defines. The
    // aliases are genuinely the same function object in the original
    // (`P.absoluteValue = P.abs = ...`), and the tests use both spellings.
    let instance: &[(&'static str, sys::napi_callback)] = &[
        ("absoluteValue\0", Some(m_abs)),
        ("abs\0", Some(m_abs)),
        ("negated\0", Some(m_neg)),
        ("neg\0", Some(m_neg)),
        ("ceil\0", Some(m_ceil)),
        ("floor\0", Some(m_floor)),
        ("round\0", Some(m_round)),
        ("truncated\0", Some(m_trunc)),
        ("trunc\0", Some(m_trunc)),
        ("plus\0", Some(m_plus)),
        ("add\0", Some(m_plus)),
        ("minus\0", Some(m_minus)),
        ("sub\0", Some(m_minus)),
        ("times\0", Some(m_times)),
        ("mul\0", Some(m_times)),
        ("dividedBy\0", Some(m_div)),
        ("div\0", Some(m_div)),
        ("dividedToIntegerBy\0", Some(m_div_to_int)),
        ("divToInt\0", Some(m_div_to_int)),
        ("modulo\0", Some(m_mod)),
        ("mod\0", Some(m_mod)),
        ("comparedTo\0", Some(m_compared_to)),
        ("cmp\0", Some(m_compared_to)),
        ("equals\0", Some(m_equals)),
        ("eq\0", Some(m_equals)),
        ("lessThan\0", Some(m_lt)),
        ("lt\0", Some(m_lt)),
        ("lessThanOrEqualTo\0", Some(m_lte)),
        ("lte\0", Some(m_lte)),
        ("greaterThan\0", Some(m_gt)),
        ("gt\0", Some(m_gt)),
        ("greaterThanOrEqualTo\0", Some(m_gte)),
        ("gte\0", Some(m_gte)),
        ("isNaN\0", Some(m_is_nan)),
        ("isFinite\0", Some(m_is_finite)),
        ("isInteger\0", Some(m_is_integer)),
        ("isInt\0", Some(m_is_integer)),
        ("isZero\0", Some(m_is_zero)),
        ("isNegative\0", Some(m_is_negative)),
        ("isNeg\0", Some(m_is_negative)),
        ("isPositive\0", Some(m_is_positive)),
        ("isPos\0", Some(m_is_positive)),
        ("decimalPlaces\0", Some(m_decimal_places)),
        ("dp\0", Some(m_decimal_places)),
        ("precision\0", Some(m_precision)),
        ("sd\0", Some(m_precision)),
        ("toDecimalPlaces\0", Some(m_to_dp)),
        ("toDP\0", Some(m_to_dp)),
        ("toSignificantDigits\0", Some(m_to_sd)),
        ("toSD\0", Some(m_to_sd)),
        ("toFixed\0", Some(m_to_fixed)),
        ("toExponential\0", Some(m_to_exponential)),
        ("toPrecision\0", Some(m_to_precision)),
        ("toNumber\0", Some(m_to_number)),
        ("toString\0", Some(m_to_string)),
        ("valueOf\0", Some(m_value_of)),
        ("toJSON\0", Some(m_value_of)),
        ("clampedTo\0", Some(m_clamp)),
        ("clamp\0", Some(m_clamp)),
        ("toNearest\0", Some(m_to_nearest)),
        // Present so that loading a module cannot abort the run; not yet
        // ported, and each returns NaN. See DECISIONS.md.
        ("inverseCosine\0", Some(m_acos)),
        ("acos\0", Some(m_acos)),
        ("inverseHyperbolicCosine\0", Some(m_acosh)),
        ("acosh\0", Some(m_acosh)),
        ("inverseSine\0", Some(m_asin)),
        ("asin\0", Some(m_asin)),
        ("inverseHyperbolicSine\0", Some(m_asinh)),
        ("asinh\0", Some(m_asinh)),
        ("inverseTangent\0", Some(m_atan)),
        ("atan\0", Some(m_atan)),
        ("inverseHyperbolicTangent\0", Some(m_atanh)),
        ("atanh\0", Some(m_atanh)),
        ("cubeRoot\0", Some(m_cbrt)),
        ("cbrt\0", Some(m_cbrt)),
        ("cosine\0", Some(m_cos)),
        ("cos\0", Some(m_cos)),
        ("hyperbolicCosine\0", Some(m_cosh)),
        ("cosh\0", Some(m_cosh)),
        ("naturalExponential\0", Some(m_exp)),
        ("exp\0", Some(m_exp)),
        ("naturalLogarithm\0", Some(m_ln)),
        ("ln\0", Some(m_ln)),
        ("logarithm\0", Some(m_log)),
        ("log\0", Some(m_log)),
        ("toPower\0", Some(m_pow)),
        ("pow\0", Some(m_pow)),
        ("sine\0", Some(m_sin)),
        ("sin\0", Some(m_sin)),
        ("hyperbolicSine\0", Some(m_sinh)),
        ("sinh\0", Some(m_sinh)),
        ("squareRoot\0", Some(m_sqrt)),
        ("sqrt\0", Some(m_sqrt)),
        ("tangent\0", Some(m_tan)),
        ("tan\0", Some(m_tan)),
        ("hyperbolicTangent\0", Some(m_tanh)),
        ("tanh\0", Some(m_tanh)),
        ("toBinary\0", Some(m_to_binary)),
        ("toHexadecimal\0", Some(m_to_hex)),
        ("toHex\0", Some(m_to_hex)),
        ("toOctal\0", Some(m_to_octal)),
        ("toFraction\0", Some(m_to_fraction)),
    ];
    for &(name, cb) in instance {
        properties.push(method(name, cb, data));
    }

    let accessors: &[(&'static str, sys::napi_callback)] =
        &[("s\0", Some(get_s)), ("e\0", Some(get_e)), ("d\0", Some(get_d))];
    for &(name, cb) in accessors {
        properties.push(getter(name, cb, data));
    }

    // Statics.
    properties.push(static_entry("config\0", Some(s_config), None, None, data));
    properties.push(static_entry("set\0", Some(s_config), None, None, data));
    properties.push(static_entry("clone\0", Some(s_clone), None, None, data));
    properties.push(static_entry("isDecimal\0", Some(s_is_decimal), None, None, data));

    let statics: &[(&'static str, sys::napi_callback)] = &[
        ("abs\0", Some(s_abs)),
        ("acos\0", Some(s_acos)),
        ("acosh\0", Some(s_acosh)),
        ("add\0", Some(s_add)),
        ("asin\0", Some(s_asin)),
        ("asinh\0", Some(s_asinh)),
        ("atan\0", Some(s_atan)),
        ("atanh\0", Some(s_atanh)),
        ("atan2\0", Some(s_atan2)),
        ("cbrt\0", Some(s_cbrt)),
        ("ceil\0", Some(s_ceil)),
        ("clamp\0", Some(s_clamp)),
        ("cos\0", Some(s_cos)),
        ("cosh\0", Some(s_cosh)),
        ("div\0", Some(s_div)),
        ("exp\0", Some(s_exp)),
        ("floor\0", Some(s_floor)),
        ("hypot\0", Some(s_hypot)),
        ("ln\0", Some(s_ln)),
        ("log\0", Some(s_log)),
        ("log2\0", Some(s_log2)),
        ("log10\0", Some(s_log10)),
        ("max\0", Some(s_max)),
        ("min\0", Some(s_min)),
        ("mod\0", Some(s_mod)),
        ("mul\0", Some(s_mul)),
        ("pow\0", Some(s_pow)),
        ("random\0", Some(s_random)),
        ("round\0", Some(s_round)),
        ("sign\0", Some(s_sign)),
        ("sin\0", Some(s_sin)),
        ("sinh\0", Some(s_sinh)),
        ("sqrt\0", Some(s_sqrt)),
        ("sub\0", Some(s_sub)),
        ("sum\0", Some(s_sum)),
        ("tan\0", Some(s_tan)),
        ("tanh\0", Some(s_tanh)),
        ("trunc\0", Some(s_trunc)),
    ];
    for &(name, cb) in statics {
        properties.push(static_entry(name, cb, None, None, data));
    }

    properties.push(static_entry("precision\0", None, Some(get_precision), Some(set_precision), data));
    properties.push(static_entry("rounding\0", None, Some(get_rounding), Some(set_rounding), data));
    properties.push(static_entry("modulo\0", None, Some(get_modulo), Some(set_modulo), data));
    properties.push(static_entry("toExpNeg\0", None, Some(get_to_exp_neg), Some(set_to_exp_neg), data));
    properties.push(static_entry("toExpPos\0", None, Some(get_to_exp_pos), Some(set_to_exp_pos), data));
    properties.push(static_entry("minE\0", None, Some(get_min_e), Some(set_min_e), data));
    properties.push(static_entry("maxE\0", None, Some(get_max_e), Some(set_max_e), data));

    // SAFETY: `properties` lives until after the call returns, and `data` is
    // leaked, so both outlive every invocation of the methods they describe.
    let class = unsafe { define_class(env, "Decimal", Some(construct_decimal), data, &properties) };

    // The rounding-mode constants, and the reference the methods use to build
    // their results.
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

    // SAFETY: `data` is the state just leaked for this class.
    unsafe {
        state(data).ctor = env.create_reference(class);
    }

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
    build_class(Env(env), Config::default())
}
