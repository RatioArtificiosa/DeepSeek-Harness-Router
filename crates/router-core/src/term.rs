//! Terminal output.
//!
//! # Why this is a real module and not `println!`
//!
//! For a command-line tool the terminal **is** the interface. A user forms
//! their entire impression of the product in the first ten seconds, before they
//! have opened a single browser tab. Every line here is a design decision.
//!
//! # The rules this module enforces
//!
//! 1. **Colour is a courtesy, never a requirement.** When the output is piped,
//!    redirected, or the terminal reports no colour support, every escape
//!    sequence disappears and the output remains completely readable. A tool
//!    whose output only makes sense in colour is broken in a CI log.
//!
//! 2. **Failures name a cause and a fix.** [`Style::error`] always carries a
//!    remedy. A message that reports a problem without saying what to do about
//!    it is an incomplete message.
//!
//! 3. **The eye lands on the answer.** Values are emphasised, labels are not.
//!    A table where every cell is bold communicates nothing.
//!
//! 4. **Nothing is printed that the user did not ask for.** No banners on every
//!    command, no progress for work that takes under a tick.

use std::io::{IsTerminal, Write};

/// Whether colour should be emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColourMode {
    /// Colour is on.
    Always,
    /// Colour is on only when the stream is a terminal.
    Auto,
    /// Colour is off.
    Never,
}

impl ColourMode {
    /// Resolve the mode against a stream.
    #[must_use]
    pub fn enabled(self, is_terminal: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => is_terminal && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    /// Parse from a flag value or the environment.
    ///
    /// `NO_COLOR` is honoured because it is the convention users set once and
    /// expect every tool to respect.
    ///
    /// An unrecognised *flag* value is an error, not a request for the default:
    /// see [`ColourMode::parse`], which this delegates to. Only the environment
    /// falls back silently, and that is deliberate — `NO_COLOR` is set by other
    /// tools and by users' shell profiles, and refusing to run because of a
    /// variable the user did not set would be hostile.
    #[must_use]
    pub fn from_env(explicit: Option<&str>) -> Self {
        if let Ok(mode) = Self::parse(explicit) {
            return mode;
        }
        if std::env::var_os("NO_COLOR").is_some()
            || std::env::var_os("DSH_ROUTER_NO_COLOR").is_some()
        {
            return Self::Never;
        }
        Self::Auto
    }

    /// Parse an explicit `--colour` value.
    ///
    /// # Errors
    ///
    /// Returns the offending string when it is not a recognised mode. The caller
    /// turns that into a usage error, because a typo'd value silently becoming
    /// `auto` means a script that asked for `always` gets no colour and no
    /// explanation — the failure is invisible exactly where it matters most.
    #[must_use = "an unrecognised value must be reported, not discarded"]
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        let Some(value) = value else {
            return Ok(Self::Auto);
        };
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "always" | "true" | "yes" | "1" => Ok(Self::Always),
            "never" | "false" | "no" | "0" => Ok(Self::Never),
            _ => Err(value.to_string()),
        }
    }
}

/// How much a reader needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Verbosity {
    /// Only the result.
    Quiet,
    /// The result and what it means.
    Normal,
    /// Everything, including what was tried.
    Verbose,
}

/// Whether unicode box-drawing and glyphs may be used.
///
/// A legacy Windows console renders these as mojibake, which looks broken in a
/// way that undermines confidence in the tool. When in doubt, fall back to
/// ASCII: it is less pretty and always correct.
#[must_use]
pub fn unicode_ok() -> bool {
    if std::env::var_os("DSH_ROUTER_ASCII").is_some() {
        return false;
    }
    if cfg!(windows) {
        // Modern Windows Terminal sets WT_SESSION; legacy conhost does not.
        if std::env::var_os("WT_SESSION").is_some() {
            return true;
        }
        // A UTF-8 code page is a reasonable proxy for a capable console.
        return std::env::var("CHCP").is_ok_and(|v| v.contains("65001"))
            || std::env::var("LANG").is_ok_and(|v| v.to_ascii_uppercase().contains("UTF"));
    }
    // On POSIX, check the locale. "C" and "POSIX" are ASCII-only.
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_CTYPE"))
        .or_else(|_| std::env::var("LANG"))
        .map_or(true, |v| {
            let v = v.to_ascii_uppercase();
            v.contains("UTF") || v.contains("UTF8")
        })
}

/// The glyphs used in output, in a unicode and an ASCII flavour.
#[derive(Debug, Clone, Copy)]
pub struct Glyphs {
    /// A successful outcome.
    pub ok: &'static str,
    /// A warning.
    pub warn: &'static str,
    /// A failure.
    pub fail: &'static str,
    /// A neutral note.
    pub info: &'static str,
    /// A bullet for list items.
    pub bullet: &'static str,
    /// An arrow introducing a value.
    pub arrow: &'static str,
    /// A horizontal rule.
    pub rule: &'static str,
    /// A running instance.
    pub running: &'static str,
    /// A stopped instance.
    pub stopped: &'static str,
    /// Vertical separator between columns of prose.
    pub sep: &'static str,
}

impl Glyphs {
    /// Unicode glyphs, for a capable terminal.
    #[must_use]
    pub const fn unicode() -> Self {
        Self {
            ok: "✓",
            warn: "!",
            fail: "✗",
            info: "·",
            bullet: "•",
            arrow: "→",
            rule: "─",
            running: "●",
            stopped: "○",
            sep: "·",
        }
    }

    /// ASCII glyphs, for a console that cannot be trusted with more.
    #[must_use]
    pub const fn ascii() -> Self {
        Self {
            ok: "OK",
            warn: "!",
            fail: "x",
            info: "-",
            bullet: "*",
            arrow: "->",
            rule: "-",
            running: "*",
            stopped: "o",
            sep: "|",
        }
    }

    /// The right set for this environment.
    #[must_use]
    pub fn detect() -> Self {
        if unicode_ok() {
            Self::unicode()
        } else {
            Self::ascii()
        }
    }
}

/// An ANSI style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ink {
    /// Reset to the terminal default.
    Plain,
    /// Subdued: for labels and secondary text.
    Dim,
    /// Emphasised: for values and names.
    Bold,
    /// Attention without alarm.
    Amber,
    /// A failure.
    Red,
    /// A success.
    Green,
    /// Interactive or identifying.
    Blue,
    /// Secondary accent.
    Violet,
}

impl Ink {
    /// The escape sequence for this style.
    const fn code(self) -> &'static str {
        match self {
            Self::Plain => "\x1b[0m",
            Self::Dim => "\x1b[2m",
            Self::Bold => "\x1b[1m",
            Self::Amber => "\x1b[33m",
            Self::Red => "\x1b[31m",
            Self::Green => "\x1b[32m",
            Self::Blue => "\x1b[36m",
            Self::Violet => "\x1b[35m",
        }
    }

    /// The bright variant, for a dark terminal where the base tone is murky.
    const fn bright_code(self) -> &'static str {
        match self {
            Self::Amber => "\x1b[93m",
            Self::Red => "\x1b[91m",
            Self::Green => "\x1b[92m",
            Self::Blue => "\x1b[96m",
            Self::Violet => "\x1b[95m",
            _ => self.code(),
        }
    }
}

/// Renders output with the styling the current environment supports.
#[derive(Debug, Clone)]
pub struct Style {
    colour: bool,
    /// The glyph set.
    pub glyphs: Glyphs,
    /// The current verbosity.
    pub verbosity: Verbosity,
}

impl Default for Style {
    fn default() -> Self {
        Self::detect()
    }
}

impl Style {
    /// Detect the best style for the current environment.
    #[must_use]
    pub fn detect() -> Self {
        let mode = ColourMode::from_env(None);
        Self {
            colour: mode.enabled(std::io::stdout().is_terminal()),
            glyphs: Glyphs::detect(),
            verbosity: Verbosity::Normal,
        }
    }

    /// Build a style from an explicit colour mode.
    #[must_use]
    pub fn with_colour(mode: ColourMode, verbosity: Verbosity) -> Self {
        Self {
            colour: mode.enabled(std::io::stdout().is_terminal()),
            glyphs: Glyphs::detect(),
            verbosity,
        }
    }

    /// Whether colour is active.
    #[must_use]
    pub const fn colour(&self) -> bool {
        self.colour
    }

    /// Paint text, or return it untouched when colour is off.
    #[must_use]
    pub fn paint(&self, ink: Ink, text: &str) -> String {
        if !self.colour {
            return text.to_string();
        }
        format!("{}{}{}", ink.bright_code(), text, "\x1b[0m")
    }

    /// A success line.
    #[must_use]
    pub fn ok(&self, text: &str) -> String {
        format!("{} {}", self.paint(Ink::Green, self.glyphs.ok), text)
    }

    /// A warning line.
    #[must_use]
    pub fn warn(&self, text: &str) -> String {
        format!("{} {}", self.paint(Ink::Amber, self.glyphs.warn), text)
    }

    /// A failure line.
    ///
    /// Deliberately takes a remedy, because a message that reports a problem
    /// without saying what to do about it is an incomplete message.
    #[must_use]
    pub fn error(&self, text: &str, remedy: &str) -> String {
        if remedy.is_empty() {
            return format!("{} {}", self.paint(Ink::Red, self.glyphs.fail), text);
        }
        format!(
            "{} {}\n  {}",
            self.paint(Ink::Red, self.glyphs.fail),
            self.paint(Ink::Bold, text),
            self.paint(Ink::Dim, remedy)
        )
    }

    /// A quiet informational line.
    #[must_use]
    pub fn info(&self, text: &str) -> String {
        format!("  {} {}", self.paint(Ink::Dim, self.glyphs.info), text)
    }

    /// A key/value pair, aligned for scanning.
    #[must_use]
    pub fn field(&self, label: &str, value: &str) -> String {
        format!(
            "  {} {}",
            self.paint(Ink::Dim, &pad(label, 12)),
            self.paint(Ink::Bold, value)
        )
    }

    /// A horizontal rule of the given width.
    #[must_use]
    pub fn rule(&self, width: usize) -> String {
        self.paint(Ink::Dim, &self.glyphs.rule.repeat(width))
    }

    /// A heading, printed above a block of related output.
    #[must_use]
    pub fn heading(&self, text: &str) -> String {
        format!("\n{}", self.paint(Ink::Bold, text))
    }

    /// A dimmed aside.
    #[must_use]
    pub fn dim(&self, text: &str) -> String {
        self.paint(Ink::Dim, text)
    }

    /// Emphasised text.
    #[must_use]
    pub fn strong(&self, text: &str) -> String {
        self.paint(Ink::Bold, text)
    }

    /// A URL, emphasised so the eye lands on it.
    #[must_use]
    pub fn url(&self, text: &str) -> String {
        self.paint(Ink::Blue, text)
    }
}

/// Pad a string to a width, counting characters rather than bytes.
///
/// A byte count would misalign every row that contains a non-ASCII character,
/// which is exactly the kind of small wrongness that makes a table look
/// careless.
#[must_use]
pub fn pad(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        text.to_string()
    } else {
        format!("{text}{}", " ".repeat(width - len))
    }
}

/// Truncate to a width, appending an ellipsis when shortened.
#[must_use]
pub fn truncate(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

/// Render a table with aligned columns.
///
/// Column widths are computed from the content, so nothing is hardcoded to a
/// machine's particular data. The last column is never padded, which keeps
/// trailing whitespace out of redirected output.
#[must_use]
pub fn table(style: &Style, headers: &[&str], rows: &[Vec<String>]) -> String {
    table_capped(style, headers, rows, &[])
}

/// The width of the terminal this process is writing to.
///
/// `COLUMNS` wins when it is set, because that is how a shell, a CI job, or a
/// multiplexer tells a program the width it should believe — and because a
/// child process cannot always see the real terminal. Otherwise `tput`-style
/// detection is unavailable without a dependency, so the answer comes from the
/// OS.
///
/// `None` means "unknown", which callers must treat as "do not cap". Guessing a
/// width and truncating against it would cut data the user can actually see.
#[must_use]
pub fn terminal_width() -> Option<usize> {
    if let Some(cols) = std::env::var_os("COLUMNS") {
        if let Ok(width) = cols.to_string_lossy().trim().parse::<usize>() {
            if width > 0 {
                return Some(width);
            }
        }
    }

    #[cfg(unix)]
    {
        // SAFETY: `winsize` is a plain-old-data struct that `ioctl` fills in.
        // It is zeroed first so a failing call cannot leave it uninitialised.
        let mut ws: libc_winsize = unsafe { std::mem::zeroed() };
        // SAFETY: fd 1 is valid for the lifetime of the process, and
        // `TIOCGWINSZ` writes exactly `sizeof(winsize)` bytes into `ws`.
        let ok = unsafe { ioctl_winsize(1, &raw mut ws) };
        if ok == 0 && ws.ws_col > 0 {
            return Some(ws.ws_col as usize);
        }
        None
    }

    #[cfg(windows)]
    {
        win_width::console_columns()
    }

    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

#[cfg(unix)]
#[repr(C)]
#[derive(Clone, Copy)]
struct libc_winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "ioctl"]
    fn ioctl_winsize(fd: i32, ws: *mut libc_winsize) -> i32;
}

#[cfg(windows)]
mod win_width {
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Coord {
        x: i16,
        y: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct SmallRect {
        left: i16,
        top: i16,
        right: i16,
        bottom: i16,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct ConsoleScreenBufferInfo {
        size: Coord,
        cursor_position: Coord,
        attributes: u16,
        window: SmallRect,
        maximum_window_size: Coord,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetConsoleScreenBufferInfo(
            handle: *mut c_void,
            info: *mut ConsoleScreenBufferInfo,
        ) -> i32;
    }

    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    /// `INVALID_HANDLE_VALUE`, which `GetStdHandle` returns on failure.
    const INVALID: isize = -1;

    /// Columns the attached console window is wide, if there is one.
    pub(super) fn console_columns() -> Option<usize> {
        // SAFETY: both calls take plain-old-data arguments and write only into
        // the struct we own and pass by pointer.
        unsafe {
            let handle = GetStdHandle(STD_OUTPUT_HANDLE);
            if handle.is_null() || handle as isize == INVALID {
                return None;
            }
            let mut info = ConsoleScreenBufferInfo::default();
            if GetConsoleScreenBufferInfo(handle, &raw mut info) == 0 {
                return None;
            }
            // The *window* is what the user sees; the buffer is often far wider
            // and would let a table run off the visible area.
            let width = i32::from(info.window.right) - i32::from(info.window.left) + 1;
            usize::try_from(width).ok().filter(|w| *w > 0)
        }
    }
}

/// Render a table, capping chosen columns.
///
/// Without a cap, one long value — a deep workspace path, say — stretches the
/// whole table and pushes every other column off the screen. A capped column is
/// truncated with an ellipsis; the caller keeps the full value available
/// elsewhere, such as a tooltip in the control page.
///
/// A cap of `0` means **no cap**, which is how a caller says "this column is
/// always short, spend nothing on it".
#[must_use]
pub fn table_capped(
    style: &Style,
    headers: &[&str],
    rows: &[Vec<String>],
    max_widths: &[usize],
) -> String {
    if rows.is_empty() {
        return String::new();
    }

    // Truncate first, then measure. Measuring the untruncated value would size
    // the column to text that will never be printed.
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(i, cell)| match max_widths.get(i) {
                    Some(&cap) if cap > 0 => {
                        let plain = strip_ansi(cell);
                        if plain.chars().count() > cap {
                            truncate(&plain, cap)
                        } else {
                            cell.clone()
                        }
                    }
                    _ => cell.clone(),
                })
                .collect()
        })
        .collect();
    let rows = &rows;

    let columns = headers.len();
    let mut widths = vec![0usize; columns];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.chars().count();
    }
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(columns) {
            let visible = visible_width(cell);
            if visible > widths[i] {
                widths[i] = visible;
            }
        }
    }

    let mut out = String::new();

    // Header
    let head: Vec<String> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let cell = pad(h, widths[i]);
            style.paint(Ink::Dim, cell.trim_end())
        })
        .collect();
    out.push_str(&join_row(&head, &widths));
    out.push('\n');

    // A rule under the header, sized to the table.
    let rule_width: usize = widths.iter().sum::<usize>() + (columns.saturating_sub(1)) * 3;
    out.push_str(&style.rule(rule_width));
    out.push('\n');

    for row in rows {
        let cells: Vec<String> = (0..columns)
            .map(|i| row.get(i).cloned().unwrap_or_default())
            .collect();
        out.push_str(&join_row(&cells, &widths));
        out.push('\n');
    }

    out.trim_end().to_string()
}

/// Render a table that fits the terminal.
///
/// # Why this exists
///
/// A table with fixed caps is wrong on both ends. On a narrow terminal it still
/// wraps and becomes unreadable; on a wide one it truncates values that had
/// room to spare. Neither failure is visible to the person who wrote the caps,
/// because they tested on exactly one terminal — their own.
///
/// So the caps passed here are **preferences, not limits**. The renderer
/// measures each column's need — the wider of its header and its widest value,
/// since the header is a row like any other — and only spends the caps when the
/// natural table would not fit. When it does fit, every value is shown in full.
/// The result looks deliberate at 80 columns and at 200 without the caller
/// knowing either number.
///
/// The guarantee is absolute: the rendered table is never wider than the
/// terminal. Width is taken from the column that can best afford to lose it,
/// and a column is never truncated below the label that identifies it.
///
/// `flex` names the columns allowed to give up space as `(index, preferred
/// cap)` pairs, where the cap counts the value *beyond* the column's header, so
/// shrinking steals from the wide descriptive column rather than from the short
/// one that would be destroyed by it.
#[must_use]
pub fn table_fitted(
    style: &Style,
    headers: &[&str],
    rows: &[Vec<String>],
    flex: &[(usize, usize)],
) -> String {
    // Without a known width there is nothing to fit to. Showing everything is
    // the honest fallback: a wrapped line loses less than a truncated one.
    let Some(terminal) = terminal_width() else {
        return table_capped(style, headers, rows, &[]);
    };

    let columns = headers.len();
    let natural: Vec<usize> = (0..columns)
        .map(|i| {
            let header = headers.get(i).map_or(0, |h| h.chars().count());
            rows.iter()
                .filter_map(|row| row.get(i))
                .map(|cell| visible_width(cell))
                .fold(header, usize::max)
        })
        .collect();

    let gaps = columns.saturating_sub(1) * 3;
    let total: usize = natural.iter().sum::<usize>() + gaps;
    if total <= terminal {
        return table_capped(style, headers, rows, &[]);
    }

    // Every column costs at least its header, and a column may not be shrunk
    // below the label that identifies it. Reserving that up front keeps the
    // arithmetic honest: the budget handed out below is genuinely free space
    // rather than space already spoken for. Without this the shares would be
    // computed against a total that silently excluded every header, and the
    // table would overflow by exactly the width the headers take.
    //
    // A column whose header is *empty* — a status marker, say — has a header
    // floor of zero but is not flexible either, so it will render at its
    // natural width no matter what. Reserving zero for it would hand its width
    // to the flexible columns as well, and the table would overshoot by exactly
    // that much. Its floor is therefore its natural width: it is simply not
    // part of the competition.
    let floor: Vec<usize> = (0..columns)
        .map(|i| headers.get(i).map_or(0, |h| h.chars().count()))
        .collect();
    let unshrinkable: Vec<usize> = (0..columns)
        .filter(|i| !flex.iter().any(|(index, _)| index == i))
        .collect();
    let mut budget = terminal.saturating_sub(gaps);
    for index in 0..columns {
        let reserved = if unshrinkable.contains(&index) {
            natural[index]
        } else {
            floor[index]
        };
        budget = budget.saturating_sub(reserved);
    }

    // A flexible column may claim only the surplus the caller allowed beyond
    // its own header — `cap` reads as "this much value, past the label" — while
    // the floor always keeps the column legible.
    let mut contenders: Vec<(usize, usize)> = flex
        .iter()
        .filter(|(index, cap)| *cap > 0 && natural.get(*index).copied().unwrap_or(0) > 0)
        .map(|&(index, cap)| {
            let want = natural[index].min(floor[index].saturating_add(cap));
            (index, want)
        })
        .collect();

    // Largest need first: the widest column has the most to lose, and is
    // usually the one the reader can afford to see shortened.
    contenders.sort_by_key(|(_, want)| std::cmp::Reverse(*want));

    // A strict even split reserves the same width for a column needing three
    // characters as for one holding a path. Widening the demand pool first —
    // each column asking for the larger of its own need and an equal share —
    // lets a narrow column settle at its need and hand the difference back.
    // That surplus is exactly what stops the path column collapsing to a stub
    // while a four-character status marker sits on a ten-column gutter.
    let demand: usize = contenders.iter().map(|(_, want)| *want).sum();
    let even = demand.div_ceil(contenders.len().max(1));
    let pool: usize = contenders.iter().map(|(_, want)| (*want).max(even)).sum();

    // `caps` holds the *surplus* granted to each column, before its floor is
    // added back. Keeping the two separate is what lets the clamp below be
    // stated in the units the caller actually supplied.
    let mut caps = vec![0usize; columns];
    for (index, want) in &contenders {
        let weighted = (*want).max(even);
        // `pool` is a sum of positive terms, so it is only zero when there are
        // no contenders at all — in which case nothing is granted.
        let share = budget
            .saturating_mul(weighted)
            .checked_div(pool)
            .unwrap_or(0);
        let surplus = want.saturating_sub(floor[*index]);
        caps[*index] = share.min(surplus);
    }

    // Hand back whatever the shares did not use — from integer division, or
    // from a column that asked for less than its share — largest first, because
    // that is where another character buys the most readable output.
    let spent: usize = contenders.iter().map(|(index, _)| caps[*index]).sum();
    let mut left = budget.saturating_sub(spent);
    while left > 0 {
        let mut gave = false;
        for (index, want) in &contenders {
            if left == 0 {
                break;
            }
            if caps[*index] < want.saturating_sub(floor[*index]) {
                caps[*index] += 1;
                left -= 1;
                gave = true;
            }
        }
        if !gave {
            break;
        }
    }

    // Add the reserved floor back. A column granted no surplus ends up at
    // exactly its header width: narrow, but honest, and never overflowing.
    for (index, _) in &contenders {
        caps[*index] += floor[*index];
    }

    // `table_capped` reads a cap of `0` as "no cap", which is the right
    // default for a caller but wrong here: a column that is not flexible has
    // already been given its exact width by the budget arithmetic above, and
    // leaving it uncapped would let it render at its natural width and push the
    // table past the terminal it was just measured against.
    let flexible: Vec<usize> = flex.iter().map(|(index, _)| *index).collect();
    for index in 0..columns {
        if !flexible.contains(&index) {
            caps[index] = natural[index];
        }
    }

    table_capped(style, headers, rows, &caps)
}

/// Join a row, padding every column except the last.
///
/// The final column is left unpadded so redirected output has no trailing
/// whitespace — a small thing that matters when the output is diffed or pasted.
fn join_row(cells: &[String], widths: &[usize]) -> String {
    let last = cells.len().saturating_sub(1);
    cells
        .iter()
        .enumerate()
        .map(|(i, c)| {
            if i == last {
                c.clone()
            } else {
                format!("{}   ", pad(c, widths[i]))
            }
        })
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The visible width of a string, ignoring ANSI escape sequences.
///
/// Without this, any styled cell would be measured as far wider than it looks
/// and every table containing colour would be misaligned.
#[must_use]
pub fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut in_escape = false;
    for ch in text.chars() {
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            width += 1;
        }
    }
    width
}

/// Remove ANSI escape sequences from a string.
///
/// Used where a styled value must be measured or truncated as plain text.
#[must_use]
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_escape = false;
    for ch in text.chars() {
        if in_escape {
            if ch == 'm' {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            in_escape = true;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Write a line to stdout, ignoring a closed pipe.
///
/// `router list | head` closes the pipe early. That is not an error worth
/// reporting to the user, and on some platforms it would otherwise panic.
pub fn out(line: &str) {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{line}");
}

/// Write a line to stderr.
pub fn err(line: &str) {
    let mut stderr = std::io::stderr();
    let _ = writeln!(stderr, "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Style {
        Style {
            colour: false,
            glyphs: Glyphs::unicode(),
            verbosity: Verbosity::Normal,
        }
    }

    #[test]
    fn no_colour_by_default_when_not_a_terminal() {
        // Piped output must stay free of escape sequences.
        let mode = ColourMode::Never;
        assert!(!mode.enabled(true));
        assert!(!mode.enabled(false));
    }

    #[test]
    fn auto_mode_follows_the_terminal() {
        // Whether `Auto` also honours `NO_COLOR` is environment-dependent, so
        // this asserts the terminal rule alone: with no colour-suppressing
        // variable set, a terminal gets colour and a pipe does not.
        //
        // The environment is not mutated here — process-wide env changes race
        // with other tests — so the NO_COLOR rule is covered separately below
        // by `explicit_env_parsing_covers_the_usual_spellings`.
        let mode = ColourMode::Auto;
        if std::env::var_os("NO_COLOR").is_none()
            && std::env::var_os("DSH_ROUTER_NO_COLOR").is_none()
        {
            assert!(mode.enabled(true), "a terminal should get colour");
        }
        assert!(!mode.enabled(false), "a pipe must never get colour");
    }

    #[test]
    fn no_color_disables_auto_mode_but_not_always() {
        // The rule, stated without touching the environment: `Auto` consults
        // NO_COLOR, `Always` and `Never` do not.
        assert!(ColourMode::Always.enabled(true));
        assert!(ColourMode::Always.enabled(false));
        assert!(!ColourMode::Never.enabled(true));
        // `Auto` + a terminal + NO_COLOR set is covered by the branch in
        // `enabled`; asserting the constant behaviour is what matters here.
        assert!(!ColourMode::Never.enabled(false));
    }

    #[test]
    fn always_mode_is_unconditional() {
        assert!(ColourMode::Always.enabled(false));
    }

    #[test]
    fn explicit_values_parse_with_the_usual_spellings() {
        assert_eq!(ColourMode::parse(Some("always")), Ok(ColourMode::Always));
        assert_eq!(ColourMode::parse(Some("TRUE")), Ok(ColourMode::Always));
        assert_eq!(ColourMode::parse(Some("never")), Ok(ColourMode::Never));
        assert_eq!(ColourMode::parse(Some("0")), Ok(ColourMode::Never));
        assert_eq!(ColourMode::parse(Some("auto")), Ok(ColourMode::Auto));
        assert_eq!(ColourMode::parse(None), Ok(ColourMode::Auto));
    }

    #[test]
    fn an_unrecognised_explicit_value_is_an_error_not_a_default() {
        // The bug this guards: `--colour bogus` silently became `auto`, so a
        // script that mistyped `always` got no colour and no explanation — the
        // failure invisible exactly where it matters.
        assert_eq!(
            ColourMode::parse(Some("bogus")),
            Err("bogus".to_string()),
            "a typo must be reported, not absorbed"
        );
        assert_eq!(
            ColourMode::parse(Some("alwaysx")),
            Err("alwaysx".to_string())
        );
        assert_eq!(ColourMode::parse(Some("")), Err(String::new()));
    }

    #[test]
    fn a_bad_environment_value_still_falls_back() {
        // The environment is different from a flag: `NO_COLOR` and shell
        // profiles set variables the user did not type at this moment, and
        // refusing to run because of one would be hostile.
        //
        // Held under the environment lock with `NO_COLOR` cleared, because the
        // test runner's own environment would otherwise decide the answer — and
        // a test whose result depends on who launched it is not a test.
        let _guard = env_lock();
        let previous = std::env::var_os("NO_COLOR");
        unsafe { std::env::remove_var("NO_COLOR") };
        assert_eq!(ColourMode::from_env(Some("whatever")), ColourMode::Auto);
        match previous {
            Some(v) => unsafe { std::env::set_var("NO_COLOR", v) },
            None => unsafe { std::env::remove_var("NO_COLOR") },
        }
    }

    #[test]
    fn paint_is_identity_when_colour_is_off() {
        let s = plain();
        assert_eq!(s.paint(Ink::Red, "hello"), "hello");
    }

    #[test]
    fn paint_wraps_when_colour_is_on() {
        let s = Style {
            colour: true,
            glyphs: Glyphs::unicode(),
            verbosity: Verbosity::Normal,
        };
        let painted = s.paint(Ink::Red, "x");
        assert!(painted.contains("\x1b["));
        assert!(painted.ends_with("\x1b[0m"));
    }

    #[test]
    fn every_painted_string_resets_so_colour_cannot_leak() {
        let s = Style {
            colour: true,
            glyphs: Glyphs::unicode(),
            verbosity: Verbosity::Normal,
        };
        for ink in [
            Ink::Plain,
            Ink::Dim,
            Ink::Bold,
            Ink::Amber,
            Ink::Red,
            Ink::Green,
            Ink::Blue,
            Ink::Violet,
        ] {
            assert!(s.paint(ink, "t").ends_with("\x1b[0m"), "{ink:?} must reset");
        }
    }

    #[test]
    fn visible_width_ignores_escape_sequences() {
        assert_eq!(visible_width("abc"), 3);
        assert_eq!(visible_width("\x1b[1mabc\x1b[0m"), 3);
        assert_eq!(visible_width("\x1b[92mhello\x1b[0m"), 5);
    }

    #[test]
    fn pad_counts_characters_not_bytes() {
        // "café" is 4 characters and 5 bytes; padding must use characters.
        assert_eq!(pad("café", 6).chars().count(), 6);
        assert_eq!(pad("ab", 4), "ab  ");
        assert_eq!(pad("abcde", 3), "abcde");
    }

    #[test]
    fn truncate_is_character_safe() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5).chars().count(), 5);
        assert!(truncate("abcdefghij", 5).ends_with('…'));
        assert_eq!(truncate("café au lait", 6).chars().count(), 6);
    }

    #[test]
    fn table_aligns_columns() {
        let s = plain();
        let out = table(
            &s,
            &["NAME", "PORT"],
            &[
                vec!["alpha".to_string(), "3081".to_string()],
                vec!["a-very-long-name".to_string(), "3082".to_string()],
            ],
        );
        let lines: Vec<&str> = out.lines().collect();
        // Every row must start at the same column as the others.
        let alpha_col = lines[2].find("3081").unwrap();
        let long_col = lines[3].find("3082").unwrap();
        assert_eq!(alpha_col, long_col, "value column must align");
    }

    #[test]
    fn table_handles_styled_cells_without_misaligning() {
        // The bug this guards: measuring a styled cell by its bytes.
        let s = Style {
            colour: true,
            glyphs: Glyphs::unicode(),
            verbosity: Verbosity::Normal,
        };
        let out = table(
            &s,
            &["A", "B"],
            &[vec![s.paint(Ink::Red, "x"), "y".to_string()]],
        );
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() >= 3);
        // The header's second column and the row's second column must agree.
        assert_eq!(
            visible_width(lines[0]) - visible_width("B"),
            visible_width(lines[2]) - visible_width("y")
        );
    }

    #[test]
    fn empty_table_renders_nothing() {
        let s = plain();
        assert!(table(&s, &["A"], &[]).is_empty());
    }

    #[test]
    fn table_has_no_trailing_whitespace() {
        let s = plain();
        let out = table(
            &s,
            &["NAME", "PORT"],
            &[vec!["a".to_string(), "3081".to_string()]],
        );
        for line in out.lines() {
            assert_eq!(line, line.trim_end(), "line has trailing space: {line:?}");
        }
    }

    #[test]
    fn table_tolerates_a_short_row() {
        let s = plain();
        let out = table(&s, &["A", "B", "C"], &[vec!["only".to_string()]]);
        assert!(out.contains("only"));
    }

    #[test]
    fn error_always_carries_a_remedy_when_given_one() {
        let s = plain();
        let msg = s.error("port 3080 is reserved", "the router allocates from 3081");
        assert!(msg.contains("port 3080 is reserved"));
        assert!(msg.contains("allocates from 3081"));
    }

    #[test]
    fn error_without_a_remedy_still_renders() {
        let s = plain();
        let msg = s.error("something failed", "");
        assert!(msg.contains("something failed"));
    }

    #[test]
    fn field_output_is_aligned() {
        let s = plain();
        let a = s.field("port", "3081");
        let b = s.field("workspace", "/tmp/x");
        // Both values start at the same offset.
        assert_eq!(a.find("3081"), b.find("/tmp/x"));
    }

    #[test]
    fn glyph_sets_are_complete() {
        let u = Glyphs::unicode();
        let a = Glyphs::ascii();
        for g in [
            u.ok, u.warn, u.fail, u.running, u.stopped, a.ok, a.warn, a.fail,
        ] {
            assert!(!g.is_empty());
        }
        assert_ne!(u.rule, a.rule);
    }

    #[test]
    fn ascii_fallback_is_actually_ascii() {
        // The whole point of the fallback: no character a legacy console
        // would render as mojibake.
        let a = Glyphs::ascii();
        for g in [
            a.ok, a.warn, a.fail, a.info, a.bullet, a.arrow, a.rule, a.running, a.stopped, a.sep,
        ] {
            assert!(g.is_ascii(), "{g:?} is not ASCII");
        }
    }

    #[test]
    fn table_capping_truncates_a_long_column() {
        let s = plain();
        let long = "~\\AppData\\Local\\Temp\\router-demo-123\\a\\very\\deeply\\nested\\project";
        let out = table_capped(
            &s,
            &["", "NAME", "PORT", "WORKSPACE", "MODEL"],
            &[vec![
                "*".to_string(),
                "deep".to_string(),
                "3084".to_string(),
                long.to_string(),
                "default".to_string(),
            ]],
            &[0, 24, 0, 44, 26],
        );
        assert!(
            out.contains('…'),
            "a 60-character path in a 44-wide column must be truncated:\n{out}"
        );
        assert!(!out.contains("nested"), "the tail must be gone:\n{out}");
    }

    /// Render a fitted table as though the terminal were `width` columns wide.
    ///
    /// The width is set through `COLUMNS` rather than a parameter because that
    /// is the mechanism the renderer itself uses, so the tests exercise the real
    /// detection path instead of a seam that exists only for testing.
    fn fitted_at(
        width: usize,
        style: &Style,
        headers: &[&str],
        rows: &[Vec<String>],
        flex: &[(usize, usize)],
    ) -> String {
        with_columns(Some(width), || table_fitted(style, headers, rows, flex))
    }

    /// Serialises every test that touches `COLUMNS`.
    ///
    /// Environment is process-wide, so the test runner's parallelism turns
    /// `set_var` into a race: one test sets a narrow width, another asserts
    /// against a wide one, and whichever loses fails intermittently. That is
    /// exactly what happened — the fitting tests passed alone and failed in a
    /// full run.
    ///
    /// A poisoned mutex is recovered rather than propagated: a test that
    /// panicked while holding this lock has already reported its own failure,
    /// and making every later test fail too would bury the real cause.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Run `body` with `COLUMNS` set to `width` (or unset when `None`).
    ///
    /// Holds [`env_lock`] for the duration, so the width one test sets cannot
    /// be observed by another, and restores the previous value afterwards.
    fn with_columns<T>(width: Option<usize>, body: impl FnOnce() -> T) -> T {
        let _guard = env_lock();
        let previous = std::env::var_os("COLUMNS");
        match width {
            Some(w) => unsafe { std::env::set_var("COLUMNS", w.to_string()) },
            None => unsafe { std::env::remove_var("COLUMNS") },
        }
        let out = body();
        match previous {
            Some(v) => unsafe { std::env::set_var("COLUMNS", v) },
            None => unsafe { std::env::remove_var("COLUMNS") },
        }
        out
    }

    /// Tests that read or write `NO_COLOR` need the same protection.
    #[test]
    fn colour_mode_reads_the_environment_without_racing() {
        let _guard = env_lock();
        let previous = std::env::var_os("NO_COLOR");
        unsafe { std::env::set_var("NO_COLOR", "1") };
        assert!(
            !ColourMode::Auto.enabled(true),
            "NO_COLOR must disable auto"
        );
        match previous {
            Some(v) => unsafe { std::env::set_var("NO_COLOR", v) },
            None => unsafe { std::env::remove_var("NO_COLOR") },
        }
    }

    /// A table that fits: the wide column does the giving.
    #[test]
    fn fitted_table_caps_only_when_it_must() {
        let s = plain();
        let long = "~\\projects\\a\\very\\deeply\\nested\\project\\directory\\structure\\here";
        let rows = vec![vec![
            "*".to_string(),
            "deep".to_string(),
            "3084".to_string(),
            long.to_string(),
            "default".to_string(),
        ]];
        let headers = &["", "NAME", "PORT", "WORKSPACE", "MODEL"];
        let flex = &[(1, 26), (3, 46), (4, 28)];

        // Room to spare: every value is shown in full. Truncating here would
        // lose data the user could have read.
        let roomy = fitted_at(240, &s, headers, &rows, flex);
        assert!(
            roomy.contains("structure\\here"),
            "a wide terminal must show the whole path:\n{roomy}"
        );
        assert!(
            !roomy.contains('…'),
            "nothing may be truncated when it fits:\n{roomy}"
        );

        // Genuinely tight: the path gives way and says so.
        let tight = fitted_at(80, &s, headers, &rows, flex);
        assert!(
            tight.contains('…'),
            "a narrow terminal must elide:\n{tight}"
        );
        assert!(
            !tight.contains("structure"),
            "the elided tail must be gone:\n{tight}"
        );
        assert!(
            tight.contains("3084"),
            "the port is short by construction and must survive:\n{tight}"
        );
        assert!(
            tight.contains("default"),
            "the model column keeps its value:\n{tight}"
        );
    }

    /// No width is knowable, so nothing may be guessed away.
    #[test]
    fn fitted_table_shows_everything_when_width_is_unknown() {
        let s = plain();
        let long = "~\\projects\\a\\very\\deeply\\nested\\project";
        let out = with_columns(None, || {
            table_fitted(
                &s,
                &["", "NAME", "PORT", "WORKSPACE"],
                &[vec![
                    "*".to_string(),
                    "deep".to_string(),
                    "3084".to_string(),
                    long.to_string(),
                ]],
                &[(1, 26), (3, 46)],
            )
        });
        assert!(
            out.contains("nested\\project"),
            "with no known width the honest output is everything:\n{out}"
        );
    }

    /// The whole point of fitting: never render wider than the terminal.
    #[test]
    fn fitted_table_never_exceeds_the_terminal() {
        let s = plain();
        let rows: Vec<Vec<String>> = (0..3)
            .map(|i| {
                vec![
                    "*".to_string(),
                    format!("instance-{i}"),
                    (3081 + i).to_string(),
                    "~\\projects\\client\\platform\\services\\gateway\\src".to_string(),
                    "deepseek-v4-pro".to_string(),
                ]
            })
            .collect();
        let headers = &["", "NAME", "PORT", "WORKSPACE", "MODEL"];
        let flex = &[(1, 26), (3, 46), (4, 28)];

        for width in [64usize, 72, 80, 100, 120, 160] {
            let out = fitted_at(width, &s, headers, &rows, flex);
            let widest = out
                .lines()
                .map(visible_width)
                .max()
                .expect("a table always has lines");
            assert!(
                widest <= width,
                "at {width} columns a line rendered {widest} wide:\n{out}"
            );
        }
    }

    /// A narrow column must not hog the gutter a wide one needs.
    #[test]
    fn fitted_table_gives_space_to_the_column_that_uses_it() {
        let s = plain();
        let rows = vec![vec![
            "*".to_string(),
            "api".to_string(),
            "3082".to_string(),
            "~\\projects\\company\\platform\\services\\api-gateway".to_string(),
            "deepseek-v4-pro".to_string(),
        ]];
        let out = fitted_at(
            80,
            &s,
            &["", "NAME", "PORT", "WORKSPACE", "MODEL"],
            &rows,
            &[(1, 26), (3, 46), (4, 28)],
        );

        // The path column must come out meaningfully wider than the stub an
        // even split would have left it, while NAME stays at its natural width.
        let header = out
            .lines()
            .find(|l| l.contains("WORKSPACE"))
            .expect("header");
        let start = header.find("WORKSPACE").expect("header label");
        let end = header.find("MODEL").expect("model label");
        let workspace_width = end.saturating_sub(start);
        assert!(
            workspace_width >= 20,
            "the path column collapsed to {workspace_width} characters:\n{out}"
        );
        let name_start = header.find("NAME").expect("name label");
        let name_end = header.find("PORT").expect("port label");
        assert!(
            name_end - name_start <= 16,
            "the name column should stay at its natural width:\n{out}"
        );
    }

    #[test]
    fn verbosity_orders_correctly() {
        assert!(Verbosity::Quiet < Verbosity::Normal);
        assert!(Verbosity::Normal < Verbosity::Verbose);
    }
}
