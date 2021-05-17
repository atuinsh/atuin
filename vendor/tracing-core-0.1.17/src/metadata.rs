//! Metadata describing trace data.
use super::{callsite, field};
use crate::stdlib::{
    cmp, fmt,
    str::FromStr,
    sync::atomic::{AtomicUsize, Ordering},
};

/// Metadata describing a [span] or [event].
///
/// All spans and events have the following metadata:
/// - A [name], represented as a static string.
/// - A [target], a string that categorizes part of the system where the span
///   or event occurred. The `tracing` macros default to using the module
///   path where the span or event originated as the target, but it may be
///   overridden.
/// - A [verbosity level].
/// - The names of the [fields] defined by the span or event.
/// - Whether the metadata corresponds to a span or event.
///
/// In addition, the following optional metadata describing the source code
/// location where the span or event originated _may_ be provided:
/// - The [file name]
/// - The [line number]
/// - The [module path]
///
/// Metadata is used by [`Subscriber`]s when filtering spans and events, and it
/// may also be used as part of their data payload.
///
/// When created by the `event!` or `span!` macro, the metadata describing a
/// particular event or span is constructed statically and exists as a single
/// static instance. Thus, the overhead of creating the metadata is
/// _significantly_ lower than that of creating the actual span. Therefore,
/// filtering is based on metadata, rather than on the constructed span.
///
/// <div class="information">
///     <div class="tooltip ignore" style="">ⓘ<span class="tooltiptext">Note</span></div>
/// </div>
/// <div class="example-wrap" style="display:inline-block">
/// <pre class="ignore" style="white-space:normal;font:inherit;">
/// <strong>Note</strong>: Although instances of <code>Metadata</code> cannot
/// be compared directly, they provide a method <a href="struct.Metadata.html#method.id">
/// <code>id</code></a>, returning an opaque <a href="../callsite/struct.Identifier.html">
/// callsite identifier</a>  which uniquely identifies the callsite where the metadata
/// originated. This can be used to determine if two <code>Metadata</code> correspond to
/// the same callsite.
/// </pre></div>
///
/// [span]: ../span/index.html
/// [event]: ../event/index.html
/// [name]: #method.name
/// [target]: #method.target
/// [fields]: #method.fields
/// [verbosity level]: #method.level
/// [file name]: #method.file
/// [line number]: #method.line
/// [module path]: #method.module
/// [`Subscriber`]: ../subscriber/trait.Subscriber.html
/// [`id`]: struct.Metadata.html#method.id
/// [callsite identifier]: ../callsite/struct.Identifier.html
pub struct Metadata<'a> {
    /// The name of the span described by this metadata.
    name: &'static str,

    /// The part of the system that the span that this metadata describes
    /// occurred in.
    target: &'a str,

    /// The level of verbosity of the described span.
    level: Level,

    /// The name of the Rust module where the span occurred, or `None` if this
    /// could not be determined.
    module_path: Option<&'a str>,

    /// The name of the source code file where the span occurred, or `None` if
    /// this could not be determined.
    file: Option<&'a str>,

    /// The line number in the source code file where the span occurred, or
    /// `None` if this could not be determined.
    line: Option<u32>,

    /// The names of the key-value fields attached to the described span or
    /// event.
    fields: field::FieldSet,

    /// The kind of the callsite.
    kind: Kind,
}

/// Indicates whether the callsite is a span or event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Kind(KindInner);

/// Describes the level of verbosity of a span or event.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Level(LevelInner);

/// A filter comparable to a verbosity `Level`.
///
/// If a `Level` is considered less than a `LevelFilter`, it should be
/// considered disabled; if greater than or equal to the `LevelFilter`, that
/// level is enabled.
///
/// Note that this is essentially identical to the `Level` type, but with the
/// addition of an `OFF` level that completely disables all trace
/// instrumentation.
#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct LevelFilter(Option<Level>);

/// Indicates that a string could not be parsed to a valid level.
#[derive(Clone, Debug)]
pub struct ParseLevelFilterError(());

static MAX_LEVEL: AtomicUsize = AtomicUsize::new(LevelFilter::OFF_USIZE);

// ===== impl Metadata =====

impl<'a> Metadata<'a> {
    /// Construct new metadata for a span or event, with a name, target, level, field
    /// names, and optional source code location.
    pub const fn new(
        name: &'static str,
        target: &'a str,
        level: Level,
        file: Option<&'a str>,
        line: Option<u32>,
        module_path: Option<&'a str>,
        fields: field::FieldSet,
        kind: Kind,
    ) -> Self {
        Metadata {
            name,
            target,
            level,
            module_path,
            file,
            line,
            fields,
            kind,
        }
    }

    /// Returns the names of the fields on the described span or event.
    pub fn fields(&self) -> &field::FieldSet {
        &self.fields
    }

    /// Returns the level of verbosity of the described span or event.
    pub fn level(&self) -> &Level {
        &self.level
    }

    /// Returns the name of the span.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Returns a string describing the part of the system where the span or
    /// event that this metadata describes occurred.
    ///
    /// Typically, this is the module path, but alternate targets may be set
    /// when spans or events are constructed.
    pub fn target(&self) -> &'a str {
        self.target
    }

    /// Returns the path to the Rust module where the span occurred, or
    /// `None` if the module path is unknown.
    pub fn module_path(&self) -> Option<&'a str> {
        self.module_path
    }

    /// Returns the name of the source code file where the span
    /// occurred, or `None` if the file is unknown
    pub fn file(&self) -> Option<&'a str> {
        self.file
    }

    /// Returns the line number in the source code file where the span
    /// occurred, or `None` if the line number is unknown.
    pub fn line(&self) -> Option<u32> {
        self.line
    }

    /// Returns an opaque `Identifier` that uniquely identifies the callsite
    /// this `Metadata` originated from.
    #[inline]
    pub fn callsite(&self) -> callsite::Identifier {
        self.fields.callsite()
    }

    /// Returns true if the callsite kind is `Event`.
    pub fn is_event(&self) -> bool {
        self.kind.is_event()
    }

    /// Return true if the callsite kind is `Span`.
    pub fn is_span(&self) -> bool {
        self.kind.is_span()
    }
}

impl<'a> fmt::Debug for Metadata<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut meta = f.debug_struct("Metadata");
        meta.field("name", &self.name)
            .field("target", &self.target)
            .field("level", &self.level);

        if let Some(path) = self.module_path() {
            meta.field("module_path", &path);
        }

        match (self.file(), self.line()) {
            (Some(file), Some(line)) => {
                meta.field("location", &format_args!("{}:{}", file, line));
            }
            (Some(file), None) => {
                meta.field("file", &format_args!("{}", file));
            }

            // Note: a line num with no file is a kind of weird case that _probably_ never occurs...
            (None, Some(line)) => {
                meta.field("line", &line);
            }
            (None, None) => {}
        };

        meta.field("fields", &format_args!("{}", self.fields))
            .field("callsite", &self.callsite())
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum KindInner {
    Event,
    Span,
}

impl Kind {
    /// `Event` callsite
    pub const EVENT: Kind = Kind(KindInner::Event);

    /// `Span` callsite
    pub const SPAN: Kind = Kind(KindInner::Span);

    /// Return true if the callsite kind is `Span`
    pub fn is_span(&self) -> bool {
        match self {
            Kind(KindInner::Span) => true,
            _ => false,
        }
    }

    /// Return true if the callsite kind is `Event`
    pub fn is_event(&self) -> bool {
        match self {
            Kind(KindInner::Event) => true,
            _ => false,
        }
    }
}

// ===== impl Level =====

impl Level {
    /// The "error" level.
    ///
    /// Designates very serious errors.
    pub const ERROR: Level = Level(LevelInner::Error);
    /// The "warn" level.
    ///
    /// Designates hazardous situations.
    pub const WARN: Level = Level(LevelInner::Warn);
    /// The "info" level.
    ///
    /// Designates useful information.
    pub const INFO: Level = Level(LevelInner::Info);
    /// The "debug" level.
    ///
    /// Designates lower priority information.
    pub const DEBUG: Level = Level(LevelInner::Debug);
    /// The "trace" level.
    ///
    /// Designates very low priority, often extremely verbose, information.
    pub const TRACE: Level = Level(LevelInner::Trace);
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Level::TRACE => f.pad("TRACE"),
            Level::DEBUG => f.pad("DEBUG"),
            Level::INFO => f.pad("INFO"),
            Level::WARN => f.pad("WARN"),
            Level::ERROR => f.pad("ERROR"),
        }
    }
}

#[cfg(feature = "std")]
#[cfg_attr(docsrs, doc(cfg(feature = "std")))]
impl crate::stdlib::error::Error for ParseLevelError {}

impl FromStr for Level {
    type Err = ParseLevelError;
    fn from_str(s: &str) -> Result<Self, ParseLevelError> {
        s.parse::<usize>()
            .map_err(|_| ParseLevelError { _p: () })
            .and_then(|num| match num {
                1 => Ok(Level::ERROR),
                2 => Ok(Level::WARN),
                3 => Ok(Level::INFO),
                4 => Ok(Level::DEBUG),
                5 => Ok(Level::TRACE),
                _ => Err(ParseLevelError { _p: () }),
            })
            .or_else(|_| match s {
                s if s.eq_ignore_ascii_case("error") => Ok(Level::ERROR),
                s if s.eq_ignore_ascii_case("warn") => Ok(Level::WARN),
                s if s.eq_ignore_ascii_case("info") => Ok(Level::INFO),
                s if s.eq_ignore_ascii_case("debug") => Ok(Level::DEBUG),
                s if s.eq_ignore_ascii_case("trace") => Ok(Level::TRACE),
                _ => Err(ParseLevelError { _p: () }),
            })
    }
}

#[repr(usize)]
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
enum LevelInner {
    /// The "trace" level.
    ///
    /// Designates very low priority, often extremely verbose, information.
    Trace = 0,
    /// The "debug" level.
    ///
    /// Designates lower priority information.
    Debug = 1,
    /// The "info" level.
    ///
    /// Designates useful information.
    Info = 2,
    /// The "warn" level.
    ///
    /// Designates hazardous situations.
    Warn = 3,
    /// The "error" level.
    ///
    /// Designates very serious errors.
    Error = 4,
}

// === impl LevelFilter ===

impl From<Level> for LevelFilter {
    #[inline]
    fn from(level: Level) -> Self {
        Self::from_level(level)
    }
}

impl From<Option<Level>> for LevelFilter {
    #[inline]
    fn from(level: Option<Level>) -> Self {
        Self(level)
    }
}

impl Into<Option<Level>> for LevelFilter {
    #[inline]
    fn into(self) -> Option<Level> {
        self.into_level()
    }
}

impl LevelFilter {
    /// The "off" level.
    ///
    /// Designates that trace instrumentation should be completely disabled.
    pub const OFF: LevelFilter = LevelFilter(None);
    /// The "error" level.
    ///
    /// Designates very serious errors.
    pub const ERROR: LevelFilter = LevelFilter::from_level(Level::ERROR);
    /// The "warn" level.
    ///
    /// Designates hazardous situations.
    pub const WARN: LevelFilter = LevelFilter::from_level(Level::WARN);
    /// The "info" level.
    ///
    /// Designates useful information.
    pub const INFO: LevelFilter = LevelFilter::from_level(Level::INFO);
    /// The "debug" level.
    ///
    /// Designates lower priority information.
    pub const DEBUG: LevelFilter = LevelFilter::from_level(Level::DEBUG);
    /// The "trace" level.
    ///
    /// Designates very low priority, often extremely verbose, information.
    pub const TRACE: LevelFilter = LevelFilter(Some(Level::TRACE));

    /// Returns a `LevelFilter` that enables spans and events with verbosity up
    /// to and including `level`.
    pub const fn from_level(level: Level) -> Self {
        Self(Some(level))
    }

    /// Returns the most verbose [`Level`] that this filter accepts, or `None`
    /// if it is [`OFF`].
    ///
    /// [`Level`]: ../struct.Level.html
    /// [`OFF`]: #associatedconstant.OFF
    pub const fn into_level(self) -> Option<Level> {
        self.0
    }

    // These consts are necessary because `as` casts are not allowed as
    // match patterns.
    const ERROR_USIZE: usize = LevelInner::Error as usize;
    const WARN_USIZE: usize = LevelInner::Warn as usize;
    const INFO_USIZE: usize = LevelInner::Info as usize;
    const DEBUG_USIZE: usize = LevelInner::Debug as usize;
    const TRACE_USIZE: usize = LevelInner::Trace as usize;
    // Using the value of the last variant + 1 ensures that we match the value
    // for `Option::None` as selected by the niche optimization for
    // `LevelFilter`. If this is the case, converting a `usize` value into a
    // `LevelFilter` (in `LevelFilter::current`) will be an identity conversion,
    // rather than generating a lookup table.
    const OFF_USIZE: usize = LevelInner::Error as usize + 1;

    /// Returns a `LevelFilter` that matches the most verbose [`Level`] that any
    /// currently active [`Subscriber`] will enable.
    ///
    /// User code should treat this as a *hint*. If a given span or event has a
    /// level *higher* than the returned `LevelFilter`, it will not be enabled.
    /// However, if the level is less than or equal to this value, the span or
    /// event is *not* guaranteed to be enabled; the subscriber will still
    /// filter each callsite individually.
    ///
    /// Therefore, comparing a given span or event's level to the returned
    /// `LevelFilter` **can** be used for determining if something is
    /// *disabled*, but **should not** be used for determining if something is
    /// *enabled*.`
    ///
    /// [`Level`]: ../struct.Level.html
    /// [`Subscriber`]: ../../trait.Subscriber.html
    #[inline(always)]
    pub fn current() -> Self {
        match MAX_LEVEL.load(Ordering::Relaxed) {
            Self::ERROR_USIZE => Self::ERROR,
            Self::WARN_USIZE => Self::WARN,
            Self::INFO_USIZE => Self::INFO,
            Self::DEBUG_USIZE => Self::DEBUG,
            Self::TRACE_USIZE => Self::TRACE,
            Self::OFF_USIZE => Self::OFF,
            #[cfg(debug_assertions)]
            unknown => unreachable!(
                "/!\\ `LevelFilter` representation seems to have changed! /!\\ \n\
                This is a bug (and it's pretty bad). Please contact the `tracing` \
                maintainers. Thank you and I'm sorry.\n \
                The offending repr was: {:?}",
                unknown,
            ),
            #[cfg(not(debug_assertions))]
            _ => unsafe {
                // Using `unreachable_unchecked` here (rather than
                // `unreachable!()`) is necessary to ensure that rustc generates
                // an identity conversion from integer -> discriminant, rather
                // than generating a lookup table. We want to ensure this
                // function is a single `mov` instruction (on x86) if at all
                // possible, because it is called *every* time a span/event
                // callsite is hit; and it is (potentially) the only code in the
                // hottest path for skipping a majority of callsites when level
                // filtering is in use.
                //
                // safety: This branch is only truly unreachable if we guarantee
                // that no values other than the possible enum discriminants
                // will *ever* be present. The `AtomicUsize` is initialized to
                // the `OFF` value. It is only set by the `set_max` function,
                // which takes a `LevelFilter` as a parameter. This restricts
                // the inputs to `set_max` to the set of valid discriminants.
                // Therefore, **as long as `MAX_VALUE` is only ever set by
                // `set_max`**, this is safe.
                crate::stdlib::hint::unreachable_unchecked()
            },
        }
    }

    pub(crate) fn set_max(LevelFilter(level): LevelFilter) {
        let val = match level {
            Some(Level(level)) => level as usize,
            None => Self::OFF_USIZE,
        };

        // using an AcqRel swap ensures an ordered relationship of writes to the
        // max level.
        MAX_LEVEL.swap(val, Ordering::AcqRel);
    }
}

impl fmt::Display for LevelFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            LevelFilter::OFF => f.pad("off"),
            LevelFilter::ERROR => f.pad("error"),
            LevelFilter::WARN => f.pad("warn"),
            LevelFilter::INFO => f.pad("info"),
            LevelFilter::DEBUG => f.pad("debug"),
            LevelFilter::TRACE => f.pad("trace"),
        }
    }
}

impl fmt::Debug for LevelFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            LevelFilter::OFF => f.pad("LevelFilter::OFF"),
            LevelFilter::ERROR => f.pad("LevelFilter::ERROR"),
            LevelFilter::WARN => f.pad("LevelFilter::WARN"),
            LevelFilter::INFO => f.pad("LevelFilter::INFO"),
            LevelFilter::DEBUG => f.pad("LevelFilter::DEBUG"),
            LevelFilter::TRACE => f.pad("LevelFilter::TRACE"),
        }
    }
}

impl FromStr for LevelFilter {
    type Err = ParseLevelFilterError;
    fn from_str(from: &str) -> Result<Self, Self::Err> {
        from.parse::<usize>()
            .ok()
            .and_then(|num| match num {
                0 => Some(LevelFilter::OFF),
                1 => Some(LevelFilter::ERROR),
                2 => Some(LevelFilter::WARN),
                3 => Some(LevelFilter::INFO),
                4 => Some(LevelFilter::DEBUG),
                5 => Some(LevelFilter::TRACE),
                _ => None,
            })
            .or_else(|| match from {
                "" => Some(LevelFilter::ERROR),
                s if s.eq_ignore_ascii_case("error") => Some(LevelFilter::ERROR),
                s if s.eq_ignore_ascii_case("warn") => Some(LevelFilter::WARN),
                s if s.eq_ignore_ascii_case("info") => Some(LevelFilter::INFO),
                s if s.eq_ignore_ascii_case("debug") => Some(LevelFilter::DEBUG),
                s if s.eq_ignore_ascii_case("trace") => Some(LevelFilter::TRACE),
                s if s.eq_ignore_ascii_case("off") => Some(LevelFilter::OFF),
                _ => None,
            })
            .ok_or_else(|| ParseLevelFilterError(()))
    }
}

/// Returned if parsing a `Level` fails.
#[derive(Debug)]
pub struct ParseLevelError {
    _p: (),
}

impl fmt::Display for ParseLevelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(
            "error parsing level: expected one of \"error\", \"warn\", \
             \"info\", \"debug\", \"trace\", or a number 1-5",
        )
    }
}

impl fmt::Display for ParseLevelFilterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(
            "error parsing level filter: expected one of \"off\", \"error\", \
            \"warn\", \"info\", \"debug\", \"trace\", or a number 0-5",
        )
    }
}

#[cfg(feature = "std")]
impl std::error::Error for ParseLevelFilterError {}

// ==== Level and LevelFilter comparisons ====

// /!\ BIG, IMPORTANT WARNING /!\
// Do NOT mess with these implementations! They are hand-written for a reason!
//
// Since comparing `Level`s and `LevelFilter`s happens in a *very* hot path
// (potentially, every time a span or event macro is hit, regardless of whether
// or not is enabled), we *need* to ensure that these comparisons are as fast as
// possible. Therefore, we have some requirements:
//
// 1. We want to do our best to ensure that rustc will generate integer-integer
//    comparisons wherever possible.
//
//    The derived `Ord`/`PartialOrd` impls for `LevelFilter` will not do this,
//    because `LevelFilter`s are represented by `Option<Level>`, rather than as
//    a separate `#[repr(usize)]` enum. This was (unfortunately) necessary for
//    backwards-compatibility reasons, as the  `tracing` crate's original
//    version of `LevelFilter` defined `const fn` conversions between `Level`s
//    and `LevelFilter`, so we're stuck with the `Option<Level>` repr.
//    Therefore, we need hand-written `PartialOrd` impls that cast both sides of
//    the comparison to `usize`s, to force the compiler to generate integer
//    compares.
//
// 2. The hottest `Level`/`LevelFilter` comparison, the one that happens every
//    time a callsite is hit, occurs *within the `tracing` crate's macros*.
//    This means that the comparison is happening *inside* a crate that
//    *depends* on `tracing-core`, not in `tracing-core` itself. The compiler
//    will only inline function calls across crate boundaries if the called
//    function is annotated with an `#[inline]` attribute, and we *definitely*
//    want the comparison functions to be inlined: as previously mentioned, they
//    should compile down to a single integer comparison on release builds, and
//    it seems really sad to push an entire stack frame to call a function
//    consisting of one `cmp` instruction!
//
//    Therefore, we need to ensure that all the comparison methods have
//    `#[inline]` or `#[inline(always)]` attributes. It's not sufficient to just
//    add the attribute to `partial_cmp` in a manual implementation of the
//    trait, since it's the comparison operators (`lt`, `le`, `gt`, and `ge`)
//    that will actually be *used*, and the default implementation of *those*
//    methods, which calls `partial_cmp`, does not have an inline annotation.
//
// 3. We need the comparisons to be inverted. The discriminants for the
//    `LevelInner` enum are assigned in "backwards" order, with `TRACE` having
//    the *lowest* value. However, we want `TRACE` to compare greater-than all
//    other levels.
//
//    Why are the numeric values inverted? In order to ensure that `LevelFilter`
//    (which, as previously mentioned, *has* to be internally represented by an
//    `Option<Level>`) compiles down to a single integer value. This is
//    necessary for storing the global max in an `AtomicUsize`, and for ensuring
//    that we use fast integer-integer comparisons, as mentioned previously. In
//    order to ensure this, we exploit the niche optimization. The niche
//    optimization for `Option<{enum with a numeric repr}>` will choose
//    `(HIGHEST_DISCRIMINANT_VALUE + 1)` as the representation for `None`.
//    Therefore, the integer representation of `LevelFilter::OFF` (which is
//    `None`) will be the number 5. `OFF` must compare higher than every other
//    level in order for it to filter as expected. Since we want to use a single
//    `cmp` instruction, we can't special-case the integer value of `OFF` to
//    compare higher, as that will generate more code. Instead, we need it to be
//    on one end of the enum, with `ERROR` on the opposite end, so we assign the
//    value 0 to `ERROR`.
//
//    This *does* mean that when parsing `LevelFilter`s or `Level`s from
//    `String`s, the integer values are inverted, but that doesn't happen in a
//    hot path.
//
//    Note that we manually invert the comparisons by swapping the left-hand and
//    right-hand side. Using `Ordering::reverse` generates significantly worse
//    code (per Matt Godbolt's Compiler Explorer).
//
// Anyway, that's a brief history of why this code is the way it is. Don't
// change it unless you know what you're doing.

impl PartialEq<LevelFilter> for Level {
    #[inline(always)]
    fn eq(&self, other: &LevelFilter) -> bool {
        self.0 as usize == filter_as_usize(&other.0)
    }
}

impl PartialOrd for Level {
    #[inline(always)]
    fn partial_cmp(&self, other: &Level) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }

    #[inline(always)]
    fn lt(&self, other: &Level) -> bool {
        (other.0 as usize) < (self.0 as usize)
    }

    #[inline(always)]
    fn le(&self, other: &Level) -> bool {
        (other.0 as usize) <= (self.0 as usize)
    }

    #[inline(always)]
    fn gt(&self, other: &Level) -> bool {
        (other.0 as usize) > (self.0 as usize)
    }

    #[inline(always)]
    fn ge(&self, other: &Level) -> bool {
        (other.0 as usize) >= (self.0 as usize)
    }
}

impl Ord for Level {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        (other.0 as usize).cmp(&(self.0 as usize))
    }
}

impl PartialOrd<LevelFilter> for Level {
    #[inline(always)]
    fn partial_cmp(&self, other: &LevelFilter) -> Option<cmp::Ordering> {
        Some(filter_as_usize(&other.0).cmp(&(self.0 as usize)))
    }

    #[inline(always)]
    fn lt(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) < (self.0 as usize)
    }

    #[inline(always)]
    fn le(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) <= (self.0 as usize)
    }

    #[inline(always)]
    fn gt(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) > (self.0 as usize)
    }

    #[inline(always)]
    fn ge(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) >= (self.0 as usize)
    }
}

#[inline(always)]
fn filter_as_usize(x: &Option<Level>) -> usize {
    match x {
        Some(Level(f)) => *f as usize,
        None => LevelFilter::OFF_USIZE,
    }
}

impl PartialEq<Level> for LevelFilter {
    #[inline(always)]
    fn eq(&self, other: &Level) -> bool {
        filter_as_usize(&self.0) == other.0 as usize
    }
}

impl PartialOrd for LevelFilter {
    #[inline(always)]
    fn partial_cmp(&self, other: &LevelFilter) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }

    #[inline(always)]
    fn lt(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) < filter_as_usize(&self.0)
    }

    #[inline(always)]
    fn le(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) <= filter_as_usize(&self.0)
    }

    #[inline(always)]
    fn gt(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) > filter_as_usize(&self.0)
    }

    #[inline(always)]
    fn ge(&self, other: &LevelFilter) -> bool {
        filter_as_usize(&other.0) >= filter_as_usize(&self.0)
    }
}

impl Ord for LevelFilter {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        filter_as_usize(&other.0).cmp(&filter_as_usize(&self.0))
    }
}

impl PartialOrd<Level> for LevelFilter {
    #[inline(always)]
    fn partial_cmp(&self, other: &Level) -> Option<cmp::Ordering> {
        Some((other.0 as usize).cmp(&filter_as_usize(&self.0)))
    }

    #[inline(always)]
    fn lt(&self, other: &Level) -> bool {
        (other.0 as usize) < filter_as_usize(&self.0)
    }

    #[inline(always)]
    fn le(&self, other: &Level) -> bool {
        (other.0 as usize) <= filter_as_usize(&self.0)
    }

    #[inline(always)]
    fn gt(&self, other: &Level) -> bool {
        (other.0 as usize) > filter_as_usize(&self.0)
    }

    #[inline(always)]
    fn ge(&self, other: &Level) -> bool {
        (other.0 as usize) >= filter_as_usize(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::mem;

    #[test]
    fn level_from_str() {
        assert_eq!("error".parse::<Level>().unwrap(), Level::ERROR);
        assert_eq!("4".parse::<Level>().unwrap(), Level::DEBUG);
        assert!("0".parse::<Level>().is_err())
    }

    #[test]
    fn filter_level_conversion() {
        let mapping = [
            (LevelFilter::OFF, None),
            (LevelFilter::ERROR, Some(Level::ERROR)),
            (LevelFilter::WARN, Some(Level::WARN)),
            (LevelFilter::INFO, Some(Level::INFO)),
            (LevelFilter::DEBUG, Some(Level::DEBUG)),
            (LevelFilter::TRACE, Some(Level::TRACE)),
        ];
        for (filter, level) in mapping.iter() {
            assert_eq!(filter.clone().into_level(), *level);
            match level {
                Some(level) => {
                    let actual: LevelFilter = level.clone().into();
                    assert_eq!(actual, *filter);
                }
                None => {
                    let actual: LevelFilter = None.into();
                    assert_eq!(actual, *filter);
                }
            }
        }
    }

    #[test]
    fn level_filter_is_usize_sized() {
        assert_eq!(
            mem::size_of::<LevelFilter>(),
            mem::size_of::<usize>(),
            "`LevelFilter` is no longer `usize`-sized! global MAX_LEVEL may now be invalid!"
        )
    }

    #[test]
    fn level_filter_reprs() {
        let mapping = [
            (LevelFilter::OFF, LevelInner::Error as usize + 1),
            (LevelFilter::ERROR, LevelInner::Error as usize),
            (LevelFilter::WARN, LevelInner::Warn as usize),
            (LevelFilter::INFO, LevelInner::Info as usize),
            (LevelFilter::DEBUG, LevelInner::Debug as usize),
            (LevelFilter::TRACE, LevelInner::Trace as usize),
        ];
        for &(filter, expected) in &mapping {
            let repr = unsafe {
                // safety: The entire purpose of this test is to assert that the
                // actual repr matches what we expect it to be --- we're testing
                // that *other* unsafe code is sound using the transmuted value.
                // We're not going to do anything with it that might be unsound.
                mem::transmute::<LevelFilter, usize>(filter)
            };
            assert_eq!(expected, repr, "repr changed for {:?}", filter)
        }
    }
}
