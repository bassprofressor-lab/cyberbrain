//! How many times one address may try an enrolment code that does not exist, and how many
//! refused sign-ins it writes into the hub's log (`SignInRefusals`).
//!
//! # Why a code needs this at all
//!
//! A fleet invitation's code carries enough randomness that guessing one is hopeless. That
//! was the whole defence, and it answers the wrong question for the person running the hub:
//! not "can they get in" but "is somebody trying, and from where". A hub that answers every
//! guess at full speed and writes nothing down cannot tell them.
//!
//! # What is counted, and what is not
//!
//! Only a refusal for an **unknown code** counts towards the limit. That is what guessing
//! looks like, and it is the one refusal a machine holding a real invitation never gets. An
//! expired or used-up invitation, a hub without a licence or a full licence are refusals too,
//! but they come from somebody who was handed the file; forty machines behind one NAT address
//! running into a used-up invitation must each still hear *why*, not a lock.
//!
//! Every refusal is written to the hub's own log, but that log cannot be deleted from, by
//! design. So what one address can put there is capped per window as well: otherwise the
//! answer to "somebody is trying" would be a disk that fills and cannot be emptied.
//!
//! # What this does not do
//!
//! It keys on the address the connection came from. Behind a reverse proxy every machine is
//! the proxy, so one guesser there locks everybody out for the rest of the window — the lock
//! is short for that reason, and the refusal says when to try again. Forwarded-for headers are
//! not read: they are written by whoever sends the request.
//!
//! Nothing survives a restart. A limit that resets when the service does is still a limit on
//! guessing, and it is not worth a table.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long one address's count lasts.
pub const WINDOW: Duration = Duration::from_secs(10 * 60);

/// Unknown codes one address may try in a window before it is turned away unheard.
pub const MAX_GUESSES: u32 = 10;

/// Refusals from one address that are written to the hub's log in a window. More than
/// `MAX_GUESSES`, so every guess before a lock is on record, and room for the refusals a
/// person can act on.
pub const MAX_RECORDED: u32 = 20;

/// Addresses tracked at once. Past this, new addresses share one count: a flood from many
/// addresses then locks them together, which is the better failure than unbounded memory.
const MAX_SOURCES: usize = 4096;

/// Who a count belongs to. IPv6 by its /64, because one machine is handed a whole /64 and
/// can change address inside it at will.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Source {
    V4(u32),
    V6(u64),
    Overflow,
}

impl Source {
    fn of(ip: IpAddr) -> Source {
        match ip.to_canonical() {
            IpAddr::V4(v4) => Source::V4(u32::from(v4)),
            IpAddr::V6(v6) => Source::V6((u128::from(v6) >> 64) as u64),
        }
    }
}

#[derive(Debug)]
struct Window {
    opened: Instant,
    guesses: u32,
    recorded: u32,
    /// Admitted and not settled yet. Counted against the limit, so a hundred requests sent at
    /// once cannot all pass the check before the first of them is refused.
    pending: u32,
}

impl Window {
    fn new(now: Instant) -> Window {
        Window {
            opened: now,
            guesses: 0,
            recorded: 0,
            pending: 0,
        }
    }

    fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.opened) >= WINDOW
    }

    /// A fresh count once the window is over. In-flight attempts carry over: they were
    /// admitted under the old count and will settle into the new one.
    fn roll(&mut self, now: Instant) {
        if self.expired(now) {
            let pending = self.pending;
            *self = Window::new(now);
            self.pending = pending;
        }
    }
}

#[derive(Debug, Default)]
pub struct EnrolAttempts {
    by_source: Mutex<HashMap<Source, Window>>,
}

/// Refused sign-ins on the hub's page, counted per address for one purpose only: deciding
/// what goes into the hub's log.
///
/// Unlike enrolment there is no lock. A lock on the password would let anybody on the
/// network keep the operator out of their own hub by typing wrong passwords at it
/// (`admin.rs` says the same about the delay). What the operator gets instead is the record:
/// every refusal from an address, up to `MAX_RECORDED` per window, and then one line saying
/// that more came and were not written down, because the log cannot be emptied.
#[derive(Debug, Default)]
pub struct SignInRefusals {
    by_source: Mutex<HashMap<Source, Window>>,
}

/// What to write for one refused sign-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// The refusal itself.
    Refusal,
    /// The first refusal past the cap: say that further ones from this address are not
    /// written until the window ends, in this long.
    Quiet(Duration),
    /// Past the cap and already said so.
    Nothing,
}

impl SignInRefusals {
    pub fn refused(&self, ip: IpAddr, now: Instant) -> Entry {
        let mut map = self.by_source.lock().unwrap_or_else(|p| p.into_inner());
        let source = EnrolAttempts::slot(&mut map, Source::of(ip), now);
        let w = map.entry(source).or_insert_with(|| Window::new(now));
        w.roll(now);
        w.recorded = w.recorded.saturating_add(1);
        match w.recorded {
            n if n <= MAX_RECORDED => Entry::Refusal,
            n if n == MAX_RECORDED + 1 => {
                Entry::Quiet(WINDOW.saturating_sub(now.saturating_duration_since(w.opened)))
            }
            _ => Entry::Nothing,
        }
    }
}

/// What a settled refusal means for the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settled {
    /// Write this refusal to the hub's log.
    pub record: bool,
    /// This refusal was the one that locked the address: say so in the log, once.
    pub locked: Option<Duration>,
}

/// One admitted attempt. Settle it with what the hub answered; dropped unsettled, it counts
/// as a guess, so an attempt that never finished cannot be a free one.
#[derive(Debug)]
pub struct Ticket {
    attempts: Arc<EnrolAttempts>,
    source: Source,
    settled: bool,
}

impl EnrolAttempts {
    /// May this address try a code now? `Err` carries how long until it may.
    pub fn admit(self: &Arc<Self>, ip: IpAddr, now: Instant) -> Result<Ticket, Duration> {
        let mut map = self.by_source.lock().unwrap_or_else(|p| p.into_inner());
        let source = Self::slot(&mut map, Source::of(ip), now);
        let w = map.entry(source).or_insert_with(|| Window::new(now));
        w.roll(now);
        if w.guesses + w.pending >= MAX_GUESSES {
            let left = WINDOW.saturating_sub(now.saturating_duration_since(w.opened));
            return Err(left.max(Duration::from_secs(1)));
        }
        w.pending += 1;
        Ok(Ticket {
            attempts: Arc::clone(self),
            source,
            settled: false,
        })
    }

    /// The key to count under: the address's own, or the shared one when the table is full
    /// even after windows that are over have been cleared out.
    fn slot(map: &mut HashMap<Source, Window>, source: Source, now: Instant) -> Source {
        if map.contains_key(&source) || map.len() < MAX_SOURCES {
            return source;
        }
        map.retain(|_, w| w.pending > 0 || !w.expired(now));
        if map.len() < MAX_SOURCES {
            source
        } else {
            Source::Overflow
        }
    }

    fn settle(&self, source: Source, refused: Option<bool>, now: Instant) -> Settled {
        let mut map = self.by_source.lock().unwrap_or_else(|p| p.into_inner());
        let w = map.entry(source).or_insert_with(|| Window::new(now));
        w.roll(now);
        w.pending = w.pending.saturating_sub(1);
        let Some(guess) = refused else {
            return Settled {
                record: false,
                locked: None,
            };
        };
        if guess {
            w.guesses += 1;
        }
        let record = w.recorded < MAX_RECORDED;
        if record {
            w.recorded += 1;
        }
        let locked = (guess && w.guesses == MAX_GUESSES)
            .then(|| WINDOW.saturating_sub(now.saturating_duration_since(w.opened)));
        Settled { record, locked }
    }
}

impl Ticket {
    /// The hub enrolled the machine. Nothing to count, nothing to add to the log.
    pub fn enrolled(mut self, now: Instant) {
        self.settled = true;
        self.attempts.settle(self.source, None, now);
    }

    /// The hub refused. `guess` is whether the code was unknown to it.
    pub fn refused(mut self, guess: bool, now: Instant) -> Settled {
        self.settled = true;
        self.attempts.settle(self.source, Some(guess), now)
    }

    /// The request never reached a question about the code (a malformed body, a hub
    /// record that could not be opened). Frees the slot and counts nothing.
    pub fn unasked(mut self, now: Instant) {
        self.settled = true;
        self.attempts.settle(self.source, None, now);
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.settled {
            self.settled = true;
            self.attempts
                .settle(self.source, Some(true), Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn ten_unknown_codes_lock_an_address_for_the_rest_of_the_window() {
        let a = Arc::new(EnrolAttempts::default());
        let t0 = Instant::now();
        for n in 1..=MAX_GUESSES {
            let s = a.admit(ip("192.168.1.50"), t0).unwrap().refused(true, t0);
            assert!(s.record, "guess {n} is on record");
            assert_eq!(s.locked.is_some(), n == MAX_GUESSES, "guess {n}");
        }
        let later = t0 + Duration::from_secs(60);
        let left = a.admit(ip("192.168.1.50"), later).unwrap_err();
        assert_eq!(left, WINDOW - Duration::from_secs(60));
        assert!(
            a.admit(ip("192.168.1.51"), later).is_ok(),
            "another address is not affected"
        );
        assert!(
            a.admit(ip("192.168.1.50"), t0 + WINDOW).is_ok(),
            "the lock ends with the window"
        );
    }

    #[test]
    fn a_refusal_for_a_real_invitation_never_locks_but_is_recorded_only_so_often() {
        let a = Arc::new(EnrolAttempts::default());
        let t0 = Instant::now();
        let recorded = (0..100)
            .filter(|_| {
                a.admit(ip("10.0.0.1"), t0)
                    .unwrap()
                    .refused(false, t0)
                    .record
            })
            .count();
        assert_eq!(recorded, MAX_RECORDED as usize);
        assert!(a.admit(ip("10.0.0.1"), t0).is_ok());
    }

    #[test]
    fn attempts_in_flight_count_before_they_are_answered() {
        let a = Arc::new(EnrolAttempts::default());
        let t0 = Instant::now();
        let held: Vec<Ticket> = (0..MAX_GUESSES)
            .map(|_| a.admit(ip("10.0.0.2"), t0).unwrap())
            .collect();
        assert!(a.admit(ip("10.0.0.2"), t0).is_err());
        for t in held {
            t.unasked(t0);
        }
        assert!(a.admit(ip("10.0.0.2"), t0).is_ok(), "freed once answered");
    }

    #[test]
    fn an_attempt_dropped_unanswered_counts_as_a_guess() {
        let a = Arc::new(EnrolAttempts::default());
        let t0 = Instant::now();
        for _ in 0..MAX_GUESSES {
            drop(a.admit(ip("10.0.0.3"), t0).unwrap());
        }
        assert!(a.admit(ip("10.0.0.3"), t0).is_err());
    }

    #[test]
    fn an_ipv6_address_is_counted_by_its_64_and_a_mapped_ipv4_as_itself() {
        let a = Arc::new(EnrolAttempts::default());
        let t0 = Instant::now();
        for n in 0..MAX_GUESSES {
            a.admit(ip(&format!("2001:db8:1:2::{n:x}")), t0)
                .unwrap()
                .refused(true, t0);
        }
        assert!(a.admit(ip("2001:db8:1:2:ffff::1"), t0).is_err());
        assert!(a.admit(ip("2001:db8:1:3::1"), t0).is_ok());
        for _ in 0..MAX_GUESSES {
            a.admit(ip("192.0.2.7"), t0).unwrap().refused(true, t0);
        }
        assert!(a.admit(ip("::ffff:192.0.2.7"), t0).is_err());
    }

    #[test]
    fn refused_sign_ins_are_recorded_up_to_the_cap_then_said_once_and_never_locked() {
        let s = SignInRefusals::default();
        let t0 = Instant::now();
        let entries: Vec<Entry> = (0..MAX_RECORDED + 5)
            .map(|_| s.refused(ip("192.168.1.60"), t0))
            .collect();
        assert!(
            entries[..MAX_RECORDED as usize]
                .iter()
                .all(|e| *e == Entry::Refusal)
        );
        assert_eq!(entries[MAX_RECORDED as usize], Entry::Quiet(WINDOW));
        assert!(
            entries[MAX_RECORDED as usize + 1..]
                .iter()
                .all(|e| *e == Entry::Nothing)
        );
        assert_eq!(
            s.refused(ip("192.168.1.61"), t0),
            Entry::Refusal,
            "another address has its own count"
        );
        assert_eq!(
            s.refused(ip("192.168.1.60"), t0 + WINDOW),
            Entry::Refusal,
            "a new window records again"
        );
    }

    #[test]
    fn a_full_table_shares_one_count_rather_than_growing() {
        let a = Arc::new(EnrolAttempts::default());
        let t0 = Instant::now();
        for n in 0..MAX_SOURCES as u32 {
            a.admit(IpAddr::V4(n.into()), t0)
                .unwrap()
                .refused(false, t0);
        }
        for n in 0..MAX_GUESSES {
            a.admit(IpAddr::V4((u32::MAX - n).into()), t0)
                .unwrap()
                .refused(true, t0);
        }
        assert!(a.admit(ip("203.0.113.200"), t0).is_err());
        assert!(a.by_source.lock().unwrap().len() <= MAX_SOURCES + 1);
        assert!(
            a.admit(ip("203.0.113.200"), t0 + WINDOW).is_ok(),
            "windows that are over make room again"
        );
    }
}
