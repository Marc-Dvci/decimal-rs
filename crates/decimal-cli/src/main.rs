//! `decimal-calc` — a standalone evaluator over `decimal-core`.
//!
//! # Why this binary exists
//!
//! The deliverable of this project is `decimal-core`: pure Rust, no
//! dependencies, no `unsafe`, and no notion that Node exists. The Node addon is
//! *evidence* — it is what lets the original test suite run unmodified — but a
//! port reachable only through an addon would be a plugin for decimal.js's host
//! rather than a library in its own right.
//!
//! So this exists to be run with no Node installed:
//!
//! ```text
//! $ decimal-calc 2 sqrt --precision 40
//! 1.41421356237309504880168872420969807857
//! $ decimal-calc 355 div 113
//! 3.1415929203539823009
//! ```
//!
//! It is deliberately thin. It parses through `parse::from_str` — the same
//! function the addon's string constructor calls — dispatches to `decimal-core`,
//! and renders through the same `format::to_string`, so what it prints is what
//! `Decimal.prototype.toString` prints at the same configuration. It adds no
//! arithmetic of its own: anything computed here would be a behaviour the port
//! has and the original does not.
//!
//! See DECISIONS.md D-01 and D-02.

use decimal_core::{
    arith, elementary, format, inverse, ops, parse, power, roots, trig, Config, Ctx, Decimal,
    Error, EXP_LIMIT, MAX_DIGITS,
};

fn main() {
    match run(std::env::args().skip(1)) {
        Ok(text) => println!("{text}"),
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(1);
        }
    }
}

fn run(args: impl Iterator<Item = String>) -> Result<String, String> {
    let request = parse_arguments(args)?;
    let mut ctx = Ctx::new(request.config);

    let x = value(&mut ctx, &request.x)?;
    let y = match &request.y {
        Some(text) => Some(value(&mut ctx, text)?),
        None => None,
    };

    let result = apply(&mut ctx, &request.operation, &x, y.as_ref())?;

    // An operation that needed a digit array larger than the original's host
    // would build has abandoned itself and set a flag, and is carrying a NaN
    // that means nothing. The addon turns that flag into `RangeError: Invalid
    // array length`; here it is a message and a non-zero exit. Checked once,
    // after the dispatch, because every arm above can raise it. D-10 and D-19.
    if ctx.take_array_limit_exceeded() {
        return Err("Invalid array length".to_string());
    }

    Ok(format::to_string(&result, &ctx.cfg))
}

/// What the user asked for, once the arguments have been understood.
struct Request {
    x: String,
    operation: String,
    y: Option<String>,
    config: Config,
}

/// Parse one operand, reporting the constructor's own message on failure.
fn value(ctx: &mut Ctx, text: &str) -> Result<Decimal, String> {
    parse::from_str(ctx, text).map_err(|error: Error| error.to_string())
}

/// Dispatch one operation.
///
/// Every arm is a call into `decimal-core` and nothing else. The binary
/// operations are matched first so that `decimal-calc 2 div` reports a missing
/// operand by name rather than falling through to "unknown operation".
fn apply(
    ctx: &mut Ctx,
    operation: &str,
    x: &Decimal,
    y: Option<&Decimal>,
) -> Result<Decimal, String> {
    type Binary = fn(&mut Ctx, &Decimal, &Decimal) -> decimal_core::Result<Decimal>;

    let binary: Option<Binary> = match operation {
        "add" | "plus" => Some(|c, a, b| Ok(arith::add(c, a, b))),
        "sub" | "minus" => Some(|c, a, b| Ok(arith::sub(c, a, b))),
        "mul" | "times" => Some(|c, a, b| Ok(arith::mul(c, a, b))),
        "div" => Some(|c, a, b| {
            let rm = c.cfg.rounding;
            Ok(arith::divide(c, a, b, None, rm, false, None))
        }),
        "mod" => Some(|c, a, b| Ok(ops::modulo(c, a, b))),
        "pow" => Some(power::to_power),
        "log" => Some(|c, a, b| power::logarithm(c, a, Some(b))),
        "atan2" => Some(inverse::atan2),
        "hypot" => Some(|c, a, b| Ok(roots::hypot(c, &[a.clone(), b.clone()]))),
        _ => None,
    };

    if let Some(function) = binary {
        let b = y.ok_or_else(|| format!("{operation} needs a second operand"))?;
        return function(ctx, x, b).map_err(|error| error.to_string());
    }

    if y.is_some() {
        return Err(format!("{operation} takes one operand, not two"));
    }

    type Unary = fn(&mut Ctx, &Decimal) -> decimal_core::Result<Decimal>;

    let unary: Unary = match operation {
        "abs" => |c, a| Ok(ops::abs(c, a)),
        "neg" => |c, a| Ok(ops::neg(c, a)),
        "ceil" => |c, a| Ok(ops::ceil(c, a)),
        "floor" => |c, a| Ok(ops::floor(c, a)),
        "round" => |c, a| Ok(ops::round(c, a)),
        "trunc" => |c, a| Ok(ops::trunc(c, a)),
        "sqrt" => |c, a| Ok(roots::sqrt(c, a)),
        "cbrt" => |c, a| Ok(roots::cbrt(c, a)),
        "exp" => |c, a| Ok(elementary::exp(c, a)),
        "sinh" => |c, a| Ok(trig::sinh(c, a)),
        "cosh" => |c, a| Ok(trig::cosh(c, a)),
        "tanh" => |c, a| Ok(trig::tanh(c, a)),
        "ln" => elementary::ln,
        "log10" => |c, a| power::logarithm(c, a, None),
        "sin" => trig::sin,
        "cos" => trig::cos,
        "tan" => trig::tan,
        "asin" => inverse::asin,
        "acos" => inverse::acos,
        "atan" => inverse::atan,
        "asinh" => inverse::asinh,
        "acosh" => inverse::acosh,
        "atanh" => inverse::atanh,
        other => return Err(format!("unknown operation: {other}\n\n{USAGE}")),
    };

    unary(ctx, x).map_err(|error| error.to_string())
}

const USAGE: &str = "\
decimal-calc — arbitrary-precision decimal arithmetic, with no Node present

usage:
  decimal-calc <x> <operation> [<y>] [options]

unary:   abs neg ceil floor round trunc sqrt cbrt exp ln log10
         sin cos tan sinh cosh tanh asin acos atan asinh acosh atanh
binary:  add sub mul div mod pow log atan2 hypot

options:
  --precision N   significant digits, 1 to 1e9        (default 20)
  --rounding N    rounding mode, 0 to 8               (default 4, half up)
  --min-e N       exponent floor                      (default -9e15)
  --max-e N       exponent ceiling                    (default  9e15)

examples:
  decimal-calc 2 sqrt --precision 40
  decimal-calc 355 div 113
  decimal-calc 0x1.8p3 add 1
  decimal-calc 1e-7 ln --precision 30";

/// Read the command line into a [`Request`].
///
/// Positional arguments are taken in order and options may appear anywhere
/// among them, which is what a calculator invoked by hand tends to receive.
fn parse_arguments(args: impl Iterator<Item = String>) -> Result<Request, String> {
    let mut positional: Vec<String> = Vec::new();
    let mut config = Config::default();
    let mut args = args.peekable();

    while let Some(argument) = args.next() {
        // Each option's value is read here and range-checked below, together,
        // so that the checks read as one list against one table of ranges.
        let mut number = |name: &str| -> Result<i64, String> {
            args.next()
                .ok_or_else(|| format!("{name} needs a value"))?
                .parse::<i64>()
                .map_err(|_| format!("{name} needs an integer"))
        };
        match argument.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--precision" => config.precision = number("--precision")?,
            "--rounding" => {
                let value = number("--rounding")?;
                checked("rounding", value, Config::ROUNDING_RANGE)?;
                config.rounding = value as u8;
            }
            "--min-e" => config.min_e = number("--min-e")?,
            "--max-e" => config.max_e = number("--max-e")?,
            _ => positional.push(argument),
        }
    }

    checked("precision", config.precision, Config::PRECISION_RANGE)?;
    checked("minE", config.min_e, Config::MIN_E_RANGE)?;
    checked("maxE", config.max_e, Config::MAX_E_RANGE)?;

    let mut positional = positional.into_iter();
    let x = positional.next().ok_or_else(|| USAGE.to_string())?;
    let operation = positional
        .next()
        .ok_or_else(|| format!("no operation given\n\n{USAGE}"))?;
    let y = positional.next();
    if positional.next().is_some() {
        return Err(format!("too many operands\n\n{USAGE}"));
    }

    Ok(Request {
        x,
        operation,
        y,
        config,
    })
}

/// Range-check one setting, phrasing the failure the way `Decimal.config` does.
///
/// The ranges come from `Config`'s own constants rather than being restated
/// here, so a setting this binary accepts is exactly a setting the library
/// accepts.
fn checked(name: &str, value: i64, (low, high): (i64, i64)) -> Result<(), String> {
    if value < low || value > high {
        return Err(Error::InvalidArgument(format!("{name}: {value}")).to_string());
    }
    Ok(())
}

/// The two limits named in `USAGE` are the library's, not this binary's.
const _: () = assert!(Config::PRECISION_RANGE.1 == MAX_DIGITS);
const _: () = assert!(Config::MAX_E_RANGE.1 == EXP_LIMIT);

#[cfg(test)]
mod tests {
    use super::*;

    fn calc(args: &[&str]) -> Result<String, String> {
        run(args.iter().map(|s| s.to_string()))
    }

    /// Expectations read out of Node against upstream decimal.js at the same
    /// configuration, not out of this implementation.
    #[test]
    fn it_computes_what_the_original_computes() {
        assert_eq!(
            calc(&["355", "div", "113"]).unwrap(),
            "3.1415929203539823009"
        );
        assert_eq!(
            calc(&["2", "sqrt", "--precision", "40"]).unwrap(),
            "1.41421356237309504880168872420969807857"
        );
        assert_eq!(
            calc(&["1e-7", "ln", "--precision", "30"]).unwrap(),
            "-16.1180956509583197881259401828"
        );
        assert_eq!(calc(&["-7", "abs"]).unwrap(), "7");
        assert_eq!(calc(&["2", "pow", "10"]).unwrap(), "1024");
        // The radix forms go through the same constructor path as the addon's.
        assert_eq!(calc(&["0x1.8p3", "add", "1"]).unwrap(), "13");
    }

    #[test]
    fn it_reports_rather_than_guesses() {
        assert!(calc(&["2", "div"]).unwrap_err().contains("second operand"));
        assert!(calc(&["2", "sqrt", "3"])
            .unwrap_err()
            .contains("one operand"));
        assert!(calc(&["2", "frobnicate"]).unwrap_err().contains("unknown"));
        assert!(calc(&["oops", "abs"])
            .unwrap_err()
            .contains("Invalid argument"));
        assert!(calc(&["2", "sqrt", "--precision", "0"])
            .unwrap_err()
            .contains("precision: 0"));
    }

    /// The ceiling `decimal-core` reproduces from the original's host reaches
    /// this binary too, as a message rather than as a meaningless NaN.
    #[test]
    fn the_host_array_ceiling_is_reported_here_as_well() {
        let error = calc(&["1", "div", "3", "--precision", "1000000000"]).unwrap_err();
        assert_eq!(error, "Invalid array length");
    }
}
