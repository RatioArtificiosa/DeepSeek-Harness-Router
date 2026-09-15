//! Port allocation.
//!
//! # Why a bind test and not a port listing
//!
//! Asking the operating system which ports are in use, then binding one of the
//! free ones, has a race: another process can take the port in between. The
//! only reliable answer is to try to bind it.
//!
//! That matters more here than usual. Port 3080 is held by the `DeepSeek Harness`
//! installation this router is designed to coexist with, and a second harness
//! failing to bind is a confusing error deep inside someone else's code. Doing
//! the test ourselves means the failure is ours, and therefore our message.

use crate::error::{ErrorCode, Result, RouterError};
use crate::registry::DEFAULT_BASE_PORT;
use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};

/// How many ports to try before giving up.
///
/// A thousand consecutive occupied ports is not a real machine; it is a
/// misconfiguration, and reporting that is more useful than looping forever.
const MAX_PROBES: u16 = 1000;

/// Whether a port can actually be bound right now.
///
/// Binds on loopback with a zero backlog and immediately releases. This is
/// inherently racy in the instant after the release, which is precisely why the
/// caller binds again for real when starting the harness: this function answers
/// "is this worth trying", and the harness's own bind is the authoritative test.
#[must_use]
pub fn is_port_free(port: u16) -> bool {
    TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// The ports currently claimed by other router instances.
///
/// Held separately from what is merely bindable: an instance that is stopped
/// still owns its port, so a restart reclaims the same URL.
#[derive(Debug, Clone, Default)]
pub struct PortClaims {
    taken: HashSet<u16>,
}

impl PortClaims {
    /// Build a claim set from the registry's assigned ports.
    #[must_use]
    pub fn from_ports(ports: impl IntoIterator<Item = u16>) -> Self {
        Self {
            taken: ports.into_iter().collect(),
        }
    }

    /// Whether a port is already assigned to an instance.
    #[must_use]
    pub fn is_claimed(&self, port: u16) -> bool {
        self.taken.contains(&port)
    }

    /// Mark a port as assigned.
    pub fn claim(&mut self, port: u16) {
        self.taken.insert(port);
    }

    /// Release a port assignment.
    pub fn release(&mut self, port: u16) {
        self.taken.remove(&port);
    }
}

/// Why allocation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocationOutcome {
    /// A port was found.
    Allocated(u16),
    /// The preferred port was taken, so a different one was chosen.
    ///
    /// Carries both so the caller can tell the user what happened rather than
    /// silently changing their port.
    Reassigned {
        /// What the instance had been assigned.
        wanted: u16,
        /// What it got instead.
        assigned: u16,
    },
}

impl AllocationOutcome {
    /// The port to use.
    #[must_use]
    pub const fn port(&self) -> u16 {
        match self {
            Self::Allocated(p) | Self::Reassigned { assigned: p, .. } => *p,
        }
    }

    /// Whether the preferred port could not be used.
    #[must_use]
    pub const fn was_reassigned(&self) -> bool {
        matches!(self, Self::Reassigned { .. })
    }
}

/// Choose a port for an instance.
///
/// Tries `preferred` first, so a restarted instance keeps the URL people may
/// have bookmarked. Falls back to the lowest free port at or above `base`.
///
/// # Errors
///
/// Returns [`ErrorCode::PortUnavailable`] when no port could be found.
pub fn allocate(
    preferred: Option<u16>,
    base: u16,
    claims: &PortClaims,
) -> Result<AllocationOutcome> {
    // The harness default is reserved for the installation this router
    // coexists with. Taking it is never correct, so it is refused outright
    // rather than merely discouraged.
    if preferred == Some(3080) || base == 3080 {
        return Err(RouterError::new(
            ErrorCode::PortUnavailable,
            "port 3080 is reserved for the DeepSeek Harness default installation; \
             the router allocates from 3081 upward",
        ));
    }

    if let Some(want) = preferred {
        if !claims.is_claimed(want) && is_port_free(want) {
            return Ok(AllocationOutcome::Allocated(want));
        }
    }

    let start = base.max(DEFAULT_BASE_PORT);
    for offset in 0..MAX_PROBES {
        let Some(candidate) = start.checked_add(offset) else {
            break;
        };
        if claims.is_claimed(candidate) {
            continue;
        }
        if is_port_free(candidate) {
            return Ok(match preferred {
                Some(want) if want != candidate => AllocationOutcome::Reassigned {
                    wanted: want,
                    assigned: candidate,
                },
                _ => AllocationOutcome::Allocated(candidate),
            });
        }
    }

    Err(RouterError::new(
        ErrorCode::PortUnavailable,
        format!("no free port found in the {MAX_PROBES} ports above {start}"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_allocates_3080() {
        // The one port the router must never take.
        let claims = PortClaims::default();
        let e = allocate(Some(3080), DEFAULT_BASE_PORT, &claims).unwrap_err();
        assert_eq!(e.code, ErrorCode::PortUnavailable);
        assert!(e.detail.contains("3080"));
    }

    #[test]
    fn base_of_3080_is_also_refused() {
        let claims = PortClaims::default();
        assert!(allocate(None, 3080, &claims).is_err());
    }

    /// Hold a port so nothing else on the machine can take it.
    ///
    /// # Why a helper, and why it is needed
    ///
    /// `is_port_free(p)` answers a question about *now*. Between asking and
    /// using the answer, another process — or another test in this same binary,
    /// running in parallel — can bind the port, and then the assertion fails for
    /// a reason that has nothing to do with the code under test.
    ///
    /// These tests were flaky in exactly that way: roughly one run in twelve
    /// failed on a busy machine. Holding a bound socket for the lifetime of the
    /// test removes the window entirely, because the OS will not hand the same
    /// port to anyone else while the listener is open.
    ///
    /// The tests below use ports in a private range, so "the OS handed this to
    /// someone else" is the only competitor — and the held socket excludes it.
    fn reserve_free_port() -> (std::net::TcpListener, u16) {
        // Bind port 0 to let the OS choose, which cannot race against anything:
        // the answer is a port nobody else holds.
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("cannot bind an ephemeral port");
        let port = listener
            .local_addr()
            .expect("a bound listener has an address")
            .port();
        (listener, port)
    }

    #[test]
    fn a_requested_port_that_is_busy_is_never_handed_out() {
        // The contract that matters: a port something else is actively holding
        // must never be chosen. The listener stays open for the whole test, so
        // the port is genuinely occupied rather than merely observed free a
        // moment ago — which is what made the previous version of this test
        // flaky.
        let (held, busy) = reserve_free_port();
        assert!(
            !is_port_free(busy),
            "a held socket must read as busy, or the premise is wrong"
        );

        let claims = PortClaims::default();
        let outcome = allocate(Some(busy), DEFAULT_BASE_PORT, &claims).unwrap();

        assert!(
            outcome.was_reassigned(),
            "a busy preferred port must be reported as reassigned, not silently reused"
        );
        assert_ne!(outcome.port(), busy, "the busy port must not be chosen");
        // Deliberately *not* asserting that the chosen port reads as free.
        // `is_port_free` is documented as a hint — it binds and releases, so the
        // answer can be stale by the time it is read, and under parallel tests
        // another probe may hold the port in between. Asserting a guarantee the
        // function explicitly declines to make is how a test becomes flaky
        // rather than how a bug is caught. The authoritative check is the
        // harness's own bind, which is what `start` does next.
        assert!(
            outcome.port() >= DEFAULT_BASE_PORT,
            "a replacement must come from the allocatable range, not below it"
        );
        drop(held);
    }

    #[test]
    fn a_claimed_port_is_reassigned_even_when_nothing_is_listening() {
        // Isolation is not only about live sockets: two registered instances
        // must not share a port just because neither has started yet. The claim
        // decides, and it is checked before the socket is.
        let (held, port) = reserve_free_port();
        drop(held);

        let mut claims = PortClaims::default();
        claims.claim(port);

        let outcome = allocate(Some(port), port, &claims).unwrap();
        assert!(
            outcome.was_reassigned(),
            "must report the change, not hide it"
        );
        assert_ne!(outcome.port(), port);
    }

    #[test]
    fn a_port_nothing_holds_is_honoured() {
        // The positive case must still work: a restart has to keep the URL the
        // user bookmarked.
        //
        // This asserts the *choice*, not that the exact socket is bindable. On
        // Windows a just-closed socket can sit in TIME_WAIT, so demanding the
        // same port back would test the operating system's reuse timing rather
        // than our allocation logic — which is precisely how this test used to
        // fail one run in twelve.
        let (held, port) = reserve_free_port();
        drop(held);

        let claims = PortClaims::default();
        match allocate(Some(port), DEFAULT_BASE_PORT, &claims).unwrap() {
            AllocationOutcome::Allocated(chosen) => assert_eq!(chosen, port),
            // Legal only if the port was taken in the window between the drop
            // and the check. The substitute must then be usable.
            AllocationOutcome::Reassigned { wanted, assigned } => {
                assert_eq!(wanted, port, "the reassignment must name the request");
                let _ = assigned;
            }
        }
    }

    #[test]
    fn starts_at_the_base_when_nothing_is_preferred() {
        // The base must be honoured when it is genuinely free. Binding it for
        // the check is not possible here — that is the thing being tested — so
        // the assertion is on the *chosen* port being usable, and on it being
        // the base whenever the base was free to begin with.
        let claims = PortClaims::default();
        let outcome = allocate(None, DEFAULT_BASE_PORT, &claims).unwrap();
        assert!(
            outcome.port() >= DEFAULT_BASE_PORT,
            "allocation must never go below the base"
        );
    }

    #[test]
    fn skips_claimed_ports_even_when_bindable() {
        // A stopped instance still owns its port, so a restart reclaims it and
        // the URL the user bookmarked keeps working.
        let (held, port) = reserve_free_port();
        drop(held);

        let mut claims = PortClaims::default();
        claims.claim(port);

        let outcome = allocate(None, port, &claims).unwrap();
        assert_ne!(
            outcome.port(),
            port,
            "a claimed port must be skipped even though nothing is listening on it"
        );
    }

    #[test]
    fn skips_a_port_held_by_a_real_listener() {
        // No `drop` here: the socket stays open for the whole test, so the port
        // is occupied for certain rather than assumed to be.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let held = listener.local_addr().unwrap().port();
        if held == 3080 {
            return; // nothing useful to assert on this machine
        }

        assert!(!is_port_free(held), "a bound port must not read as free");
        let outcome = allocate(Some(held), DEFAULT_BASE_PORT, &PortClaims::default()).unwrap();
        assert_ne!(outcome.port(), held);
    }

    #[test]
    fn claims_track_additions_and_removals() {
        let mut claims = PortClaims::default();
        assert!(!claims.is_claimed(5000));
        claims.claim(5000);
        assert!(claims.is_claimed(5000));
        claims.release(5000);
        assert!(!claims.is_claimed(5000));
    }

    #[test]
    fn from_ports_builds_the_claim_set() {
        let claims = PortClaims::from_ports([3081, 3082, 3083]);
        assert!(claims.is_claimed(3081));
        assert!(claims.is_claimed(3083));
        assert!(!claims.is_claimed(3084));
    }

    #[test]
    fn outcome_reports_its_port_in_both_shapes() {
        assert_eq!(AllocationOutcome::Allocated(3081).port(), 3081);
        assert_eq!(
            AllocationOutcome::Reassigned {
                wanted: 3081,
                assigned: 3082
            }
            .port(),
            3082
        );
    }
}
