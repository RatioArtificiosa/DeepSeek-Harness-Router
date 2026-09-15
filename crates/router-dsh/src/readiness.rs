//! Readiness detection.
//!
//! Upstream documents that the web bundle prints a `dsh web:` URL line **only
//! after** the plugin tree settles and connection authentication is available:
//!
//! > "The URL line and browser handoff are readiness signals: supervisors RPC
//! > as soon as they observe the line… A tree disposed mid-boot announces
//! > nothing."
//!
//! So the line is a real signal, not a log message we happen to recognize. But
//! it proves only that the harness *believes* it is ready; it does not prove
//! the socket accepts connections. The supervisor therefore requires **both**
//! the line and a successful HTTP probe, and never a fixed sleep
//! never a fixed sleep.
//!
//! This module owns the line parsing, which is the part worth testing: a
//! regex that quietly stops matching after an upstream release would turn a
//! healthy boot into a timeout, and nothing else in the system would notice.

/// What a line of harness output told us.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputSignal {
    /// The readiness line, carrying the URL the harness believes it serves.
    Ready {
        /// The authenticated URL the harness announced.
        url: String,
    },
    /// The harness reported a build problem.
    FrontendMissing,
    /// The harness reported a missing or unusable credential.
    MissingCredential,
    /// The harness reported that process confinement is unavailable.
    SandboxUnavailable,
    /// The harness reported a port conflict.
    PortInUse,
    /// A recognized but uninteresting line.
    Other,
}

/// The marker the harness prints when it is ready to serve.
///
/// Matched case-insensitively and prefix-wise, because the exact spacing has
/// varied and a strict match on whitespace is a needless fragility.
const READY_MARKER: &str = "dsh web:";

/// Phrases upstream uses for a missing frontend build.
const FRONTEND_MARKERS: &[&str] = &["frontend", "not built", "pnpm run build"];

/// Phrases upstream uses for a missing credential.
const CREDENTIAL_MARKERS: &[&str] = &["missing_credential", "missing credential"];

/// Phrases upstream uses when confinement is unavailable.
const SANDBOX_MARKERS: &[&str] = &["sandbox_unavailable", "sandbox unavailable"];

/// Phrases the OS emits on a port conflict.
const PORT_MARKERS: &[&str] = &["eaddrinuse", "address already in use"];

/// Classify one line of harness output.
///
/// Order matters: a line is checked for specific failures before the generic
/// readiness marker, so a diagnostic that happens to contain "dsh web:" is not
/// mistaken for success.
#[must_use]
pub fn classify_line(line: &str) -> OutputSignal {
    let lower = line.to_ascii_lowercase();

    if PORT_MARKERS.iter().any(|m| lower.contains(m)) {
        return OutputSignal::PortInUse;
    }
    if SANDBOX_MARKERS.iter().any(|m| lower.contains(m)) {
        return OutputSignal::SandboxUnavailable;
    }
    if CREDENTIAL_MARKERS.iter().any(|m| lower.contains(m)) {
        return OutputSignal::MissingCredential;
    }
    // A build hint is only meaningful when paired with the frontend wording;
    // "pnpm run build" alone appears in unrelated advice.
    if FRONTEND_MARKERS[0..2].iter().any(|m| lower.contains(m)) && lower.contains("dist") {
        return OutputSignal::FrontendMissing;
    }

    if let Some(url) = extract_ready_url(line) {
        return OutputSignal::Ready { url };
    }

    OutputSignal::Other
}

/// Extract the URL from a readiness line, if this is one.
///
/// Returns `None` for any line that is not the readiness announcement.
#[must_use]
pub fn extract_ready_url(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let pos = lower.find(READY_MARKER)?;

    // Everything after the marker is the URL, possibly followed by trailing
    // punctuation or colour codes that the harness or a wrapper may add.
    let rest = &line[pos + READY_MARKER.len()..];
    let candidate = rest
        .trim()
        .trim_start_matches(['\u{1b}', '[', '0', 'm']) // stray ANSI reset
        .trim();

    let url: String = candidate
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '\u{1b}')
        .collect();

    if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.trim_end_matches(['.', ',']).to_string())
    } else {
        None
    }
}

/// Whether a line indicates the harness has given up.
///
/// Deliberately conservative: it must not fire on ordinary startup chatter, so
/// it looks for an explicit failure marker rather than merely the word "error"
/// appearing somewhere in a line.
///
/// The `dsh:` prefix is the launcher's own diagnostic channel, which formats
/// failures as `dsh: <CODE>: <message>`. Any such line is a real failure, so
/// the code does not need to contain the word "error" for us to trust it —
/// `dsh: DSH_READY_TIMEOUT: gave up` is exactly as fatal as
/// `dsh: DSH_EXITED: the runtime stopped`.
#[must_use]
pub fn is_fatal(line: &str) -> bool {
    // Check the original casing for the code, because a screaming-snake
    // identifier is uppercase by definition and lowercasing it first would
    // destroy the very signal we are looking for.
    if let Some(rest) = line.trim().strip_prefix("dsh: ") {
        // The headless runner narrates progress on this channel; it is not a
        // failure, and mistaking it for one would abort healthy runs.
        if rest.to_ascii_lowercase().starts_with("reasoning:") {
            return false;
        }
        if let Some(code) = rest.split(':').next() {
            let looks_like_a_code = code.len() > 3
                && code
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
            if looks_like_a_code {
                return true;
            }
        }
    }

    let lower = line.to_ascii_lowercase();
    lower.contains("error:") || lower.contains("fatal") || lower.contains("panic")
}

/// Keep only the last `n` lines of a rolling buffer.
///
/// The stderr tail is what explains a crash, so it is bounded rather than
/// unbounded: a chatty runtime must not grow the supervisor's memory.
pub fn push_bounded(buffer: &mut Vec<String>, line: String, max: usize) {
    if buffer.len() >= max {
        buffer.remove(0);
    }
    buffer.push(line);
}

/// Convenience: the default tail length.
pub const DEFAULT_TAIL_LINES: usize = 50;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_readiness_line() {
        let sig = classify_line("dsh web: http://127.0.0.1:3081");
        assert_eq!(
            sig,
            OutputSignal::Ready {
                url: "http://127.0.0.1:3081".to_string()
            }
        );
    }

    #[test]
    fn readiness_line_is_matched_case_insensitively() {
        // A casing change upstream must not break readiness detection.
        assert!(matches!(
            classify_line("DSH WEB: http://127.0.0.1:3081"),
            OutputSignal::Ready { .. }
        ));
    }

    #[test]
    fn readiness_url_ignores_trailing_punctuation() {
        let sig = classify_line("dsh web: http://127.0.0.1:3081.");
        match sig {
            OutputSignal::Ready { url } => assert_eq!(url, "http://127.0.0.1:3081"),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn readiness_url_takes_the_token_form() {
        // The harness prints a URL carrying a launch token.
        let sig = classify_line("dsh web: http://127.0.0.1:3081/?token=abc123");
        match sig {
            OutputSignal::Ready { url } => assert!(url.contains("token=abc123")),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn a_non_url_after_the_marker_is_not_readiness() {
        // Guards against matching a log line that merely mentions the marker.
        assert_eq!(classify_line("dsh web: starting"), OutputSignal::Other);
    }

    #[test]
    fn detects_a_missing_frontend_build() {
        let sig = classify_line(
            "Error: the frontend dist is missing; run `pnpm run build` before starting.",
        );
        assert_eq!(sig, OutputSignal::FrontendMissing);
    }

    #[test]
    fn detects_a_missing_credential() {
        assert_eq!(
            classify_line("request failed: MISSING_CREDENTIAL"),
            OutputSignal::MissingCredential
        );
    }

    #[test]
    fn detects_unavailable_confinement() {
        assert_eq!(
            classify_line("SANDBOX_UNAVAILABLE: no usable backend"),
            OutputSignal::SandboxUnavailable
        );
    }

    #[test]
    fn detects_a_port_conflict() {
        assert_eq!(
            classify_line("Error: listen EADDRINUSE: address already in use 127.0.0.1:3081"),
            OutputSignal::PortInUse
        );
    }

    #[test]
    fn specific_failures_win_over_the_readiness_marker() {
        // A diagnostic that happens to contain the marker must not be read as
        // success. This ordering is the reason the check exists.
        let sig = classify_line("dsh web: error EADDRINUSE on 3081");
        assert_eq!(sig, OutputSignal::PortInUse);
    }

    #[test]
    fn ordinary_lines_are_other() {
        for line in [
            "",
            "Loading plugins...",
            "  \u{1b}[32mok\u{1b}[0m some-plugin",
            "dsh: reasoning: thinking about the task",
        ] {
            assert_eq!(classify_line(line), OutputSignal::Other, "line: {line:?}");
        }
    }

    #[test]
    fn fatal_detection_is_conservative() {
        assert!(is_fatal("Error: something broke"));
        assert!(is_fatal("FATAL: cannot continue"));
        assert!(is_fatal("dsh: DSH_READY_TIMEOUT: gave up"));
        assert!(!is_fatal("Loading plugins..."));
        assert!(!is_fatal("dsh web: http://127.0.0.1:3081"));
    }

    #[test]
    fn bounded_buffer_keeps_the_most_recent_lines() {
        let mut buf = Vec::new();
        for i in 0..10 {
            push_bounded(&mut buf, format!("line-{i}"), 3);
        }
        assert_eq!(buf, vec!["line-7", "line-8", "line-9"]);
    }

    #[test]
    fn bounded_buffer_stays_bounded() {
        let mut buf = Vec::new();
        for i in 0..10_000 {
            push_bounded(&mut buf, format!("l{i}"), DEFAULT_TAIL_LINES);
        }
        assert_eq!(buf.len(), DEFAULT_TAIL_LINES);
    }

    #[test]
    fn extra_whitespace_after_the_marker_is_tolerated() {
        let sig = classify_line("dsh web:    http://127.0.0.1:3081");
        assert!(matches!(sig, OutputSignal::Ready { .. }));
    }
}
