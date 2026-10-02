//! The one error type every fallible operation in the kit returns.

use core::fmt;

/// The single error type of the `modhash` kit.
///
/// Every fallible operation across the workspace returns
/// [`Result<T>`](crate::Result), which uses this type, so a caller that
/// handles errors once handles all of them. Each variant carries a
/// `&'static str` naming the field or structure at fault, so a message
/// names a thing rather than saying "invalid input".
///
/// [`Display`](core::fmt::Display) never allocates and never runs the
/// formatting machinery: it concatenates the literal prefix with the
/// `what` field through [`Formatter::write_str`](core::fmt::Formatter::write_str)
/// alone. The numeric payloads (`needed`, `found`, `limit`) stay
/// reachable through `Debug`, which derives normally.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// Input ended before a structure that the format guarantees.
    Truncated {
        /// What was being read, e.g. `"idat chunk"`.
        what: &'static str,
        /// How many bytes the structure needs.
        needed: usize,
        /// How many bytes were actually there.
        found: usize,
    },
    /// A magic number, signature or fixed field did not match.
    InvalidMagic {
        /// The field that did not match, e.g. `"PNG signature"`.
        what: &'static str,
    },
    /// A structurally valid but semantically impossible value: a zero
    /// denominator, a declared length that cannot fit, a table index past
    /// the end of the table.
    BadValue(&'static str),
    /// A real format variant this crate deliberately does not implement.
    /// This is not a failure to parse; it is a refusal to guess.
    Unsupported(&'static str),
    /// An allocation the format could request but that exceeds a
    /// configured ceiling. Always a refusal, never an OOM.
    TooLarge {
        /// The thing whose size exceeded the ceiling.
        what: &'static str,
        /// The configured ceiling, in bytes.
        limit: usize,
    },
}

impl Error {
    /// Builds [`Error::Truncated`] without spelling the struct literal.
    pub fn truncated(what: &'static str, needed: usize, found: usize) -> Self {
        Self::Truncated { what, needed, found }
    }

    /// Builds [`Error::TooLarge`] without spelling the struct literal.
    pub fn too_large(what: &'static str, limit: usize) -> Self {
        Self::TooLarge { what, limit }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated { what, .. } => {
                f.write_str("truncated: ")?;
                f.write_str(what)
            }
            Error::InvalidMagic { what } => {
                f.write_str("invalid magic: ")?;
                f.write_str(what)
            }
            Error::BadValue(what) => {
                f.write_str("bad value: ")?;
                f.write_str(what)
            }
            Error::Unsupported(what) => {
                f.write_str("unsupported: ")?;
                f.write_str(what)
            }
            Error::TooLarge { what, .. } => {
                f.write_str("too large: ")?;
                f.write_str(what)
            }
        }
    }
}


/// The result type used by every fallible operation in the kit. The
/// error type defaults to [`Error`], which is what the kit's own APIs
/// return; the parameter stays open so host code can reuse the alias
/// with its own error type.
pub type Result<T, E = Error> = core::result::Result<T, E>;
